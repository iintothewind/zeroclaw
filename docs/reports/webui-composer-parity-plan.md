# Plan: web chat composer — live session stats row + context ring

**Status:** implemented (v2) — see `webui-overall-plan.md` §10 for the landed commits, the
deviations from §3, and the review rounds. This document is the design record and its acceptance
criteria, not a live status.
**Scope:** `crates/zeroclaw-gateway/src/ws.rs` (~15 lines) + `web/` (React dashboard). **No database query, no ledger read, no new endpoint.**
**Supersedes:** v1, which proposed an on-demand ledger scan behind `GET /api/sessions/{id}/usage`. Dropped — see §11.

Reference UI: DeepSeek Harness composer — `+` upload at the input's bottom-left, circular context
occupancy at the bottom-right (click opens a detail panel), and a stats line under the input.

---

## 1. Goal

Give the ZeroClaw web chat composer the same at-a-glance telemetry as the reference, without adding
any server-side query:

1. **Context ring** at the input's bottom-right, click opens a detail panel. Replaces the
   always-visible linear `ContextBar`.
2. **Stats row** under the input: turns · steps · total tokens · cache-hit rate.
3. **Remove** the always-visible linear `ContextBar` from the footer.

The row counts **the current session, as observed by this page**. It starts at zero and is built
entirely from the WebSocket stream the client already receives. Nothing is fetched, nothing is
persisted, nothing is queried.

---

## 2. Non-goals

Confirmed out of scope:

- **Historical counts.** Opening an existing conversation does not show what happened in it before
  you opened it, and a refresh does not restore what you saw. This is the deliberate trade that buys
  the whole backend away (§4.1).
- **Cost display.** Not in the reference row, and production models are unpriced — every one of the
  1,600 conversation rows in the ledger carries `pricing_available: false` (§11). Dropped entirely
  rather than rendered as `$0.00`.
- **Context breakdown by category** (system prompt / tool definitions / conversation messages).
  The data does not exist; it would need new instrumentation in
  `crates/zeroclaw-runtime/src/agent/turn/provider_call.rs`.
- **The reference's collapsible tool-call group** (`7 次工具调用 · 6 条消息`, expanding to the
  interleaved thinking / text / tool-call sequence). This is message-list rendering, not the composer,
  and it cannot be durable: tool calls are not persisted (fact 13). Reproducible live, lost on reload —
  a separate change with its own persistence decision. See §3.
- Trajectory view. `@` file/session references. Permission presets. Queue / steer send.
  Non-image attachments.
- **TTFT and tok/s.** `TurnEvent::Usage` (`crates/zeroclaw-api/src/agent.rs:114-128`) carries token
  counts only — no duration, no per-token timestamps. TTFT cannot be reconstructed at all; tok/s
  could only be a front-end wall-clock estimate. Both deferred rather than faked.
- The streaming-markdown defect (`web/src/pages/AgentChat.tsx:840` renders `whitespace-pre-wrap`
  during streaming and only switches to markdown once the message settles). Real, but a separate change.

---

## 3. Verified facts

Every displayed number must come from a real source. These were verified in the tree:

