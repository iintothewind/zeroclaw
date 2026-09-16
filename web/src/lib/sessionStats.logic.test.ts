import assert from 'node:assert/strict';
import test from 'node:test';

import {
  applyDone,
  applyUsage,
  cacheHitRatio,
  emptyStats,
  formatTokens,
  turnStarted,
  type LiveStats,
} from './sessionStats.logic.ts';

/** Fold a frame sequence the way AgentContext's handler does. */
function run(frames: Array<{ kind: 'turn' } | { kind: 'usage'; f: Parameters<typeof applyUsage>[1] } | { kind: 'done'; f: Parameters<typeof applyDone>[1] }>): LiveStats {
  let s = emptyStats;
  for (const frame of frames) {
    if (frame.kind === 'turn') s = turnStarted(s);
    else if (frame.kind === 'usage') s = applyUsage(s, frame.f);
    else s = applyDone(s, frame.f);
  }
  return s;
}

// ── Steps ───────────────────────────────────────────────────────────────────

test('one usage frame is one step and folds its tokens in', () => {
  const s = run([
    { kind: 'turn' },
    { kind: 'usage', f: { input_tokens: 17214, output_tokens: 32, cached_input_tokens: 1536 } },
  ]);
  assert.equal(s.turns, 1);
  assert.equal(s.steps, 1);
  assert.equal(s.input, 17214);
  assert.equal(s.output, 32);
  assert.equal(s.cached, 1536);
});

test('a usage frame with no reported numbers still counts a step', () => {
  // The response was accepted; the provider just stayed silent. The step is
  // still a step — it is the message-flow view's segment boundary.
  const s = run([{ kind: 'turn' }, { kind: 'usage', f: {} }]);
  assert.equal(s.steps, 1);
  assert.equal(s.input, 0);
  assert.equal(s.output, 0);
  assert.equal(s.cached, 0);
});

test('null fields are treated as not-reported, never as NaN', () => {
  const s = run([
    { kind: 'usage', f: { input_tokens: null, output_tokens: null, cached_input_tokens: null } },
  ]);
  assert.equal(s.steps, 1);
  assert.deepEqual(
    { input: s.input, output: s.output, cached: s.cached },
    { input: 0, output: 0, cached: 0 },
  );
  assert.equal(cacheHitRatio(s), null, 'no input reported means no rate to show');
});

// ── Turns ───────────────────────────────────────────────────────────────────

test('turns advance on the turn boundary, not on the client send', () => {
  // A send that fails before the turn starts yields an `error` frame and no
  // `agent_start`, so it must not show up as a turn.
  const failed = run([{ kind: 'usage', f: { input_tokens: 10 } }]);
  assert.equal(failed.turns, 0, 'frames without a turn boundary are not a turn');
  assert.equal(failed.steps, 1, 'but the work that did happen is still counted');
});

test('a multi-step turn keeps steps and turns in one window', () => {
  const s = run([
    { kind: 'turn' },
    { kind: 'usage', f: { input_tokens: 100, cached_input_tokens: 80, output_tokens: 5 } },
    { kind: 'usage', f: { input_tokens: 200, cached_input_tokens: 80, output_tokens: 5 } },
    { kind: 'usage', f: { input_tokens: 300, output_tokens: 5 } },
    { kind: 'done', f: { steps: 3, cached_input_tokens: 160 } },
  ]);
  assert.equal(s.turns, 1);
  assert.equal(s.steps, 3);
  assert.equal(s.input, 600);
  assert.equal(s.output, 15);
  assert.equal(s.cached, 160);
});

test('after N turns the row equals what the frames say', () => {
  // Acceptance 2, stated as the identity it is: turns are the `agent_start`
  // count, steps the `usage` count, tokens Σ(input + output), rate
  // Σcached ÷ Σinput — accumulated across the window, not reset per turn.
  //
  //   turn 1   100 in ( 60 cached)   10 out   1 step
  //   turn 2   200 in (150 cached)   20 out   2 steps
  //            300 in (250 cached)   30 out
  //   turn 3   400 in (uncached)     40 out   1 step
  //   ───────────────────────────────────────────────
  //   totals 3 turns, 4 steps, 1000 in, 100 out, 460 cached → 46%
  const s = run([
    { kind: 'turn' },
    { kind: 'usage', f: { input_tokens: 100, cached_input_tokens: 60, output_tokens: 10 } },
    { kind: 'done', f: { steps: 1, cached_input_tokens: 60 } },
    { kind: 'turn' },
    { kind: 'usage', f: { input_tokens: 200, cached_input_tokens: 150, output_tokens: 20 } },
    { kind: 'usage', f: { input_tokens: 300, cached_input_tokens: 250, output_tokens: 30 } },
    { kind: 'done', f: { steps: 2, cached_input_tokens: 400 } },
    { kind: 'turn' },
    { kind: 'usage', f: { input_tokens: 400, output_tokens: 40 } },
    { kind: 'done', f: { steps: 1 } },
  ]);
  assert.equal(s.turns, 3);
  assert.equal(s.steps, 4);
  assert.equal(s.input, 1000);
  assert.equal(s.output, 100);
  assert.equal(s.cached, 460);
  assert.equal(Math.round(cacheHitRatio(s)! * 100), 46);
});

