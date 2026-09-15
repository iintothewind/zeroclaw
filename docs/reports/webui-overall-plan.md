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
   (click opens a detail panel), and a stats row under the input:
   `本次 2 轮 10 步 · 135K tok · 缓存命中 83%`. The always-visible linear `ContextBar` is removed.
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
cd web && npm run test:contexts     # turnStream + chatHistoryStorage + modelPicker + historyTrimMerge
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
| 25 | On `history_trimmed` the client **re-fetches and replaces the whole message list** | `AgentContext.tsx:606-616` |
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
is the only reset that needs code**: a trim rewrites the server-side transcript and the client
re-fetches it (fact 25), but it does **not** touch JS counters, so `history_trimmed` must zero them
explicitly, next to the existing `tokens_after` assignment at `AgentContext.tsx:596-597`.

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

- [ ] P0 landed; gateway tests green single-threaded
- [ ] P1 landed; `npm run test:contexts`, `npm test`, `npm run typecheck`, `npm run build` green
- [ ] P2 landed; same suite green; existing `turnStream.logic.test.ts` cases unchanged
- [ ] P3 landed; `chatHistoryStorage.logic.test.ts` updated
- [ ] All new test files registered in `web/package.json`
- [ ] The 7 manual checks in P4 pass
- [ ] Only `zh` + `en` gained strings
- [ ] `git status` clean apart from intended changes

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
