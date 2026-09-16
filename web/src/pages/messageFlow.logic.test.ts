import assert from 'node:assert/strict';
import test from 'node:test';

import type { TurnSegments } from '../contexts/turnStream.logic.ts';
import {
  countGroupMessages,
  countToolCalls,
  groupMessages,
  type FlowMessage,
} from './messageFlow.logic.ts';

function msg(over: Partial<FlowMessage> & { id: string }): FlowMessage {
  return { role: 'agent', content: '', timestamp: new Date(0), ...over };
}

function segments(over: Partial<TurnSegments> = {}): TurnSegments {
  return { steps: [], finalText: '', finalThinking: '', ...over };
}

const toolStep = (id: string, text = '') => ({
  thinking: '',
  text,
  toolCalls: [{ name: 'shell', id }],
});

const kinds = (blocks: ReturnType<typeof groupMessages>) => blocks.map((b) => b.kind);

// ── Turns with a trajectory ─────────────────────────────────────────────────

test('a live turn becomes one block that absorbs its tool cards', () => {
  const blocks = groupMessages([
    msg({ id: 'u1', role: 'user', content: 'analyze this' }),
    msg({ id: 't1', toolCall: { name: 'shell', id: 'c1', output: 'ok' } }),
    msg({ id: 't2', toolCall: { name: 'file_read', id: 'c2', output: 'ok' } }),
    msg({
      id: 'a1',
      content: 'the answer',
      markdown: true,
      segments: segments({ steps: [toolStep('c1'), toolStep('c2')], finalText: 'the answer' }),
    }),
  ]);

  assert.deepEqual(kinds(blocks), ['user', 'turn']);
  const turn = blocks[1]!;
  assert.equal(turn.kind, 'turn');
  if (turn.kind !== 'turn') return;
  assert.equal(turn.message.id, 'a1');
  assert.equal(turn.segments.steps.length, 2, 'the loose cards live inside the group now');
});

test('a turn with no tool calls stays a plain bubble', () => {
  // No empty group, no header reading `0 次工具调用`.
  const blocks = groupMessages([
    msg({ id: 'u1', role: 'user', content: 'hi' }),
    msg({ id: 'a1', content: 'hello', segments: segments({ finalText: 'hello' }) }),
  ]);
  assert.deepEqual(kinds(blocks), ['user', 'plain']);
});

// ── Everything that must render exactly as it does today ────────────────────

test('hydrated turns render plain: no group, no empty header', () => {
  // A reloaded conversation carries no segments at all — the accepted
  // live-only trade, asserted rather than tolerated.
  const blocks = groupMessages([
    msg({ id: 'u1', role: 'user', content: 'hi' }),
    msg({ id: 'a1', content: 'hello', markdown: true }),
    msg({ id: 't1', toolCall: { name: 'shell', id: 'c1', output: 'out' } }),
    msg({ id: 'a2', content: 'done', markdown: true }),
  ]);
  assert.deepEqual(kinds(blocks), ['user', 'plain', 'plain', 'plain']);
});

test('tool cards with no following trajectory stay their own blocks', () => {
  // A turn still streaming, or one that ended on a tool call.
  const blocks = groupMessages([
    msg({ id: 'u1', role: 'user', content: 'hi' }),
    msg({ id: 't1', toolCall: { name: 'shell', id: 'c1' } }),
  ]);
  assert.deepEqual(kinds(blocks), ['user', 'plain']);
  assert.equal(blocks[1]!.kind === 'plain' && blocks[1]!.message.id, 't1');
});

test('notice rows keep their own block kind', () => {
  const blocks = groupMessages([
    msg({ id: 'u1', role: 'user', content: 'hi' }),
    msg({ id: 'n1', content: 'context trimmed', notice: true }),
    msg({ id: 'a1', content: 'ok' }),
  ]);
  assert.deepEqual(kinds(blocks), ['user', 'notice', 'plain']);
});

