# Overall plan — dsh-style conversation UI + composer telemetry (handoff)

**For:** an agent picking this up with no prior session context.
**Status:** approved for execution. Phases P1–P4 may be implemented.
**Companion documents (read them before starting a phase):**
- `docs/reports/webui-composer-parity-plan.md` — composer detail (§4–§8 are the spec for P1)
- `docs/reports/webui-message-flow-plan.md` — message-flow detail (§4–§6 are the spec for P2)

This document is the index: order, dependencies, shared context, traps. The two companions carry the
per-change detail and the acceptance criteria. **Do not re-derive the facts in §5** — they were
verified against the source tree and against a production snapshot, with `file:line` evidence.

---

## 1. Repository state at handoff

| | |
|---|---|
| Branch | `zerolite` |
| HEAD | `cde6023a7 fix(web): keep chat transcript pinned to tail without hijacking scroll` |
| Working tree | two untracked plan documents, **no source changes** |
| `git status --short` | `?? docs/reports/webui-composer-parity-plan.md`<br>`?? docs/reports/webui-message-flow-plan.md` |

Suggested first commit: the two plan documents on their own, so the handoff is in history before any
code lands. Commit style follows HEAD: `docs(web): …`.

---

## 2. Goal

Port DeepSeek Harness's conversation presentation into ZeroClaw's web chat, in two visible pieces:

1. **Composer** — `+` upload at the input's bottom-left, circular context ring at the bottom-right
   (click opens a detail panel), and a stats strip in the toolbar between them:
   `2 轮 10 步 · 135K tok · 缓存命中 83%`. The always-visible linear `ContextBar` is removed.
   **Canonical field definitions:** §10 "Composer stats semantics".
2. **Message flow** — a turn renders as ordered **step segments** (thinking → text → tool calls per
   LLM call), with the non-final steps collapsed under a header reading
   `7 次工具调用 · 6 条消息` that expands into the full trajectory, and the final answer left expanded.

Both are **live-only**: the numbers and the trajectory are built from the WebSocket stream the client
already receives, and are lost on refresh. This is a deliberate, approved trade that removes the
entire server-side cost of the feature.

---

## 3. Hard constraints — do not violate

1. **No durability work.** Tool calls are not persisted today (fact 10–12). Fixing that is **option D**,
   explicitly out of scope (§11). Do **not** touch `persist_conversation_messages`'s filter, the
   session schema, or anything that changes what the model sees.
2. **No new endpoint, no database query, no ledger read, no new Rust types.** The only backend change
   in this entire plan is the `usage` frame in P0.
3. **Live-only semantics are the requirement, not a limitation.** `0 轮 0 步` on open is correct
   behaviour. Do not "fix" it by fetching history.
4. **i18n: `zh` + `en` only.** The other 29 locales fall back to English.
5. **Repository artifacts in English; discussion in Chinese** (`AGENTS.md` rule 10).
6. **Do not change `classifyCompletion`'s behaviour** (P2). Its reasoning-only / empty / tool-only
   classification is covered by existing tests; they must stay green.

---

## 4. Test environment (this machine, Windows)

Read this before running anything.

**Cargo — `zeroclaw-gateway` must run single-threaded.** Its tests bind fixed ports and collide when
run in parallel (`Os { code: 10048, kind: AddrInUse }`).

```bash
env -u http_proxy -u https_proxy -u HTTP_PROXY -u HTTPS_PROXY \
  cargo test -p zeroclaw-gateway --lib -- --test-threads=1
```

**The proxy variables must be cleared** for any test touching `localhost`/wiremock — the sandbox
injects `http_proxy=127.0.0.1:54419` and requests silently go through it instead of the mock.
`--test-threads=1` is **gateway-only**; other crates run in parallel and are ~8× slower serialized.

**npm — new test files must be registered.** `npm test` runs only the files listed in
`web/package.json` scripts. A new `*.logic.test.ts` that is not added to a script **will never run**.
`test:contexts` is the natural home for the two new pure-logic test files, and it also runs
automatically as `pretest`. Note it already contains `turnStream.logic.test.ts` and
`chatHistoryStorage.logic.test.ts` — the two files P2 and P3 modify.

```bash
cd web && npm run test:contexts     # turnStream + chatHistoryStorage + modelPicker + stripServerTimestamp
cd web && npm test                  # broader suite; runs test:contexts via pretest
cd web && npm run typecheck         # check:generated + tsc -b
cd web && npm run build             # check:generated + tsc -b + vite build
```