| # | Fact | Evidence |
|---|---|---|
| 1 | The WS `done` frame already carries per-turn `input_tokens`, `output_tokens`, `tokens_used`, `last_input_tokens`, `max_context_tokens`, `cost_usd` | `ws.rs:1550-1559` |
| 2 | The web client already reads `max_context_tokens` and `last_input_tokens` from it | `AgentContext.tsx:464-475` |
| 3 | `TurnEvent::Usage` is emitted **once per LLM call** — i.e. once per step | `parse_response.rs:304`; doc comment at `zeroclaw-api/src/agent.rs:114` |
| 4 | `Usage` carries `input_tokens`, `cached_input_tokens`, `output_tokens`, `cost_usd`, all `Option`; **no** `total_tokens` | `zeroclaw-api/src/agent.rs:115-128` |
| 5 | **The gateway discards `Usage` and never forwards it.** The match arm accumulates then `continue`s | `ws.rs:1295-1310` |
| 6 | The arm's value is assigned to `ws_msg` and sent by one shared `sender.send` — so forwarding is a one-line change, not a restructure | `ws.rs:1363-1366` |
| 7 | The `aborted` frame is `{"type":"aborted"}` only — **no token totals** | `ws.rs:1445` |
| 8 | **One turn persists exactly one assistant message**, regardless of step count | `history_append.rs:15` |
| 9 | Proof of #8 across the whole deployment: every one of the 14 sessions in the snapshot has `users == assistants` exactly, and **zero `role='tool'` rows** | OCI snapshot (§11) |
| 10 | **Tool calls are not persisted — but the data exists upstream.** The runtime's durable history *does* carry them: `replay_loop_messages` converts assistant tool rounds into `AssistantToolCalls { text, tool_calls, reasoning_content }` and `ToolResults` | `agent.rs:2385-2428` |
| 11 | **Root cause: the gateway's persist filter drops them.** `persist_conversation_messages` destructures `ConversationMessage::Chat` and `continue`s on every other variant, silently discarding `AssistantToolCalls` and `ToolResults` | `ws.rs:919-925` |
| 12 | The web client **already has parsers** for a native `{content, reasoning_content, tool_calls}` assistant row and for `role='tool'` rows — code that can never fire on today's data | `chatHistoryStorage.logic.ts:50-115` |
| 13 | So steps **cannot** be derived from the transcript, and neither can the tool-call sequence | #8–#12 |
| 14 | On `history_trimmed` the client **re-fetches and replaces the whole message list** | `AgentContext.tsx:606-616` |
| 15 | A trim retains `keep_recent_turns` whole turns (default 5, clamped 1..=10) | `scattered_types.rs:170-176` |
| 16 | `agent_start` is emitted once per turn, after the turn begins — not on the client's send | `ws.rs:1083` |
| 17 | Tool calls **are** streamed live, in order: `thinking`, `chunk`, `tool_call`, `tool_result` frames, rendered today as one ungrouped `ToolCallCard` per call | `ws.rs:1313-1324`; `AgentChat.tsx:1066` |

Facts 8–13 exist to justify what the plan **refuses** to do; they are not load-bearing for the
implementation. Fact 13 is also why the reference's collapsible tool-call group
(`7 次工具调用 · 6 条消息`) is out of scope: it is reproducible **live** from fact 17, but on reload the
tool calls are gone, so it cannot be a durable view of the conversation.

---

## 4. Metric definitions — the live session window

Every number below is a running total over the current page's lifetime for the current session.

| Display | Definition | Source |
|---|---|---|
| 轮数 | Turns **completed in this window** | `agent_start` frames received |
| 步数 | LLM calls **in this window** | `usage` frames received (new frame, §5.1) |
| 总 token | `Σ (input_tokens + output_tokens)` over those frames | `usage` frame |
| 缓存命中率 | `Σ cached_input_tokens ÷ Σ input_tokens` | `usage` frame |
| 上下文占用 | `last_input_tokens ÷ max_context_tokens` | existing `done` frame |

Notes:

- **`agent_start` is the turn boundary, not the user's send.** A send that fails before the turn
  starts should not count as a turn (fact 16).
- **`cached_input_tokens` is a subset of `input_tokens`**, never additive — the rate is
  `cached ÷ input`, not `cached ÷ total`. The field's own doc comment says so
  (`zeroclaw-api/src/agent.rs:118-123`).
- **A missing `cached_input_tokens` counts as 0, not as "unknown".** Absence is normal (provider
  silent, flag off). The rate is therefore a **lower bound**; acceptable for a live counter, and it
  must not be presented as exact.
- **总 token counts a multi-step turn's growing prompt once per step.** Summed over a session this is
  much larger than "how big is this conversation". The reference's `135K tok` reads the same way.
  The ring, not this field, answers the size question.
