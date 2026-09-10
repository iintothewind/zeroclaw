import assert from 'node:assert/strict';
import test from 'node:test';

import {
  normalizeUserContent,
  stripServerTimestamp,
} from '../lib/stripServerTimestamp.ts';
import { selectLocalPendingAfterRebuild } from './historyTrimMerge.logic.ts';

type Msg = {
  id: string;
  role: 'user' | 'agent';
  content: string;
  local?: boolean;
  notice?: boolean;
};

function user(id: string, content: string, local = false): Msg {
  return { id, role: 'user', content, local: local || undefined };
}

function agent(id: string, content: string): Msg {
  return { id, role: 'agent', content };
}

// ── strip / normalize ───────────────────────────────────────────────────────

test('stripServerTimestamp removes channel wall-clock prefix', () => {
  assert.equal(
    stripServerTimestamp('[2026-09-09 22:00:00 PDT] hello'),
    'hello',
  );
});

test('stripServerTimestamp removes agent CURRENT DATE envelope', () => {
  assert.equal(
    stripServerTimestamp(
      '[CURRENT DATE & TIME: 2026-09-09 22:00:00 PDT]\n\nhello',
    ),
    'hello',
  );
});

test('stripServerTimestamp leaves mid-message brackets intact', () => {
  const body = 'see log [2026-09-09 22:00:00 PDT] later';
  assert.equal(stripServerTimestamp(body), body);
  const labeled =
    'Explain [CURRENT DATE & TIME: 2026-09-09 22:00:00 UTC] as a literal';
  assert.equal(stripServerTimestamp(labeled), labeled);
});

test('normalizeUserContent trims after strip', () => {
  assert.equal(
    normalizeUserContent(
      '[CURRENT DATE & TIME: 2026-03-14 09:30:00 UTC]\n\n  hello  ',
    ),
    'hello',
  );
});

// ── selectLocalPendingAfterRebuild ──────────────────────────────────────────

test('drops trailing local already present in rebuilt (agent envelope)', () => {
  const prev = [
    user('old', 'earlier'),
    agent('a1', 'ok'),
    user('local-1', 'hello', true),
  ];
  const rebuilt = [
    user('srv-1', '[CURRENT DATE & TIME: 2026-09-09 22:00:00 PDT]\n\nhello'),
  ];
  assert.deepEqual(selectLocalPendingAfterRebuild(prev, rebuilt), []);
});

test('keeps trailing local not yet on the server', () => {
  const prev = [
    user('old', 'earlier'),
    agent('a1', 'ok'),
    user('local-1', 'pending', true),
  ];
  const rebuilt = [
    user('srv-1', '[CURRENT DATE & TIME: 2026-09-09 22:00:00 PDT]\n\nearlier'),
    agent('a1', 'ok'),
  ];
  const pending = selectLocalPendingAfterRebuild(prev, rebuilt);
  assert.equal(pending.length, 1);
  assert.equal(pending[0]?.id, 'local-1');
});

test('identical double-send keeps the extra local (tail align)', () => {
  const prev = [
    user('local-1', 'hello', true),
    user('local-2', 'hello', true),
  ];
  const rebuilt = [
    user('srv-1', '[CURRENT DATE & TIME: 2026-09-09 22:00:00 PDT]\n\nhello'),
  ];
  const pending = selectLocalPendingAfterRebuild(prev, rebuilt);
  assert.equal(pending.length, 1);
  assert.equal(pending[0]?.id, 'local-1');
});

test('ignores trailing notices when finding last server message', () => {
  const prev = [
    user('srv-visible', 'earlier'),
    agent('a1', 'ok'),
    { id: 'n1', role: 'agent' as const, content: 'trimmed', notice: true },
    user('local-1', 'hello', true),
  ];
  const rebuilt = [
    user('srv-1', '[2026-09-09 22:00:00 PDT] hello'),
  ];
  assert.deepEqual(selectLocalPendingAfterRebuild(prev, rebuilt), []);
});
