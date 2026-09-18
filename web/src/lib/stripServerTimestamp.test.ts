import assert from 'node:assert/strict';
import test from 'node:test';

import { stripServerTimestamp } from './stripServerTimestamp.ts';

// The transcript renders every non-local user bubble through this, so the two
// envelope shapes it removes and the cases it must leave alone are pinned here.
// These cases used to live in `contexts/historyTrimMerge.logic.test.ts`, a file
// that went away with the module it was named after.

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

test('stripServerTimestamp keeps a labeled envelope without the blank line', () => {
  // A user-authored example that merely starts with the label is not an
  // envelope — the runtime always writes the blank line.
  const content = '[CURRENT DATE & TIME: 2026-09-09 22:00:00 PDT] and then';
  assert.equal(stripServerTimestamp(content), content);
});