- **No cost segment.** See §2.
- `input_tokens: None` means "unavailable for this call", not zero. Count the step, add 0 to the
  token sums, and let the ratio's denominator decide whether a rate can be shown at all.

### 4.1 What resets the counters, and why

The counters are plain component state. They reset to `0 轮 0 步` on:

| Trigger | Mechanism | Code needed |
|---|---|---|
| First load of a session | Component mounts | none |
| Page refresh | JS state is gone | none |
| Switching conversation and back | State is cleared on session switch | none |
| New session | Same as switching | none |
| **Context trim** | **Not automatic** — see below | **explicit reset** |

**Trim is the one case that needs code.** A trim rewrites the *persisted* transcript on the server
(`replace_messages`) and the client re-fetches it (fact 11), but a trim does not touch the browser's
counters — server-side state and JS state are independent. So the `history_trimmed` handler must zero
the counters explicitly, alongside the `tokens_after` it already applies (`AgentContext.tsx:596-597`).

This is why the row is coherent: turns and steps always share the same window. Deriving turns from
the loaded transcript instead would be free and historical — but it would make turns survive a
refresh while steps could not, and the row would read `5 轮 0 步` (D1).

**No staleness warning is needed.** The numbers are visibly small after a refresh; a `0 轮 0 步` row
on a conversation with history is self-evidently "since you opened this page". The `本次` prefix in
the label is the whole explanation (§7.4).

---

## 5. Wire contract — three frame changes, no new endpoint

### 5.1 New frame: `usage`

Forward the per-step usage the gateway already receives and currently drops (fact 5).

```json
{ "type": "usage", "input_tokens": 17214, "output_tokens": 32, "cached_input_tokens": 1536 }
```

The three fields are nullable and passed through as-is. Absent contributes 0 to the sums; the client
distinguishes absent from zero only in that it never claims an exact cache rate (§4).

**This frame is also the step boundary for the message-flow plan**
(`webui-message-flow-plan.md`): `Usage` is emitted once the response is accepted, *before* that step's
tool-call events, so it is the only server-authoritative step delimiter the client can get. That plan
depends on this one landing.

**Why forward rather than count server-side and send a total.** The client needs a per-step signal to
count steps; the tokens come along for free in the same event. It also makes the row advance *during*
a turn rather than only at its end, which is what the reference does. The cost is one small frame per
step — an 860-step conversation emits 860 of them over its whole life, at roughly 80 bytes each.

### 5.2 `done` — add `steps` and `cached_input_tokens`

```json
{ "type": "done", …, "steps": 7, "cached_input_tokens": 22621 }
```

`steps` is the gateway's own count of `Usage` events for the turn. It is **not** how the client counts
steps — the client counts frames. It exists so the client can reconcile the turn in progress: if the
page reconnects mid-turn, the client has missed that turn's earlier `usage` frames and its own count
is low. On `done` the client takes the frame's `steps` for the turn it just finished.
`cached_input_tokens` is added for the same reason.

### 5.3 `aborted` — add the same fields

`ws.rs:1445` currently sends `{"type":"aborted"}` and nothing else, so a user-cancelled turn silently
vanishes from the row (fact 7). The accumulator variables are in scope at that point (declared at
`ws.rs:1152-1158`, same function), so the fix is to send the same fields as `done`. This resolves
plan v1's open question Q6.

### 5.4 Unchanged

`history_trimmed` keeps its current shape; the client only gains the counter reset (§4.1). The ring's
inputs are unchanged: `max_context_tokens` / `last_input_tokens` on `done`, `tokens_after` on
`history_trimmed`.

---

## 6. Backend changes

All in `crates/zeroclaw-gateway/src/ws.rs`. Nothing else in the workspace changes.

1. Two accumulators beside the existing `total_input_tokens` / `total_output_tokens`
   (`ws.rs:1152-1158`): `total_cached_input_tokens`, `steps`.