---

## 5. Verified facts — the foundation

Do not re-derive these. Evidence is `file:line` in the tree at HEAD `cde6023a7`.

### 5.1 Backend / wire

| # | Fact | Evidence |
|---|---|---|
| 1 | The WS `done` frame already carries `input_tokens`, `output_tokens`, `tokens_used`, `last_input_tokens`, `max_context_tokens`, `cost_usd` | `crates/zeroclaw-gateway/src/ws.rs:1550-1559` |
| 2 | The client already reads `max_context_tokens` and `last_input_tokens` from it | `web/src/contexts/AgentContext.tsx:464-475` |
| 3 | `TurnEvent::Usage` is emitted **once per LLM call** — i.e. once per step | `crates/zeroclaw-runtime/src/agent/turn/parse_response.rs:304` |
| 4 | `Usage` carries `input_tokens`, `cached_input_tokens`, `output_tokens`, `cost_usd`, all `Option`; **no** `total_tokens` | `crates/zeroclaw-api/src/agent.rs:115-128` |
| 5 | **The gateway discards `Usage` and never forwards it** — the match arm accumulates then `continue`s | `ws.rs:1295-1310` |
| 6 | That arm assigns to `ws_msg`, sent by one shared `sender.send` — forwarding is a value change, not a restructure | `ws.rs:1363-1366` |
| 7 | The `aborted` frame is `{"type":"aborted"}` only — no token totals | `ws.rs:1445` |
| 8 | **Per-step event order is `thinking` → `chunk` → `Usage` → `ToolCall`/`ToolResult`.** `Usage` fires once the response is accepted; tool-call events fire later, during execution | `parse_response.rs:267-311`; `crates/zeroclaw-runtime/src/agent/turn/call_prep.rs:207,303` |
| 9 | **So the `usage` frame is also the only server-authoritative step boundary.** This is the load-bearing fact for P2 | #8 |
| 10 | `chunk_reset` exists **only** in the web client and its test — **no Rust code emits it**. The client's model of it is dead | repo-wide search |
| 11 | `agent_start` is emitted once per turn, after the turn begins — not on the client's send | `ws.rs:1083` |

### 5.2 Persistence — why this is live-only

| # | Fact | Evidence |
|---|---|---|
| 12 | One turn persists exactly one assistant message, regardless of step count | `crates/zeroclaw-runtime/src/agent/turn/history_append.rs:15` |
| 13 | Verified across the whole deployment: all 14 sessions have `users == assistants` exactly and **zero `role='tool'` rows** | OCI snapshot, `~/.workbuddy-ai/oci-fixtures/sessions.db` |
| 14 | The runtime's durable history **does** carry tool rounds — `replay_loop_messages` converts them to `AssistantToolCalls` + `ToolResults` | `crates/zeroclaw-runtime/src/agent/agent.rs:2385-2428` |
| 15 | **Root cause they are missing: the gateway's persist filter drops them.** `persist_conversation_messages` destructures `ConversationMessage::Chat` and `continue`s on every other variant | `ws.rs:919-925` |
| 16 | The web client **already has parsers** for the native `{content, reasoning_content, tool_calls}` assistant row and for `role='tool'` rows — code that can never fire on today's data | `web/src/lib/chatHistoryStorage.logic.ts:50-115` |
| 17 | `jsonl_import_receipts` is empty — the missing rows are native write behaviour, not an import artifact | OCI snapshot |

### 5.3 Frontend

| # | Fact | Evidence |
|---|---|---|
| 18 | `TurnStreamState` is **flat**: `pendingContent` / `pendingThinking` concatenate the whole turn, with no per-step structure | `web/src/contexts/turnStream.logic.ts:7-16` |
| 19 | Tool cards are pushed into `messages` as separate agent messages, in arrival order | `web/src/contexts/AgentContext.tsx:482-525` |
| 20 | Text and thinking stream into **one growing bubble**, rendered after the message list | `web/src/pages/AgentChat.tsx:825-849` |
| 21 | **Therefore today's live view does not preserve interleaving** — a 7-step turn reads as "one blob of text, then 7 cards". This is what P2 fixes | #18–#20 |
| 22 | `ToolCallCard` already renders icon + name + args + output + executing spinner, correlating by `tool_call_id` | `web/src/components/ToolCallCard.tsx` |
| 23 | The collapse pattern already exists — thinking renders inside a `<details>` | `AgentChat.tsx:1061-1066`, `:834-836` |
| 24 | `ContextBar` is defined at `AgentChat.tsx:55` and used at `:974` | grep |
| 25 | On `history_trimmed` the client **cuts the transcript locally** from `kept_turns` — a store read here returns the pre-trim transcript. See §10 | `AgentContext.tsx:93` (`purgeUiMessagesToKeptUserTurns`), `:645` |
| 26 | A trim retains `keep_recent_turns` whole turns (default 5, clamped 1..=10) | `crates/zeroclaw-config/src/scattered_types.rs:170-176` |