// ── Reconciliation ──────────────────────────────────────────────────────────

test('done reconciles a turn the page only partly observed', () => {
  // Reconnected mid-turn: two of the three usage frames were missed.
  const s = run([
    { kind: 'turn' },
    { kind: 'usage', f: { input_tokens: 300, output_tokens: 5 } },
    { kind: 'done', f: { steps: 3, cached_input_tokens: 160 } },
  ]);
  assert.equal(s.steps, 3, "takes the gateway's count for the turn it finished");
  assert.equal(s.input, 300, 'missing frames cannot be invented');
  assert.equal(s.cached, 160);
});

test('done never double-counts a turn that was fully observed', () => {
  const s = run([
    { kind: 'turn' },
    { kind: 'usage', f: { input_tokens: 10, cached_input_tokens: 4 } },
    { kind: 'usage', f: { input_tokens: 20, cached_input_tokens: 6 } },
    { kind: 'done', f: { steps: 2, cached_input_tokens: 10 } },
  ]);
  assert.equal(s.steps, 2);
  assert.equal(s.cached, 10);
});

test('a lost agent_start cannot make done subtract steps', () => {
  // The event bus is lossy, so the boundary frame can go missing. Steps must
  // stay monotonic rather than going backwards.
  const s = run([
    { kind: 'usage', f: { input_tokens: 10 } },
    { kind: 'done', f: { steps: 1, cached_input_tokens: 0 } },
    { kind: 'usage', f: { input_tokens: 10 } },
    { kind: 'done', f: { steps: 1, cached_input_tokens: 0 } },
  ]);
  assert.equal(s.steps, 2);
  assert.equal(s.cached, 0);
});

test('a cancelled turn still advances by its steps and tokens', () => {
  // `aborted` carries the same totals as `done` (gateway ws.rs).
  const s = run([
    { kind: 'turn' },
    { kind: 'usage', f: { input_tokens: 120, cached_input_tokens: 64, output_tokens: 7 } },
    { kind: 'done', f: { steps: 1, cached_input_tokens: 64 } },
  ]);
  assert.equal(s.turns, 1);
  assert.equal(s.steps, 1);
  assert.equal(s.input, 120);
  assert.equal(s.output, 7);
});

// ── Cache-hit rate ──────────────────────────────────────────────────────────

test('cache rate is cached over input, not over input plus output', () => {
  const s = run([
    { kind: 'usage', f: { input_tokens: 1000, cached_input_tokens: 900, output_tokens: 500 } },
  ]);
  assert.equal(cacheHitRatio(s), 0.9);
});

test('cache rate is null with no input, and clamped when a provider over-reports', () => {
  assert.equal(cacheHitRatio(emptyStats), null);
  const over = run([{ kind: 'usage', f: { input_tokens: 10, cached_input_tokens: 99 } }]);
  assert.equal(cacheHitRatio(over), 1, 'never renders 100%+');
  const s = cacheHitRatio(over);
  assert.ok(s !== null && Number.isFinite(s) && s <= 1);
});

test('cache rate stays finite across an all-null session', () => {
  const s = run([
    { kind: 'usage', f: {} },
    { kind: 'usage', f: {} },
    { kind: 'done', f: {} },
  ]);
  const ratio = cacheHitRatio(s);
  assert.equal(ratio, null);
  assert.ok(!Number.isNaN(s.steps) && !Number.isNaN(s.input));
});

// ── Formatting ──────────────────────────────────────────────────────────────

test('formatTokens matches the reference row', () => {
  assert.equal(formatTokens(0), '0');
  assert.equal(formatTokens(999), '999');
  assert.equal(formatTokens(1000), '1K');
  assert.equal(formatTokens(1500), '1.5K');
  assert.equal(formatTokens(135_000), '135K');
  assert.equal(formatTokens(27_500_000), '27.5M');
  assert.equal(formatTokens(2_000_000_000), '2B');
});

test('formatTokens never renders NaN or Infinity', () => {
  assert.equal(formatTokens(Number.NaN), '0');
  assert.equal(formatTokens(Number.POSITIVE_INFINITY), '0');
  assert.equal(formatTokens(-5), '0');
});
