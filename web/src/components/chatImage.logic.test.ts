import assert from 'node:assert/strict';
import test from 'node:test';

import { chatImageLocksBubbleWidth } from './chatImage.logic.ts';

test('a tall screenshot would collapse to a sliver under a short cap, so it keeps the bubble width', () => {
  // 1080×8000 at a 480px bubble: a 400px height cap yields a 54px-wide strip.
  assert.equal(chatImageLocksBubbleWidth(480, 1080, 8000), true);
});

test('a landscape image already fits under the cap, so it stays intrinsic', () => {
  assert.equal(chatImageLocksBubbleWidth(480, 1920, 1080), false);
});

test('a square image stays a square instead of being stretched to the bubble width', () => {
  assert.equal(chatImageLocksBubbleWidth(500, 1000, 1000), false);
});

test('missing dimensions do not lock the bubble width', () => {
  assert.equal(chatImageLocksBubbleWidth(0, 1080, 8000), false);
  assert.equal(chatImageLocksBubbleWidth(480, 0, 8000), false);
  assert.equal(chatImageLocksBubbleWidth(480, 1080, 0), false);
});