---

## 6. Decisions already made — do not re-litigate

**Composer (from `webui-composer-parity-plan.md` §9):**

- **C-D1** All-live counters. Turns and steps share one window and reset together.
  `轮数` means "turns in this session", **not** "turns retained in the transcript".
- **C-D2** Ring at the input's bottom-right, left of send.
- **C-D3** `+` is icon-only; intake stays image-only (`accept="image/*"`).
- **C-D4** `总 token` = cumulative billed tokens summed from `usage` frames.
- **C-D5** Fix the `[Tool results]` / breadcrumb bubble mapping **here** (P3).
- **C-D6** Forward a per-step `usage` frame rather than only adding `steps` to `done`.
- **C-D7** No cost segment anywhere in the row. Production models are unpriced.
- **C-D8** The `本次` prefix in the stats row is load-bearing — it is the only explanation for why the
  numbers are small on a conversation that visibly has history. Do not remove it.

**Message flow (from `webui-message-flow-plan.md` §7):**

- **L-D1** Collapse default: expanded while streaming, collapse on `done` unless the user toggled it.
- **L-D2** `M 条消息` = the number of steps carrying thinking or text. The reference does not document
  this; if it turns out to mean something else, only one function changes.
- **L-D3 / Q1** The group wraps only the prefix **before** the final answer. The final answer never
  goes inside the group.

---

## 7. Order and dependencies

```
P0  ws.rs — the usage frame                        (backend, ~15 lines, no deps)
 │
 ├──────────────┬──────────────┐
 ▼              ▼              │
P1 composer    P2 message flow  │   both depend on P0's usage frame
 │              │              │
 └──────┬───────┘              │
        ▼                      │
       P3 defect fix + i18n    │
        ▼                      │
       P4 verification  ◄──────┘
```

**P1 and P2 both edit `AgentContext.tsx` and `AgentChat.tsx`. Run them sequentially, never in
parallel** — P1 first (smaller, already reviewed in detail), then P2. Do not open two worktrees on
the same files.

**P0 must land before P1 and P2.** P2 has no fallback: the only alternative to the `usage` frame is
inferring boundaries client-side from `tool_call` frames, which fact 8 makes unreliable for tool-free
steps and for parallel calls.

---

## 8. Phase detail

### P0 — the `usage` frame (backend, ~15 lines)

**File:** `crates/zeroclaw-gateway/src/ws.rs` only. Nothing else in the workspace changes.

1. Two accumulators beside the existing `total_input_tokens` / `total_output_tokens`
   (`:1152-1158`): `total_cached_input_tokens`, `steps`.
2. In the `TurnEvent::Usage` arm (`:1295-1310`): bind `cached_input_tokens` instead of
   `cached_input_tokens: _`; accumulate it and `steps += 1`; replace `continue;` with the new frame:
   ```json
   { "type": "usage", "input_tokens": 17214, "output_tokens": 32, "cached_input_tokens": 1536 }
   ```
   Fields are nullable and passed through as-is.
3. `done` frame (`:1550-1559`): add `steps` and `cached_input_tokens`.
4. `aborted` frame (`:1445`): add `input_tokens`, `output_tokens`, `cached_input_tokens`, `steps`.
   The accumulator variables are in scope (declared at `:1152-1158`, same function).

**Tests:** one `Usage` event increments `steps` and the cache accumulator; a `Usage` with all-`None`
fields still increments `steps` and leaves the sums unchanged; `done` and `aborted` carry the values.

**Acceptance:** `cargo test -p zeroclaw-gateway --lib -- --test-threads=1` green.

**Why `steps` goes on `done` at all:** not for counting (the client counts frames) but for
reconciliation — if the page reconnects mid-turn it has missed that turn's earlier `usage` frames,
and takes the frame's authoritative value for the turn it just finished.

### P1 — composer

