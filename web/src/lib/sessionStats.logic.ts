// Live session telemetry for the chat composer's stats row.
//
// Every number is a running total over *this page's* lifetime for the current
// session, built only from WebSocket frames the client already receives. It
// starts at zero on open and is lost on refresh: nothing is fetched, nothing is
// persisted, nothing is queried. That trade is what buys the feature its entire
// backend away — see `docs/reports/webui-composer-parity-plan.md` §4.1.
//
// Pure and React-free so the metric rules are pinned by unit tests before any
// UI exists, matching the repo's `.logic.ts` convention.

/** Wire payload of a per-step `usage` frame. Every field is nullable: absence
 *  means "the provider did not report this for the call", not zero. */
export interface UsageFrame {
  input_tokens?: number | null;
  output_tokens?: number | null;
  cached_input_tokens?: number | null;
}

/** Wire payload of a turn-terminating frame (`done` / `aborted`). */
export interface TurnTerminalFrame {
  /** The gateway's own count of `Usage` events for the turn it just finished. */
  steps?: number | null;
  cached_input_tokens?: number | null;
}

export interface LiveStats {
  /** Turns started in this window (`agent_start` frames). */
  turns: number;
  /** LLM calls in this window (`usage` frames). */
  steps: number;
  /** Σ `input_tokens` over those frames. */
  input: number;
  /** Σ `output_tokens` over those frames. */
  output: number;
  /** Σ `cached_input_tokens` over those frames — a *subset* of `input`. */
  cached: number;
  /** `usage` frames seen since the last turn boundary. Bookkeeping for
   *  `applyDone`'s reconciliation; never displayed. */
  turnSteps: number;
  /** Σ `cached` since the last turn boundary. Bookkeeping; never displayed. */
  turnCached: number;
}

export const emptyStats: LiveStats = {
  turns: 0,
  steps: 0,
  input: 0,
  output: 0,
  cached: 0,
  turnSteps: 0,
  turnCached: 0,
};

/** Treat an absent or malformed wire value as "not reported" (0 for sums). */
function wireNumber(value: number | null | undefined): number {
  return typeof value === 'number' && Number.isFinite(value) ? value : 0;
}

/** A turn boundary: `agent_start`, emitted once per turn.
 *
 *  This is deliberately *not* the client's send. A send that fails before the
 *  turn starts produces an `error` frame and no `agent_start`, so it does not
 *  count as a turn. */
export function turnStarted(s: LiveStats): LiveStats {
  return { ...s, turns: s.turns + 1, turnSteps: 0, turnCached: 0 };
}

/** One LLM call completed. Counts the step and folds its tokens in.
 *
 *  A frame whose fields are all absent still counts a step: the response was
 *  accepted, the provider just reported nothing, and the step is what the
 *  message-flow view uses as its segment boundary. */
export function applyUsage(s: LiveStats, frame: UsageFrame): LiveStats {
  const cached = wireNumber(frame.cached_input_tokens);
  return {
    ...s,
    steps: s.steps + 1,
    turnSteps: s.turnSteps + 1,
    input: s.input + wireNumber(frame.input_tokens),
    output: s.output + wireNumber(frame.output_tokens),
    cached: s.cached + cached,
    turnCached: s.turnCached + cached,
  };
}

/** Reconcile the finished turn against the gateway's own totals.
 *
 *  The client counts frames; the gateway counts `Usage` events. They agree
 *  unless the page missed part of the turn, so this only ever adds the
 *  difference and never rewrites history — a dropped `agent_start` (the bus is
 *  lossy) leaves `turnSteps` stale, which floors at zero instead of
 *  double-counting. */
export function applyDone(s: LiveStats, frame: TurnTerminalFrame): LiveStats {
  const turnSteps =
    typeof frame.steps === 'number' && Number.isFinite(frame.steps)
      ? frame.steps
      : s.turnSteps;
  const turnCached =
    typeof frame.cached_input_tokens === 'number' &&
    Number.isFinite(frame.cached_input_tokens)
      ? frame.cached_input_tokens
      : s.turnCached;
  return {
    ...s,
    steps: s.steps + Math.max(0, turnSteps - s.turnSteps),
    cached: s.cached + Math.max(0, turnCached - s.turnCached),
  };
}

/** Cache-hit rate, or `null` when no rate can be shown at all.
 *
 *  `cached_input_tokens` is a subset of `input_tokens`, so the rate is
 *  `cached ÷ input` — never `cached ÷ (input + output)`. A provider that stays
 *  silent contributes 0, which makes the result a **lower bound**; callers must
 *  not present it as exact. Clamped to `[0, 1]` so a provider reporting more
 *  cached tokens than prompt tokens cannot render `NaN`, `Infinity` or `100%+`. */
export function cacheHitRatio(s: LiveStats): number | null {
  if (s.input <= 0) return null;
  return Math.min(1, Math.max(0, s.cached / s.input));
}

/** Compact token count: `999`, `1.5K`, `135K`, `27.5M`. */
export function formatTokens(n: number): string {
  if (!Number.isFinite(n) || n < 0) return '0';
  const scaled: [number, string][] = [
    [1e9, 'B'],
    [1e6, 'M'],
    [1e3, 'K'],
  ];
  for (const [unit, suffix] of scaled) {
    if (n >= unit) {
      const value = n / unit;
      // One decimal, but never a trailing `.0` — the reference reads `135K`,
      // not `135.0K`.
      const rounded = Math.round(value * 10) / 10;
      return `${Number.isInteger(rounded) ? rounded : rounded.toFixed(1)}${suffix}`;
    }
  }
  return String(Math.round(n));
}