2. In the `TurnEvent::Usage` arm (`ws.rs:1295-1310`):
   - bind `cached_input_tokens` instead of `cached_input_tokens: _`;
   - add it to the accumulator and `steps += 1`;
   - replace `continue;` with the `usage` frame from §5.1 — the arm already assigns to `ws_msg` and
     a shared `sender.send` follows (fact 6), so this is a value change, not a restructure.
3. `done` frame (`ws.rs:1550-1559`): add `steps` and `cached_input_tokens`.
4. `aborted` frame (`ws.rs:1445`): add `input_tokens`, `output_tokens`, `cached_input_tokens`,
   `steps`, using the same values.

Unit tests: one `Usage` event increments `steps` and the cache accumulator; a `Usage` with all-`None`
fields still increments `steps` and leaves the sums unchanged; `done` and `aborted` carry the
accumulated values.

**No change** to `zeroclaw-config`, `zeroclaw-infra`, `zeroclaw-runtime`, the API surface, the
session schema, or the cost ledger.

---

## 7. Frontend changes

### 7.1 `web/src/lib/ws.ts`
Add `usage` to the inbound frame union. No behaviour.

### 7.2 `web/src/pages/sessionStats.logic.ts` (new)
The whole metric layer, pure and unit-tested — no React, no fetching:

```ts
export interface LiveStats { turns: number; steps: number; input: number; output: number; cached: number }
export const emptyStats: LiveStats
export function applyUsage(s: LiveStats, frame): LiveStats   // steps + 1, sums += fields
export function applyDone(s: LiveStats, frame): LiveStats    // reconcile steps/cached for the turn
export function cacheHitRatio(s: LiveStats): number | null   // null when input === 0
export function formatTokens(n: number): string              // 135K, 27.5M
```

Keeping this out of the component is what makes acceptance 6–9 testable without a browser, and it
matches the repo's existing `.logic.ts` convention.

### 7.3 `web/src/contexts/AgentContext.tsx`
- New state `liveStats: LiveStats`, initialised to `emptyStats`; cleared on session switch.
- `agent_start` → `turns + 1`.
- `usage` → `applyUsage`.
- `done` / `aborted` → `applyDone` (§5.2, §5.3), then keep the existing `max_context_tokens` /
  `last_input_tokens` handling at `:464-475` untouched.
- `history_trimmed` → reset to `emptyStats`, next to the existing `tokens_after` assignment
  (`:596-597`). This is the only reset that is not free (§4.1).
- **No fetch is added anywhere.** Acceptance 5 asserts it.

### 7.4 `web/src/pages/AgentChat.tsx`
Target layout:

```
┌────────────────────────────────────────────────┐
│  [ textarea ]                                  │
│  (+)   …                          (ring) (send)│
└────────────────────────────────────────────────┘
        本次 2 轮 10 步 · 135K tok · 缓存命中 83%
```

- Input row: `+` button on the left — the existing image control restyled to a `+` glyph, same
  `accept="image/*"` intake, same `aria-label` (D3). Ring + send/stop on the right, ring immediately
  left of send (D2).
- Remove `<ContextBar>` from the footer row (`AgentChat.tsx:972-975`); keep the connection status dot.
- Stats row under the input. It is always present once a session is active — `本次 0 轮 0 步` is a
  legitimate state, not an error, and hiding it would make the row appear and disappear.
- **`本次` prefix is load-bearing**, not decoration: it is the only thing that explains why the
  numbers are small on a conversation that visibly has history.
- 步数 follows 轮数; the v1 argument about leading with steps is moot now that both are live.
- Segment rules: hide `缓存命中` when `cacheHitRatio` is `null`; never render `NaN` / `Infinity`.
- Untouched: the message list, the header toolbar, the model picker, `MessageItem`.

### 7.5 New files
| File | Role |
|---|---|
| `web/src/components/ContextRing.tsx` | SVG ring, props `{ used, max }`, no data fetching |
| `web/src/components/SessionStatsRow.tsx` | Pure presentational row over `LiveStats` |
| `web/src/pages/sessionStats.logic.ts` | §7.2 |
| `web/src/pages/sessionStats.logic.test.ts` | Tests |