**Files:** `web/src/lib/ws.ts` (add `usage` to the frame union), new
`web/src/pages/sessionStats.logic.ts` + test, new `web/src/components/ContextRing.tsx`,
new `web/src/components/SessionStatsRow.tsx`, `web/src/contexts/AgentContext.tsx`,
`web/src/pages/AgentChat.tsx`, `web/src/lib/i18n.ts`.

Full spec: `webui-composer-parity-plan.md` §4, §7, §8. Read §4.1 (reset triggers) carefully — **trim
is the only reset that needs code**: a trim rewrites the server-side transcript without touching JS
counters, so `history_trimmed` must zero them explicitly, next to the existing `tokens_after`
assignment at `AgentContext.tsx:659-660` (fact 25, §10).

Order inside P1: pure logic + tests first → `AgentContext` wiring → components → i18n.

**Register the new test file in `web/package.json`** (see §4) or it will never run.

### P2 — message flow

**Files:** `web/src/contexts/turnStream.logic.ts` (extend), new `web/src/pages/messageFlow.logic.ts`
+ test, new `web/src/components/ToolCallGroup.tsx`, `web/src/pages/AgentChat.tsx`,
`web/src/contexts/AgentContext.tsx`, `web/src/lib/i18n.ts`.

Full spec: `webui-message-flow-plan.md` §4–§6.

Load-bearing rules:
- A step closes on the `usage` frame (fact 9). `tool_call` frames arriving after it attach to the
  step that just closed — **several calls in one step is parallel tool use, not several steps**.
- `turnStream.logic.ts` is **extended, not rewritten**. `classifyCompletion` must behave identically.
- Hydrated (reloaded) turns keep rendering as plain bubbles — no group, no empty header. Asserted in
  acceptance, not tolerated.

### P3 — defect fix and i18n

- `web/src/lib/chatHistoryStorage.logic.ts:130-140` — `mapServerMessagesToPersisted` maps every
  `role == "user"` row to a user bubble with no filter, so on reload a prompt-mode conversation shows
  each tool round as if the operator typed it, and a trim breadcrumb shows as a user bubble. Two
  lines, using the same predicate as the trimmer (`role == 'user'` and
  `!content.startsWith('[Tool results]')`, plus the breadcrumb string from i18n).
  `chatHistoryStorage.logic.test.ts` exists and must be updated.
- All new i18n keys, `zh` + `en` only.

### P4 — verification

Run everything in §4, then the manual checks:

1. Open a conversation → `本次 0 轮 0 步`, no `ContextBar`, ring present.
2. Send a message → the row advances **during** the turn, not only at the end.
3. Run a multi-step turn (7+ tool calls) → one collapsed `N 次工具调用 · M 条消息` row with the final
   answer below; expanding shows thinking/text/tools in arrival order.
4. Cancel mid-turn → the row still advances by that turn's steps and tokens.
5. Force a trim (lower `trim_threshold_percent`) → the stats row resets to `0 轮 0 步`.
6. Refresh on a conversation with history → the row resets and the trajectory is gone. **This is
   correct.** Do not treat it as a bug.
7. Switch conversations and back → both reset.

---

## 9. Traps

1. **`npm test` will not run a new test file** unless it is added to `web/package.json` scripts.
2. **`--test-threads=1` is gateway-only.** Applying it elsewhere makes the suite ~8× slower.
3. **Clear the proxy variables** before any test that touches localhost, or mocks silently receive
   zero requests and the failure looks like a 502.
4. **`chunk_reset` is a dead frame** (fact 10). Do not build logic that waits for it.
5. **P1 and P2 collide** in `AgentContext.tsx` and `AgentChat.tsx`. Sequential only.
6. **`cached_input_tokens` is a subset of `input_tokens`** — the rate is `cached ÷ input`, never
   `cached ÷ total`. A missing value counts as 0, making the rate a lower bound; never present it as
   exact.
7. **`total_tokens` does not exist on the `Usage` event** (fact 4) — compute `input + output`.
8. **Do not render `$` anywhere in the stats row** (C-D7).
9. **Do not "fix" the reset behaviour.** Live-only is the requirement.
10. `agent_start` (fact 11) is the turn boundary — **not** the client's send. A send that fails before
    the turn starts must not count as a turn.

---

## 10. Definition of done

Executed on branch `zerolite`: `b5cfe173f` (P0) → `af9fbb652` (P1) → `2acbcc93f` (P2) →
`1deef768d` (P3) → `aa71d2a32`, `8f2ca33ba` (P4) → `483cafa0f` (the SSE pin under deviation 1).

