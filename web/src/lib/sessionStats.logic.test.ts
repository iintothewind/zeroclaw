import assert from 'node:assert/strict';
import test from 'node:test';

import {
  applyDone,
  applyUsage,
  cacheHitRatio,
  countTranscriptSteps,
  countTranscriptTurns,
  displayStats,
  emptyTokenStats,
  formatTokens,
  turnStarted,
  type TokenStats,
  type TranscriptMessage,
} from './sessionStats.logic.ts';

function run(
  frames: Array<
    | { kind: 'turn' }
    | { kind: 'usage'; f: Parameters<typeof applyUsage>[1] }
    | { kind: 'done'; f: Parameters<typeof applyDone>[1] }
  >,
): TokenStats {
  let s = emptyTokenStats;
  for (const frame of frames) {
    if (frame.kind === 'turn') s = turnStarted(s);
    else if (frame.kind === 'usage') s = applyUsage(s, frame.f);
    else s = applyDone(s, frame.f);
  }
  return s;
}

function msg(
  role: 'user' | 'agent',
  extras: Partial<TranscriptMessage> = {},
): TranscriptMessage {
  return { role, ...extras };
}

function step(
  extras: { thinking?: string; text?: string; tools?: number } = {},
): { thinking: string; text: string; toolCalls: unknown[] } {
  return {
    thinking: extras.thinking ?? '',
    text: extras.text ?? '',
    toolCalls: Array.from({ length: extras.tools ?? 0 }, (_, i) => ({ id: i })),
  };
}

// ── Transcript turns / steps ────────────────────────────────────────────────

test('turns are user bubbles; steps = messages + tool calls + finals', () => {
  const messages = [
    msg('user'),
    msg('agent'),
    msg('user'),
    msg('agent', { toolCall: { name: 'shell' } }),
    msg('agent', {
      segments: {
        steps: [
          step({ text: 'a', tools: 2 }),
          step({ thinking: 'b', tools: 1 }),
        ],
      },
    }),
    msg('user', { ephemeral: true }),
    msg('agent', { notice: true }),
  ];
  assert.equal(countTranscriptTurns(messages), 2);
  // Plain final (1) + (2 messages + 3 tools + 1 final) = 7. Tool card skipped.
  assert.equal(countTranscriptSteps(messages), 1 + 2 + 3 + 1);
});

test('hydrated history: N user + N final answers → N turns and N steps', () => {
  const messages = [
    msg('user'),
    msg('agent'),
    msg('user'),
    msg('agent'),
    msg('user'),
    msg('agent'),
    msg('user'),
    msg('agent'),
    msg('user'),
    msg('agent'),
  ];
  assert.deepEqual(displayStats(messages, emptyTokenStats), {
    turns: 5,
    steps: 5,
    input: 0,
    output: 0,
    cached: 0,
  });
});

test('a multi-step turn counts group messages + tool calls + final answer', () => {
  // Header "9 tool calls · 4 messages" + answer below → 4+9+1=14 for that turn.
  const messages = [
    msg('user'),
    msg('agent'),
    msg('user'),
    msg('agent'),
    msg('user'),
    msg('agent'),
    msg('user'),
    msg('agent', {
      segments: {
        steps: [
          step({ text: '1', tools: 2 }),
          step({ text: '2', tools: 3 }),
          step({ thinking: '3', tools: 2 }),
          step({ text: '4', tools: 2 }),
        ],
      },
    }),
  ];
  assert.equal(countTranscriptTurns(messages), 4);
  assert.equal(countTranscriptSteps(messages), 3 + (4 + 9 + 1));
});

test('an in-flight turn counts closed messages/tools plus the open answer', () => {
  const messages = [msg('user')];
  const live = {
    steps: [
      step({ thinking: 'a', tools: 1 }),
      step({ text: 'b', tools: 2 }),
    ],
    open: step({ text: 'still writing' }),
  };
  // closed: 2 messages + 3 tools; open answer text: 1 → 6
  assert.equal(countTranscriptSteps(messages, live), 2 + 3 + 1);
});

test('displayStats pairs list counts with live token totals', () => {
  const tokens = run([
    { kind: 'turn' },
    { kind: 'usage', f: { input_tokens: 100, cached_input_tokens: 40, output_tokens: 5 } },
  ]);
  const stats = displayStats([msg('user'), msg('agent')], tokens);
  assert.deepEqual(stats, {
    turns: 1,
    steps: 1,
    input: 100,
    output: 5,
    cached: 40,
  });
});

// ── Token accumulation ──────────────────────────────────────────────────────

test('usage folds tokens; it does not invent list turns or steps', () => {
  const tokens = run([
    { kind: 'turn' },
    { kind: 'usage', f: { input_tokens: 17214, output_tokens: 32, cached_input_tokens: 1536 } },
  ]);
  assert.equal(tokens.input, 17214);
  assert.equal(tokens.output, 32);
  assert.equal(tokens.cached, 1536);
  assert.deepEqual(displayStats([], tokens).turns, 0);
  assert.deepEqual(displayStats([], tokens).steps, 0);
});

test('null fields are treated as not-reported, never as NaN', () => {
  const tokens = run([
    { kind: 'usage', f: { input_tokens: null, output_tokens: null, cached_input_tokens: null } },
  ]);
  assert.deepEqual(
    { input: tokens.input, output: tokens.output, cached: tokens.cached },
    { input: 0, output: 0, cached: 0 },
  );
});

test('done reconciles cache when the page missed usage frames', () => {
  const tokens = run([
    { kind: 'turn' },
    { kind: 'usage', f: { input_tokens: 300, output_tokens: 5 } },
    { kind: 'done', f: { cached_input_tokens: 160 } },
  ]);
  assert.equal(tokens.input, 300);
  assert.equal(tokens.cached, 160);
});

test('done never double-counts a fully observed turn', () => {
  const tokens = run([
    { kind: 'turn' },
    { kind: 'usage', f: { input_tokens: 10, cached_input_tokens: 4 } },
    { kind: 'usage', f: { input_tokens: 20, cached_input_tokens: 6 } },
    { kind: 'done', f: { cached_input_tokens: 10 } },
  ]);
  assert.equal(tokens.cached, 10);
});

test('cache rate is cached over input; zero input shows 0%', () => {
  assert.equal(cacheHitRatio(emptyTokenStats), 0);
  const tokens = run([
    { kind: 'usage', f: { input_tokens: 1000, cached_input_tokens: 900, output_tokens: 500 } },
  ]);
  assert.equal(cacheHitRatio(tokens), 0.9);
  const over = run([{ kind: 'usage', f: { input_tokens: 10, cached_input_tokens: 99 } }]);
  assert.equal(cacheHitRatio(over), 1);
});

test('formatTokens matches the reference row', () => {
  assert.equal(formatTokens(0), '0');
  assert.equal(formatTokens(999), '999');
  assert.equal(formatTokens(1000), '1K');
  assert.equal(formatTokens(1500), '1.5K');
  assert.equal(formatTokens(135_000), '135K');
  assert.equal(formatTokens(27_500_000), '27.5M');
  assert.equal(formatTokens(2_000_000_000), '2B');
  assert.equal(formatTokens(Number.NaN), '0');
  assert.equal(formatTokens(Number.POSITIVE_INFINITY), '0');
  assert.equal(formatTokens(-5), '0');
});