### 7.6 i18n
`web/src/lib/i18n.ts`, `zh` + `en` only (other locales fall back to English), following the existing
flat `'agent.xxx'` convention with `{placeholder}` interpolation:

`agent.stats.session` (`本次` / `this session`), `agent.stats.turns`, `agent.stats.steps`,
`agent.stats.tokens`, `agent.stats.cache_hit`, `agent.context.ring_label`,
`agent.context.ring_detail_used` / `_max` / `_percent`.

---

## 8. Acceptance criteria

1. Opening a session shows `本次 0 轮 0 步`; sending a message makes the row advance **during** the
   turn, not only at its end.
2. After N turns the row equals what the frames say: turns = `agent_start` count, steps = `usage`
   count, tokens = Σ(input+output), rate = Σcached ÷ Σinput.
3. **Trim resets the row to `本次 0 轮 0 步`** (§4.1). Verified by forcing a trim
   (`trim_threshold_percent` lowered) and observing the reset alongside the existing trim notice.
4. **Refresh resets the row** to `0 轮 0 步` on a conversation that visibly has history — the
   accepted trade, asserted rather than tolerated.
5. **Zero network requests are added.** A test harness counts calls to `/api/sessions/*`; opening a
   session and completing a turn must add none beyond today's baseline.
6. A cancelled turn still advances the row by that turn's steps and tokens (`aborted` carries them).
7. A step whose `Usage` fields are all `None` still increments 步数 and leaves the token sums
   unchanged; no `NaN` appears in the row.
8. `缓存命中` is hidden when the input sum is 0, and never renders `NaN` / `Infinity` / `100%+`.
9. The ring reflects current context fill and opens a panel with used / max / percent. It renders
   correctly on a freshly loaded conversation, before any `done` frame, using `last_input_tokens`
   if the session store supplies it — and degrades to an empty ring if it does not.