- [x] P0 landed; gateway tests green single-threaded — **486 passed**, including the end-to-end
      WebSocket test that drives a real upgrade against an Anthropic SSE fixture.
- [x] P1 landed; `npm run test:contexts`, `npm test`, `npm run typecheck`, `npm run build` green
- [x] P2 landed; same suite green; existing `turnStream.logic.test.ts` cases unchanged (12 → 20)
- [x] P3 landed; `chatHistoryStorage.logic.test.ts` updated
- [x] All new test files registered in `web/package.json`
- [x] The 7 manual checks in P4 pass — 6 are covered by the 28 headless integration cases in
      `sessionLifecycle.test.ts` (ring present / linear bar gone, row advances mid-turn, cancel,
      trim, conversation switch, refresh renders ungrouped, expansion order). The 7th — a real
      7+ tool-call turn — was **not** run: it needs a live daemon and a model. Checks 3 and 6 are
      covered in substance, not at that scale.
- [x] Only `zh` + `en` gained strings — all 12 new keys appear exactly twice
- [x] `git status` clean apart from intended changes

Found while turning the acceptance criteria into assertions, and fixed:

- **The ring's panel rendered `25%% used`** (`3c0c0a79e`). The templates carry their own `%`
  (`{percent}% used` / `占用 {percent}%`) but the call site passed a label that already ended in one.
  Only check 1's panel was ever asserted, and only in its empty state, so it survived P1. Both
  `{percent}` call sites now pass the bare number.
- **Acceptance 5 / message-flow 8 had nothing behind it** (`9380e5d7c`). The harness counted
  `/api/config/prop` writes but nothing on `/api/sessions*`, so "zero new network requests" was a
  claim about the implementation rather than a measurement of it.
- **Acceptance 2 and 4 were asserted at the wrong scale.** The row's identity (`turns` = the
  `agent_start` count, `steps` = the `usage` count, and so on) was only checked for one turn, and
  "refresh resets the row" was only checked on an *empty* conversation. Both now run at the scale
  the criteria name: three turns across the identity, and a conversation that visibly has history
  for the reset.

Found in review, after the fact — both were claims the code did not keep (`f9ba09f4f`):

- **The step boundary was not driven by `usage`.** `AgentContext`'s `usage` case only fed the stats
  row, so the boundary actually came from `attachToolCall`'s internal `closeStep` — the fallback
  §8.5 calls unreliable. A turn whose intermediate step made no tool calls collapsed into one step,
  welding the intermediate text onto the answer and leaving no trajectory to group. The `tool_call`
  comment asserted the opposite, and the integration tests were green either way because a tool call
  closes the step as a side effect.
- **The streaming bubble was never replaced.** §5.4 and D1 ask for the trajectory to render while
  the turn runs. The transcript kept a flat `streamingContent` / `streamingThinking` bubble and only
  grouped after `done`, so D1's "expanded while streaming" never applied. Both buffers are gone now
  that nothing reads them.

Two deviations from §3, recorded here rather than left implicit (three and four follow in the second
review round below):

1. **`agent_start` gained `session_id`** (one line, `ws.rs`). §5 fact 11 / trap 10 assume the chat
   socket can count turns from that frame, but it was broadcast unscoped, so
   `event_matches_session` → `is_global_chat_event` (which admits only `cron_result`) dropped it and
   the count would have been permanently 0. Chosen over counting `done` + `aborted` client-side.
   Consequence: `is_public_sse_event` now withholds the frame from `/api/events`, which also stops it
   duplicating the observer path's `agent_start` on the public stream. `logs/subscribe` does not
   filter by session and is unaffected. Pinned by
   `sse::tests::the_chat_turn_boundary_is_not_a_public_sse_event`.
2. **`showToolActivity: false` suppresses the group**, not just the loose cards. The group *is* tool
   activity — its header counts calls — and the toggle's default-off is a pre-existing product
   decision ("tool execution is plumbing, not chat"), so it wins over acceptance criterion 1. The
   turn's answer and its captured trajectory both survive, so turning the toggle on reveals the turn
   retroactively. Pinned by two cases in `sessionLifecycle.test.ts`.

### Second review round — single-source cleanup

`ca45965ce` (web), `b6e199e1b` (naming), `2375faa13` (machinery rows), `cf0bb7e37` (token totals).

Every finding was checked against the code before it was acted on; all four held.

