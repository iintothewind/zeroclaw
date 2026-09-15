import assert from 'node:assert/strict';
import test from 'node:test';

import {
  createFollowState,
  distanceFromBottom,
  foldScroll,
  isNearBottom,
  NEAR_BOTTOM_PX,
  resumeFollowing,
  type FollowState,
  type ScrollMetrics,
} from './agentChatScroll.logic.ts';

/** Transcript pane geometry used across the tests: 2000px of content in a
 *  600px viewport, so the bottom-most offset is 1400. */
const CONTENT = 2000;
const VIEWPORT = 600;
const MAX_SCROLL = CONTENT - VIEWPORT;

function metrics(scrollTop: number, content = CONTENT): ScrollMetrics {
  return { scrollTop, scrollHeight: content, clientHeight: VIEWPORT };
}

/** Fold a sequence of offsets, as a scroll handler would. */
function runOffsets(state: FollowState, offsets: number[]): FollowState {
  let next = state;
  for (const offset of offsets) next = foldScroll(next, metrics(offset));
  return next;
}

// ── geometry helpers ────────────────────────────────────────────────────────

test('distanceFromBottom reports unscrolled content below the offset', () => {
  assert.equal(distanceFromBottom(metrics(0)), MAX_SCROLL);
  assert.equal(distanceFromBottom(metrics(MAX_SCROLL)), 0);
  assert.equal(distanceFromBottom(metrics(MAX_SCROLL / 2)), MAX_SCROLL / 2);
});

test('distanceFromBottom clamps overscroll to zero', () => {
  // Elastic scrolling and a transcript that shrank under the reader both report
  // an offset past the end; that is still "at the bottom", not a negative gap.
  assert.equal(distanceFromBottom(metrics(MAX_SCROLL + 80)), 0);
});

test('isNearBottom uses an inclusive threshold', () => {
  assert.equal(isNearBottom(metrics(MAX_SCROLL - NEAR_BOTTOM_PX)), true);
  assert.equal(isNearBottom(metrics(MAX_SCROLL - NEAR_BOTTOM_PX - 1)), false);
  assert.equal(isNearBottom(metrics(MAX_SCROLL - 20), 10), false);
  assert.equal(isNearBottom(metrics(MAX_SCROLL - 10), 10), true);
});

// ── the state a freshly mounted pane starts in ──────────────────────────────

test('a fresh pane follows the tail', () => {
  assert.deepEqual(createFollowState(), { following: true, lastScrollTop: 0 });
});

test('createFollowState can be seeded with the pane’s live offset', () => {
  // A conversation switch re-uses the pane, so the offset on screen is not 0.
  assert.deepEqual(createFollowState(840), { following: true, lastScrollTop: 840 });
});

// ── programmatic tail-following must never look like reader intent ──────────

test('the pane following its own tail never suspends', () => {
  // Streaming: content grows, the pane re-pins to the bottom each tick. Every
  // move is forwards, so none of them may be read as a backwards move.
  let state = createFollowState();
  for (const offset of [MAX_SCROLL, MAX_SCROLL, MAX_SCROLL, MAX_SCROLL]) {
    state = foldScroll(state, metrics(offset));
  }
  assert.equal(state.following, true);
});

// ── issue #9562: a backwards move suspends, returning to the tail resumes ───

test('scrolling up suspends following', () => {
  let state = runOffsets(createFollowState(), [MAX_SCROLL]);
  assert.equal(state.following, true);

  state = foldScroll(state, metrics(400));
  assert.equal(state.following, false, 'a backwards move is reader intent');
});

test('a suspended pane stays suspended while the stream keeps appending', () => {
  // The reported bug: the reader scrolls up, then every streamed update drags
  // them back to the tail. Re-pinning must be a no-op for a suspended pane.
  let state = runOffsets(createFollowState(), [MAX_SCROLL]);
  state = foldScroll(state, metrics(200));
  assert.equal(state.following, false);

  // The pane's own re-pin attempts: content grew, so the bottom moved further
  // down, but the reader's offset did not change.
  for (const content of [2400, 2800, 3200]) {
    state = foldScroll(state, metrics(200, content));
    assert.equal(state.following, false, 'still reading history');
  }
});

test('returning near the bottom resumes following', () => {
  let state = runOffsets(createFollowState(), [MAX_SCROLL]);
  state = foldScroll(state, metrics(200));
  assert.equal(state.following, false);

  state = foldScroll(state, metrics(MAX_SCROLL - 40));
  assert.equal(state.following, true);
});

test('a backwards move near the bottom still suspends', () => {
  // Ordering rule: if proximity were checked first, dragging the scrollbar up
  // a few pixels would immediately resume and the reader could never leave.
  let state = runOffsets(createFollowState(), [MAX_SCROLL]);
  state = foldScroll(state, metrics(MAX_SCROLL - 10));
  assert.equal(state.following, false);
});

test('scrolling down through history does not resume on its own', () => {
  let state = runOffsets(createFollowState(), [MAX_SCROLL]);
  state = foldScroll(state, metrics(300));
  assert.equal(state.following, false);

  // Forwards, but still far from the tail — the reader is browsing, not done.
  state = foldScroll(state, metrics(900));
  assert.equal(state.following, false);
});

test('a suspended pane resumes once the reader reaches the tail again', () => {
  let state = runOffsets(createFollowState(), [MAX_SCROLL]);
  state = runOffsets(state, [300, 900, 1200]);
  assert.equal(state.following, false);
  state = foldScroll(state, metrics(MAX_SCROLL - 5));
  assert.equal(state.following, true);
});

// ── lastScrollTop bookkeeping ───────────────────────────────────────────────

test('lastScrollTop tracks every observed offset', () => {
  let state = runOffsets(createFollowState(), [MAX_SCROLL, 900, 300]);
  assert.equal(state.lastScrollTop, 300);
  // The next comparison is against the most recent offset, not the first.
  state = foldScroll(state, metrics(320));
  assert.equal(state.following, false, '320 is still far from the tail');
  state = foldScroll(state, metrics(300));
  assert.equal(state.following, false, 'a 20px step back is a backwards move');
});

test('a transcript shorter than the viewport counts as being at the tail', () => {
  // No scrollbar at all: scrollTop is pinned at 0 and the pane should follow.
  const short: ScrollMetrics = { scrollTop: 0, scrollHeight: 400, clientHeight: VIEWPORT };
  assert.equal(isNearBottom(short), true);
  assert.equal(foldScroll(createFollowState(), short).following, true);
});

// ── sending a message is an explicit "show me the tail" ─────────────────────

test('resumeFollowing re-attaches and keeps the live offset', () => {
  let state = runOffsets(createFollowState(), [MAX_SCROLL, 300]);
  assert.equal(state.following, false);

  state = resumeFollowing(state);
  assert.equal(state.following, true);
  assert.equal(state.lastScrollTop, 300, 'the live offset must survive');
});

test('the reader can still scroll away after resumeFollowing', () => {
  // Seeding lastScrollTop back to 0 here would make the next backwards move
  // read as forwards, so the pane could never be suspended again.
  let state = resumeFollowing(runOffsets(createFollowState(), [MAX_SCROLL, 300]));
  state = foldScroll(state, metrics(200));
  assert.equal(state.following, false);
});

test('a seeded offset still detects the reader scrolling away', () => {
  // Same hazard on a conversation switch, where the pane keeps its old offset.
  const state = foldScroll(createFollowState(840), metrics(700));
  assert.equal(state.following, false);
});