10. **No `$` appears anywhere in the row** (§2).
11. Only `zh` and `en` gain strings; the other 29 locales fall back to English.
12. No regressions: `npm test`, `test:contexts`, `test:permissions`, `test:navigation`,
    `npm run typecheck`, `npm run build` stay green; `cargo test -p zeroclaw-gateway --lib --
    --test-threads=1` stays green (the crate's tests bind fixed ports and must not run in parallel).

---

## 9. Decisions log

**D0 — `conversation_id` seam: no longer on the critical path.** Verified working against live OCI
data (1,600 of 7,179 ledger rows carry it, format `gw_<session_id>`, from 2026-09-11). Recorded only
because it was investigated; this plan reads no ledger.

**D1 — RESOLVED: option A, all-live counters.** Turns and steps share one window and both reset
together. The alternative — derive 轮数 from the loaded transcript (free, historical, and equal to
the trimmer's own predicate) while 步数 stays live — was rejected because it mixes windows: steps
cannot be derived from the transcript (facts 8–13), so after a refresh the row would read
`5 轮 0 步`, which looks broken rather than deliberate. **Option A supersedes plan v1's Q1
("保留轮数"), which is hereby withdrawn**: 轮数 now means "turns in this session", not "turns retained
in the transcript".

**D2 — RESOLVED: ring at the input's bottom-right** (reference layout), left of the send button,
replacing the footer `ContextBar`.

**D3 — RESOLVED: icon-only `+`.** Restyle the existing image-upload control to a `+` glyph; intake
stays image-only. A File / Commands menu is a separate feature needing the non-image upload path.

**D4 — RESOLVED: 总 token = cumulative billed tokens,** summed from the `usage` frames, matching the
reference's `135K tok` reading. Not "size of the last prompt" — that is the ring.

**D5 — RESOLVED: the `[Tool results]` / breadcrumb rendering defect is fixed here.**
`mapServerMessagesToPersisted` (`web/src/lib/chatHistoryStorage.logic.ts:130-140`) maps every
`role == "user"` row to a user bubble with no filter, so on reload a prompt-mode conversation shows
each tool round as if the operator had typed it, and a trim breadcrumb shows as a user bubble. Two
lines, using the same predicate as the trimmer. This is a rendering defect, not a data problem.

**D6 — RESOLVED: forward a per-step `usage` frame** rather than only adding a `steps` field to `done`
(§5.1). The frame is needed anyway for live token accumulation, and it makes the row advance during
a turn.

---

## 10. Implementation order

1. `ws.rs`: accumulate `steps` + `cached_input_tokens`, forward the `usage` frame, extend `done` and
   `aborted`. Unit tests (§6). **This is the whole backend.**
2. `web/src/lib/ws.ts`: add `usage` to the frame union.
3. `sessionStats.logic.ts` + tests (§7.2) — pure functions first, so the metric rules are pinned
   before any UI exists.
4. `AgentContext.tsx`: wire `agent_start` / `usage` / `done` / `aborted` / `history_trimmed` (§7.3).
5. `ContextRing.tsx`, `SessionStatsRow.tsx`, composer relayout, `+` icon (§7.4, §7.5).
6. i18n `zh` + `en` (§7.6); fix the `[Tool results]` / breadcrumb bubble mapping (D5).
7. Full test sweep; manual check of the three reset paths — refresh, trim, session switch.

---

## 11. Rejected design — the ledger-backed row, and why

Recorded so the measurements are not redone and the decision is not re-litigated. Plan v1 proposed
`GET /api/sessions/{id}/usage`, computing turns by SQL aggregate and steps/tokens/cache from
`state/costs.jsonl`. It was dropped when the requirement became "live session only".

**Measured** (2026-09-15, real OCI ledger: 7,179 rows / 2.3 MB / 322 bytes per row, release build,
warm page cache, offline bench reproducing `for_each_record` line for line):

| Variant | Today (7.2K rows) | Per row |
|---|---|---|
| Full deserialize — what `for_each_record` does | **4.1 ms** (p90 4.7) | 580 ns |
| Same + `contains(conversation_id)` pre-filter | **2.1 ms** (p90 2.6) | 320 ns |
| `read_to_string` alone | 0.9–1.4 ms | — |
| Split into lines, no parse | 0.14 ms | 20 ns |

Strictly linear (7.2K→2.9 ms, 72K→28.7 ms, 215K→87 ms; 401 ns/row, flat across 30×). At the current
~240 rows/day and ~322 bytes/row the file grows ~28 MB/year, so a scan costs ~55 ms at +1 year,
~157 ms at +3, ~258 ms at +5.

Three facts that shaped v1, still true and still worth knowing:

1. **An endpoint that already ships is strictly more expensive.** `GET /api/cost` (called on every
   dashboard load, `Dashboard.tsx:1774`) runs the same full scan and then materializes a
   `Vec<CostRecord>` of every matching row plus `by_model` / `by_agent` rollups
   (`tracker.rs:336-361`, `:1023-1040`). An aggregate-only scan is cheaper than what production
   already does.
2. **The real hazard was lock contention, not latency.** Every tracker read goes through
   `lock_storage()` (`tracker.rs:82-84`), the same mutex the append path takes (`tracker.rs:290`),
   and each append also pays a `File::sync_all`. Any future ledger read should bypass `CostTracker`
   and read the file directly inside `spawn_blocking`.
3. **Nothing prunes the ledger** — no rotation, no compaction, no size cap in production code.

**Tripwire, if historical counts are ever wanted:** past ~200K rows / ~64 MB, or an endpoint p95
above ~50 ms, the cheapest fix is an in-process per-conversation cache warmed by one scan — not a
storage redesign.

**Test fixtures.** A read-only snapshot of the OCI deployment was taken 2026-09-15 and lives at
`~/.workbuddy-ai/oci-fixtures/` (`costs.jsonl` 7,179 rows, `sessions.db` 230 rows / 14 sessions).
**Not needed by this plan** — kept only for the §11 analysis. They live outside the repository on
purpose: they contain real conversation content, so anything committed must be synthesised, never
copied.