- **The turn buffer was written twice.** `chunk` / `thinking` appended to both the flat buffers
  `classifyCompletion` reads and to `openStep`, the step the view renders — the duplication
  `agent-guidelines.md` §"Single Source Of Truth" forbids. They had already drifted: `chunk_reset`
  cleared the flat buffers but left the draft on the open step, so the live view showed text the
  commit had discarded. State is now `segments` + `openStep` + `hadToolCall`, and the classification
  input is derived (`streamedText`, `completionInput`). `capturedThinking` went with the buffers it
  existed to reconcile: a reset drops the open step's *text* and keeps its reasoning, which is what
  the frame means. `chunk_reset` has no emitter (fact 10), so the only observable change is that the
  two stores can no longer disagree — pinned by `a reset draft does not leak into the rendered
  trajectory`, which fails under the old design.
- **Tool-result matching had two implementations.** `findCallIndex` mirrored
  `resolveToolResultIndex`; the nesting was the only difference, and the comment admitted it.
  `toolCardMatch.ts` owns the rule and exposes `resolveToolResultLocation` for the nested shape.
- **The web hand-copied two runtime strings.** `[Tool results]` and the trim breadcrumb decided which
  `user` rows are machinery. The breadcrumb is a Fluent message resolved against the daemon's locale,
  so no copy of it made at build time can be right. The predicate is now
  `history_trim::is_synthetic_user_message`, next to the constants it owns; the gateway reports it
  per row and the client matches nothing. Pinned on both sides.
- **Comments that restated code** — the `ws.rs` borrow note and the `AgentChat` composer layout
  narration — are gone.

From the review's judgment list: `accumulate_usage`'s five `&mut` out-parameters became
`UsageTotals::record`; `fmt` → `formatExactTokens` and `num` → `wireNumber`; and
`messageFlow.logic.ts` / `sessionStats.logic.ts` moved from `pages/` to `lib/`, which is where the
modules a context and a component both import already live (`chatHistoryStorage.logic.ts`,
`toolCatalog.logic.ts`). Left open at the time, because collapsing them needs a shared type in a
pure module and `ToolCallInfo` cannot be it (it lives in a component, and `.logic.ts` modules must
not import one): `SegmentToolCall`, `ToolCallInfo`, `PersistedToolCall` and `FlowMessage.toolCall`
are four declarations of one shape. Type-only, no behaviour at stake. Taken in the third round
below.

Two further deviations from §3:

3. **`/api/sessions/{id}/messages` gained `synthetic`** — a second backend change, one boolean per
   row, additive and optional on the client. Taken over copying the strings because the breadcrumb is
   locale-dependent. Pinned by
   `api::tests::session_messages_flags_runtime_machinery_as_synthetic` and
   `history_trim::tests::synthetic_*`.
4. **`TurnSegments.finalText` / `finalThinking` are gone** (§5 declared them). No production code read
   either: the committed bubble takes its content and reasoning from `classifyCompletion`, and the
   group renders only `steps`. They were a second computation of the same values. `TurnSegments` now
   means exactly "the steps that are not the answer", and the cases that asserted them assert the
   commit they were duplicating.

### Third review round — one shape, one home

`cffd833c5` (web).

Three findings, all three about where a declaration lives rather than what the code does.

- **Both companion plans still opened with "no code has been changed."** They were written for
  approval and never revisited, so a reader landing on either one was told the work had not
  started. Both now say implemented and point here for the commits, the deviations, and the review
  rounds: this document is the live record, and the companions keep the design and the acceptance
  criteria as approved.
- **`lib/` imported `contexts/`.** `messageFlow.logic.ts` took `LiveTurn` / `StepSegment` /
  `TurnSegments` from `contexts/turnStream.logic.ts` — the reverse of the direction every other
  `lib/` module is used in (`chatHistoryStorage.logic.ts` and `toolCardMatch.ts` are imported *by*
  contexts, not the other way round). The three shapes and their constructors (`emptyStep`,
  `emptySegments`, `emptyLiveTurn`) now live in `lib/turnSegments.ts`; the reducer imports them back
  and keeps the state machine it owns (`TurnStreamState`, the frame union, `classifyCompletion`),
  so only the shared vocabulary moved. `ToolCallGroup.tsx` and `AgentChat.tsx` read `TurnSegments`
  from `lib/` now as well.
- **The four tool-call declarations are one.** `SegmentToolCall`, `ToolCallInfo`,
  `PersistedToolCall` and `FlowMessage`'s inline shape are `ToolCall` in `lib/toolCall.ts` — pure,
  no React, no path aliases, which is exactly why `ToolCallInfo` could not have been the source.
  The matcher's input is `Pick<ToolCall, 'output' | 'id'>` rather than a fifth declaration.

