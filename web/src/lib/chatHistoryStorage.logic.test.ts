import assert from 'node:assert/strict';
import test from 'node:test';

import {
  mapServerMessagesToPersisted,
  parseNativeAssistantToolPayload,
  parseNativeToolResultEnvelope,
} from './chatHistoryStorage.logic.ts';

test('parseNativeAssistantToolPayload extracts text and tool_calls', () => {
  const payload = JSON.stringify({
    content: 'Let me read that file.',
    tool_calls: [
      { id: 'call_1', name: 'file_read', arguments: '{"path":"a.ts"}' },
    ],
  });
  const parsed = parseNativeAssistantToolPayload(payload);
  assert.ok(parsed);
  assert.equal(parsed!.text, 'Let me read that file.');
  assert.equal(parsed!.toolCalls.length, 1);
  assert.equal(parsed!.toolCalls[0]!.name, 'file_read');
  assert.equal(parsed!.toolCalls[0]!.id, 'call_1');
  assert.deepEqual(parsed!.toolCalls[0]!.args, { path: 'a.ts' });
});

test('parseNativeAssistantToolPayload returns null for plain assistant text', () => {
  assert.equal(parseNativeAssistantToolPayload('hello world'), null);
  assert.equal(parseNativeAssistantToolPayload('{"content":"only text"}'), null);
});

test('parseNativeToolResultEnvelope unwraps content + tool_call_id', () => {
  const envelope = JSON.stringify({
    content: 'file contents here',
    tool_call_id: 'call_1',
  });
  assert.deepEqual(parseNativeToolResultEnvelope(envelope), {
    toolCallId: 'call_1',
    output: 'file contents here',
  });
});

test('mapServerMessagesToPersisted rebuilds ToolCallCards instead of raw JSON', () => {
  const before = Date.now();
  const assistant = JSON.stringify({
    content: null,
    tool_calls: [
      { id: 'chatcmpl-tool-af8408577ab431c3', name: 'file_read', arguments: '{"path":"build.sh"}' },
      { id: 'call_2', name: 'shell', arguments: '{"command":"ls"}' },
      { id: 'call_3', name: 'shell', arguments: '{"command":"pwd"}' },
      { id: 'call_4', name: 'shell', arguments: '{"command":"echo hi"}' },
      { id: 'call_5', name: 'shell', arguments: '{"command":"date"}' },
    ],
  });
  const tool = JSON.stringify({
    content: '1758:         IFS=$_oldifs',
    tool_call_id: 'chatcmpl-tool-af8408577ab431c3',
  });

  // 3 rows → expands to user + 5 cards + Done = 7 bubbles (more than rows).
  const bubbles = mapServerMessagesToPersisted([
    { role: 'user', content: 'read the build script', created_at: null },
    { role: 'assistant', content: assistant, created_at: null },
    { role: 'tool', content: tool, created_at: null },
    { role: 'assistant', content: 'Done reading.', created_at: null },
  ]);
  const after = Date.now();

  assert.equal(bubbles.length, 7);
  assert.equal(bubbles[0]!.role, 'user');
  assert.equal(bubbles[1]!.toolCall?.name, 'file_read');
  assert.equal(bubbles[1]!.toolCall?.id, 'chatcmpl-tool-af8408577ab431c3');
  assert.equal(bubbles[1]!.toolCall?.output, '1758:         IFS=$_oldifs');
  assert.equal(bubbles[6]!.content, 'Done reading.');
  assert.equal(bubbles[6]!.markdown, true);

  // No raw JSON blobs left in agent content.
  for (const b of bubbles) {
    if (b.toolCall) {
      assert.equal(b.content, '');
    } else {
      assert.ok(!b.content.includes('tool_call_id'));
    }
  }

  // Timestamps use final bubble count — last ≈ now-1s, never in the future.
  const lastMs = Date.parse(bubbles.at(-1)!.timestamp);
  assert.ok(lastMs <= after);
  assert.ok(lastMs >= before - 2000);
  for (let i = 1; i < bubbles.length; i++) {
    const prev = Date.parse(bubbles[i - 1]!.timestamp);
    const cur = Date.parse(bubbles[i]!.timestamp);
    assert.equal(cur - prev, 1000);
  }
});

test('mapServerMessagesToPersisted orphan tool result still becomes a card', () => {
  const tool = JSON.stringify({
    content: 'orphan output',
    tool_call_id: 'missing',
  });
  const bubbles = mapServerMessagesToPersisted([
    { role: 'tool', content: tool, created_at: null },
  ]);
  assert.equal(bubbles.length, 1);
  assert.equal(bubbles[0]!.toolCall?.output, 'orphan output');
  assert.equal(bubbles[0]!.toolCall?.id, 'missing');
  assert.ok(!bubbles[0]!.content.includes('tool_call_id'));
});