test('consecutive tool-only turns each keep their cards', () => {
  const blocks = groupMessages([
    msg({ id: 't1', toolCall: { name: 'shell', id: 'c1' } }),
    msg({ id: 't2', toolCall: { name: 'shell', id: 'c2' } }),
    msg({ id: 'a1', content: 'first' }),
    msg({ id: 't3', toolCall: { name: 'shell', id: 'c3' } }),
    msg({ id: 'a2', content: 'second' }),
  ]);
  assert.deepEqual(kinds(blocks), ['plain', 'plain', 'plain', 'plain', 'plain']);
});

test('two live turns each get their own group', () => {
  const blocks = groupMessages([
    msg({ id: 't1', toolCall: { name: 'shell', id: 'c1' } }),
    msg({ id: 'a1', content: 'one', segments: segments({ steps: [toolStep('c1')] }) }),
    msg({ id: 't2', toolCall: { name: 'shell', id: 'c2' } }),
    msg({ id: 'a2', content: 'two', segments: segments({ steps: [toolStep('c2')] }) }),
  ]);
  assert.deepEqual(kinds(blocks), ['turn', 'turn']);
});

// ── The tool-activity toggle ────────────────────────────────────────────────

test('hiding tool activity hides the loose cards and the group body', () => {
  const messages = [
    msg({ id: 't1', toolCall: { name: 'shell', id: 'c1' } }),
    msg({ id: 'a1', content: 'one', segments: segments({ steps: [toolStep('c1')] }) }),
    msg({ id: 'a2', content: 'plain answer' }),
  ];
  assert.deepEqual(kinds(groupMessages(messages, { showToolActivity: true })), [
    'turn',
    'plain',
  ]);
  assert.deepEqual(kinds(groupMessages(messages, { showToolActivity: false })), [
    'plain',
    'plain',
  ]);
});

// ── Header counts ───────────────────────────────────────────────────────────

test('a card the live group already carries is not rendered twice', () => {
  // While a turn streams there is no committed message to absorb its cards, so
  // the in-flight group carries them — and the loose card of the same
  // `tool_call_id` has to go, or every call renders twice for the duration.
  const messages = [
    msg({ id: 'u1', role: 'user', content: 'analyze this' }),
    msg({ id: 't1', toolCall: { name: 'shell', id: 'c1' } }),
    msg({ id: 't2', toolCall: { name: 'file_read', id: 'c2' } }),
  ];

  const live = groupMessages(messages, { liveSteps: [toolStep('c1')] });
  assert.deepEqual(
    live.map((block) => block.message.id),
    ['u1', 't2'],
    'the claimed card is dropped, the unclaimed one is untouched',
  );

  // No live turn, nothing claimed: today's behaviour, unchanged.
  assert.deepEqual(
    groupMessages(messages).map((block) => block.message.id),
    ['u1', 't1', 't2'],
  );
});

test('the header counts calls and messages', () => {
  const seg = segments({
    steps: [
      { thinking: 'thought', text: '', toolCalls: [{ name: 'shell', id: 'c1' }] },
      { thinking: '', text: '', toolCalls: [{ name: 'a', id: 'c2' }, { name: 'b', id: 'c3' }] },
      { thinking: '', text: 'explaining', toolCalls: [] },
    ],
  });
  assert.equal(countToolCalls(seg), 3, 'parallel calls count individually');
  assert.equal(countGroupMessages(seg), 2, 'steps carrying thinking or text');
});

test('an empty group counts zero', () => {
  assert.equal(countToolCalls(segments()), 0);
  assert.equal(countGroupMessages(segments()), 0);
});

test('grouping does not mutate the input list', () => {
  const messages = [
    msg({ id: 't1', toolCall: { name: 'shell', id: 'c1' } }),
    msg({ id: 'a1', content: 'one', segments: segments({ steps: [toolStep('c1')] }) }),
  ];
  const snapshot = JSON.stringify(messages);
  groupMessages(messages);
  assert.equal(JSON.stringify(messages), snapshot);
});