Verified: `npm run typecheck`, `npm run test:contexts` (78), `npm test`, `vite build`.

### Fourth review round — the trim frame is not a store read

One commit — `fix(web): cut the transcript locally on history_trimmed instead of re-reading the
store` — carries both the web change and the frame's doc comments, so unlike the earlier rounds
there is no separate hash to cite here. Reported symptom: after a high-water-line trim the
transcript reverted to the raw message flow — loose tool cards and `Thinking` blocks — instead of the folded
`N tool calls · M messages` group, with the trim notice sitting at the point where it happened.

Three findings, all on one handler (`AgentContext`'s `case 'history_trimmed'`).

- **A rebuild destroyed client-only state.** `segments` is deliberately not persisted (§5.2), so it
  exists only on the message objects the browser is holding. The handler replaced the whole list
  with a `getMessages` projection, which carries no trajectory, so every committed turn lost its
  group at once. The turn in flight kept its group only because its commit appends *after* the
  rebuild — which is why the symptom read as "everything before the trim, but not the newest turn".
- **The read was stale by construction.** `history_trimmed` is emitted from inside the turn
  (`maybe_compact_durable_history` at turn setup, `recompact_durable_history_after_loop_trim` after
  the loop), while the store is rewritten once the turn resolves
  (`persist_trimmed_session_history` → `replace_messages`). So the fetch returned the pre-trim
  transcript, and nothing re-read at turn end — the dropped turns stayed on screen while the notice
  said they were gone. Confirmed against the live store: session `gw_6349935a-…` holds
  `user=7 / assistant=46 / tool=45` with the breadcrumb as its first row, i.e. 5 kept turns + the
  turn that was in flight + the breadcrumb, written *after* that turn ended.
- **The rebuild was lossier than the cut.** Server rows carry no tool data at all until a trim
  rewrite has happened (`persist_conversation_messages` skips every non-`Chat` variant), rebuilt
  rows get synthetic timestamps, and client-only `ephemeral` bubbles are dropped. The refetch was
  therefore not "server authority at the cost of a round trip"; it was a read that could not do its
  job and destroyed more than the local path.

Fix: the handler cuts locally from `kept_turns` — the path that already existed as its own fallback
— and no longer reads the store. `historyTrimMerge.logic.ts` had no caller left and is deleted;
`normalizeUserContent` was its only reader and goes with it; its `stripServerTimestamp` cases move
to `lib/stripServerTimestamp.test.ts` (registered in `test:contexts`).

The frame's contract is now written down where the frame is built (`history_trimmed_ws_frame`), with
the event declarations pointing at it (`TurnEvent::HistoryTrimmed`, `ObserverEvent::HistoryTrimmed`):
it reports an in-memory trim, the store follows after the turn, and consumers cut locally from
`kept_turns`. Zerocode already did exactly that (`purge_entries_to_kept_user_turns`); the WebUI was
the only client that re-read.

Verified: `npm test` (164), `npm run typecheck`, `cargo check -p zeroclaw-api -p zeroclaw-gateway
--all-targets`. Both trim cases were run against the old implementation and fail there — the kept
turn's group count goes 1 → 0 and the frame issues a store read — so they are regression tests
rather than descriptions. The Docker cross-compile for the arm64 CLI was **not** run: it does not
work in this sandbox.

### Fifth review round — comment ownership and stale plan text

Review of the fourth round found **no behavior gap** — the local cut, the preserved `segments`, the
zeroed counters and both regression tests were re-checked against the source. Two documentation
findings, fixed by amending the same commit.

- **Comment ownership.** The `history_trimmed` case comment and the `purgeUiMessagesToKeptUserTurns`
  JSDoc in `AgentContext.tsx` restated rules owned elsewhere — the runtime's turn accounting, the
  `[Tool results]` rounds, the breadcrumb's `synthetic` flag, Zerocode's purge. Per
  `docs/book/src/contributing/how-to.md` ("point to the owner instead of copying it"), both keep only
  the local why (why cut rather than re-read) and point at `history_trimmed_ws_frame` /
  `TurnEvent::HistoryTrimmed`. `TurnEvent::HistoryTrimmed` was shrunk the same way: the full prose
  lives in `ws.rs`, where the store rewrite (`persist_trimmed_session_history`) is visible next to it.
- **Stale plan text.** Four places still described the removed read: `webui-overall-plan.md` fact 25
  and the P1 paragraph that cites it, and — the same fact — `webui-composer-parity-plan.md` fact 14
  plus §4.1's "the client re-fetches it". The message-flow plan's scope line still claimed "no
  additional backend change" while the commit adds Rust doc comments. All now describe the local cut;
  the scope line reads "no backend behavior change". Line references in both fact tables were
  refreshed to the post-edit file.

Comments and docs only — no behavior change, so the fourth round's test evidence still stands;
`npm test`, `npm run typecheck` and `cargo check --all-targets` were re-run anyway.

### Composer stats semantics (canonical)

Authoritative product rules for the InputBar telemetry
(`web/src/lib/sessionStats.logic.ts`, rendered by `SessionStatsRow` inside the
toolbar between `+` and the context ring). Supersedes the live-window /
`本次` framing in `webui-composer-parity-plan.md` §4 and C-D1/C-D8 for **display**.

#### What each field means

| Field | Definition | Source |
|---|---|---|
| **turns** | Count of user-initiated bubbles still in the current conversation's message list (`role === 'user'`, excluding ephemeral / notice). | `messages[]` |
| **steps** | UI units still visible for LLM work in the message flow. For each committed agent answer that still has a live trajectory: **group messages + tool calls + 1 final answer**. Group "messages" = steps that carry thinking or text (same rule as the `ToolCallGroup` header). Tool calls = sum of `toolCalls` on those prefix steps. Hydrated answers with no `segments` count **1** each (the retained final only). Loose tool-card rows are not counted again (already inside `segments`). While a turn is in flight, the same formula is applied to `liveTurn` (closed steps + open step's tools + open text/thinking as the arriving answer). | `messages[].segments` + `liveTurn` |
| **tok** | `Σ (input_tokens + output_tokens)` from WebSocket `usage` frames received on **this page** for the current session. | live `TokenStats` |
| **cache hit** | `Σ cached_input_tokens ÷ Σ input_tokens`, clamped to `[0, 1]`, shown as a whole percent. Missing cache fields count as 0. When `input === 0`, the row still shows **`cache hit 0%`** (segment is never hidden). | live `TokenStats` |

Example: a turn whose group header reads `9 tool calls · 4 messages`, with a final answer below, contributes **4 + 9 + 1 = 14** to **steps** for that turn.

**Not** a pure LLM-round-trip counter: one model completion may request several tools; those tools each add to **steps** under this UI-unit rule. After a hard refresh, trajectories (`segments`) are gone, so **steps** collapses to one per retained final answer — matching what the list still shows.

#### When values update

| Trigger | turns / steps | tok / cache |
|---|---|---|
| Message list changes (hydrate, send, commit answer, delete, trim cut, session switch) | Recounted from current `messages` (+ `liveTurn`) via `displayStats` | unchanged except where noted below |
| `usage` / `done` / `aborted` WebSocket frames | unchanged (except in-flight `liveTurn` while streaming) | folded into `TokenStats` (`applyUsage` / `applyDone`) |
| Session switch, transcript reset, `history_trimmed` | follow the new/cut list | `TokenStats` cleared to zero |
| Full page reload | recounted from hydrated list (finals only → steps = finals) | start at zero (nothing persisted) |

No polling and no extra network: turns/steps are an O(n) scan of the in-memory list on each React update that already re-renders the chat; tokens only move when frames arrive.

#### Placement

Stats sit **inside** the InputBar toolbar (`+` · status · stats · context ring · send), not on a separate row under the card.

Option D (durable tool trajectory) remains out of scope — see §11.

---

## 11. Deferred — option D, do NOT implement

Making the trajectory **durable** means persisting tool rounds, which is a one-line change at
`ws.rs:919-925` plus a decision about the trim rewrite path. The web client's parsers already exist
(fact 16), so the UI would need no change.

**It is out of scope because it is not a UI change.** It would:

1. inflate the session store — an 860-step turn would add roughly 1,700 rows;
2. require the trim rewrite path (`persist_trimmed_session_history` →
   `session_transcript_messages`) to preserve the same data, and it is **not confirmed** whether that
   path already does;
3. **change what the model sees on the next turn.** Today the model loses its own tool-use history
   across turns and restarts. Fixing persistence fixes that too — arguably a real bug, but it is an
   agent-behaviour change and needs its own plan and review.

If someone wants D, it gets a separate document. Do not fold it into this work.
