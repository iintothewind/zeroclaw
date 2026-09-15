/**
 * Follow-the-tail decision for the WebChat transcript pane.
 *
 * A streaming reply appends text many times per second, and the pane should
 * keep the newest output visible while the reader is already at the tail. The
 * hard part is not the following, it is knowing when to stop: the pane used to
 * re-scroll on every streamed update regardless of where the reader was, so
 * anyone who scrolled up to read history was dragged back down before they
 * could finish a sentence (issue #9562).
 *
 * Reader intent is inferred from the *direction* of a scroll, never from the
 * mere existence of a scroll event:
 *
 * - A backwards move (scrollTop decreasing) is deliberate. Only a person can
 *   move the pane away from the tail, so it suspends following immediately.
 * - Returning near the bottom resumes following.
 * - Anything else leaves the decision untouched.
 *
 * This keeps the pane's own tail-following from being mistaken for intent:
 * a programmatic scroll to the tail only ever moves *forward*, so it can never
 * trip the suspend branch, and it needs no "this scroll was mine" bookkeeping
 * that a queued event could outlive.
 */

/** Distance from the bottom, in px, that still counts as being at the tail. */
export const NEAR_BOTTOM_PX = 150;

/** The three scroll geometry values a follow decision needs. */
export interface ScrollMetrics {
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
}

export interface FollowState {
  /** True while new output should keep the tail in view. */
  following: boolean;
  /** Scroll offset observed last time, used to detect a backwards move. */
  lastScrollTop: number;
}

/**
 * A fresh state that follows the tail. Used on mount and whenever the pane
 * switches conversation, so a reader who had scrolled up in one conversation
 * still lands on the newest message of the next one.
 *
 * Pass the pane's live offset when one is already on screen: seeding
 * `lastScrollTop` with a stale 0 would swallow the reader's first backwards
 * move, because a scroll to 700 reads as forwards when compared against 0.
 */
export function createFollowState(lastScrollTop: number = 0): FollowState {
  return { following: true, lastScrollTop };
}

/**
 * Re-attach to the tail after an explicit move to it.
 *
 * Submitting from the composer is the one action that should always end at the
 * bottom: the input sits below the transcript, so the reader expects their own
 * message and the reply to land there even if they had been reading history.
 *
 * `lastScrollTop` is deliberately left alone. It already tracks the live
 * offset; resetting it here would swallow the reader's next backwards move.
 */
export function resumeFollowing(state: FollowState): FollowState {
  return { ...state, following: true };
}

/** Pixels of unscrolled content below the current offset. */
export function distanceFromBottom(metrics: ScrollMetrics): number {
  const remaining = metrics.scrollHeight - metrics.clientHeight - metrics.scrollTop;
  // Overscroll (elastic scrolling, a shrunk transcript) reports negative.
  return Math.max(0, remaining);
}

export function isNearBottom(
  metrics: ScrollMetrics,
  threshold: number = NEAR_BOTTOM_PX,
): boolean {
  return distanceFromBottom(metrics) <= threshold;
}

/**
 * Fold one scroll event into the follow state.
 *
 * A backwards move wins over proximity: dragging the scrollbar up while the
 * tail is still within the threshold must suspend, not resume, otherwise the
 * reader can never leave the bottom.
 */
export function foldScroll(
  state: FollowState,
  metrics: ScrollMetrics,
  threshold: number = NEAR_BOTTOM_PX,
): FollowState {
  if (metrics.scrollTop < state.lastScrollTop) {
    return { following: false, lastScrollTop: metrics.scrollTop };
  }
  if (isNearBottom(metrics, threshold)) {
    return { following: true, lastScrollTop: metrics.scrollTop };
  }
  // Moving forward but still far from the tail: the reader is scrolling down
  // through history. Leave the decision where it was.
  return { ...state, lastScrollTop: metrics.scrollTop };
}
