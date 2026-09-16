# Plan: message flow — step segments + collapsible tool group (live-only)

**Status:** implemented — see `webui-overall-plan.md` §10 for the landed commits, the deviations
from §3, and the review rounds. This document is the design record and its acceptance criteria,
not a live status.
**Scope:** `web/` only. Reuses the `usage` frame already planned in
`webui-composer-parity-plan.md` §5.1 — **no additional backend change.**
**Goal source:** port DeepSeek Harness's conversation view — thinking, tool calls, and text
interleaved per step, with the non-final steps collapsed under a summary row
(`7 次工具调用 · 6 条消息`) that expands into the full trajectory.
**Durability:** live-only. Tool calls are not persisted (composer plan §3 facts 10–12), so this view
covers turns watched live and is lost on refresh. The durable variant is tracked separately as
option D and is **out of scope** — it changes agent context, not just the UI.

---

## 1. Goal

Turn the live turn view from "one growing text bubble with tool cards stacking below it" into
**ordered step segments**: each LLM call's thinking, its text, and its tool calls, in the order they
happened, with the non-final steps collapsed under a summary header and the final answer left
expanded — matching the reference.

```
 分析: FAK ALL IN RATE REFERENCE 260415.xlsx                    14:38
 ┌──────────────────────────────────────────────────────────┐
 │ 7 次工具调用 · 6 条消息  ⌄                                 │   ← collapsed by default
 └──────────────────────────────────────────────────────────┘
 完美! 现在我可以为您提供详细的分析报告：                            ← final answer, expanded
 ...
```

Expanded, the group shows the trajectory in order:

```
 思考 · 用户要求分析 Excel 文件，但当前环境没有直接读取 Excel 的工具…
 我来读取并分析这个 Excel 文件。首先让我检查是否有可以直接读取的文本格式…
 ▸ Pwsh · Read and analyze Excel file structure and content
 思考 · 文件路径解析失败，需要使用 PowerShell 命令确认文件的实际位置…
 文件路径似乎有问题。让我检查一下当前工作目录并找到正确的文件路径：
 ▸ Pwsh · Check current directory and find Excel files
 …
```

---

## 2. Non-goals

- **Durability.** Nothing is written to the session store; a refresh loses the segments (see the
  header). This is the same trade the composer's stats row makes, and it is deliberate.
- **Backfilling history.** Existing conversations have no tool-call data at all (composer plan §3
  fact 9), so they will keep rendering as they do today.
- **Changing what the model sees.** Fixing the persist filter (composer plan §3 fact 11) would make
  this durable, but it also changes the agent's context across turns. Separate proposal, separate
  review.
- **The trajectory tab.** Unchanged.
- Per-step timing / TTFT / tok/s. The frames carry no timing.

---

## 3. Verified facts

| # | Fact | Evidence |
|---|---|---|
| 1 | **There is no server-side step boundary.** `chunk_reset` exists only in the web client (`types/api.ts`, `turnStream.logic.ts`, its test, `AgentContext.tsx`) — **no Rust code emits it**. The client's model of it is dead. | repo-wide search |
| 2 | **Per-step event order is `thinking` → `chunk` → `Usage` → `ToolCall`/`ToolResult`.** `Usage` is emitted by `record_accepted_chat_response` once the response is accepted; tool-call events fire later, during execution. | `parse_response.rs:267-311`; `call_prep.rs:207,303` |
| 3 | So the `usage` frame being added for the composer (its §5.1) is **also the step boundary** — one per LLM call, arriving exactly when a step's response is complete and before its tool calls. | #2 |
| 4 | Today's turn state is **flat**: `pendingContent` and `pendingThinking` concatenate the whole turn, with no per-step structure. | `turnStream.logic.ts:7-16` |
| 5 | Tool cards are pushed into `messages` as separate agent messages, in arrival order. | `AgentContext.tsx:482-525` |
| 6 | Text and thinking stream into **one** bubble that keeps growing, rendered after the message list. | `AgentChat.tsx:825-840` |
| 7 | **Therefore the live view does not preserve interleaving today**: pre-tool text accumulates in a single bubble while tool cards stack below it, so a 7-step turn reads as "one blob of text, then 7 cards" rather than the reference's alternating sequence. | #4–#6 |
| 8 | `ToolCallCard` already renders icon + name + args + output + executing spinner, and correlates results by `tool_call_id`. | `ToolCallCard.tsx` |
| 9 | The collapsible pattern already exists in this codebase — thinking renders inside a `<details>`. | `AgentChat.tsx:1061-1066`, `:834-836` |
| 10 | `agent_start` is emitted once per turn; `done`/`aborted` terminate it. | `ws.rs:1083`, `:1550`, `:1445` |

---

## 4. Step model

Two pure structures, both testable without React.

```ts
/** One LLM call: what it thought, what it said, what it invoked. */
export interface StepSegment {
  thinking: string;
  text: string;
  toolCalls: ToolCallInfo[];
}

/** A turn's full trajectory, in order. */
export interface TurnSegments {
  steps: StepSegment[];
  /** The turn's final answer, rendered outside the collapsed group. */
  finalText: string;
  finalThinking: string;
}
```

**Step boundary rule.** A step closes on the `usage` frame (fact 3). `tool_call` frames arriving
after it attach to the step that just closed — including several calls, which is one step making
parallel tool calls, not several steps.

**Why not infer boundaries from `tool_call` alone.** A step that ends the turn makes no tool call, so
tool-call-derived boundaries cannot distinguish "final answer" from "step with no tools yet", and
they would mis-split a step that emits several calls. The `usage` frame is server-authoritative and
free (already planned).

**Frame → segment mapping.**

| Frame | Effect |
|---|---|
| `thinking` | append to the open step's `thinking` |
| `chunk` | append to the open step's `text` |
| `usage` | close the open step; push it if it has any thinking, text, or tool calls |
| `tool_call` | append to the last closed step's `toolCalls` (open a step if none) |
| `tool_result` | fill the matching `toolCalls[].output` by `id` |
| `done` | `finalText` = `full_response`; the last step becomes `finalThinking` if it has no tool calls |
| `aborted` | keep what was accumulated; the last step becomes the final one |
| `turn_start` | fresh `TurnSegments` |

---

## 5. Frontend changes

### 5.1 `web/src/contexts/turnStream.logic.ts` (extend, not rewrite)
`TurnStreamState` gains `segments: TurnSegments` and a `openStep: StepSegment`. Add the `usage`
frame to `TurnStreamFrame`. Keep `pendingContent` / `pendingThinking` / `capturedThinking` /
`hadToolCall` — `classifyCompletion` depends on them and its behaviour must not change
(the reasoning-only / empty / tool-only classification is already covered by tests).

New exported pure helpers: `closeStep(state)`, `attachToolCall(state, call)`, `attachToolResult(state, id, output)`.

### 5.2 `web/src/pages/messageFlow.logic.ts` (new)
Pure grouping over the message list + segments → render blocks:

```ts
export type RenderBlock =
  | { kind: 'user'; message: ChatMessage }
  | { kind: 'turn'; segments: TurnSegments; collapsed: boolean }
  | { kind: 'notice'; message: ChatMessage }
  | { kind: 'plain'; message: ChatMessage }
```

`groupMessages(messages, segmentsByTurn)` is pure and unit-tested. It must leave hydrated turns
(plain user + assistant pairs) rendering exactly as they do today — `plain` blocks.

### 5.3 `web/src/components/ToolCallGroup.tsx` (new)
- Header: `{calls} 次工具调用 · {messages} 条消息` with a chevron; collapsed by default.
- Body: the steps in order — thinking as `<details>`, text as markdown, `ToolCallCard` per call.
- Reuses `ToolCallCard` unchanged (fact 8) and the existing `<details>` pattern (fact 9).

### 5.4 `web/src/pages/AgentChat.tsx`
- Replace the single streaming bubble (fact 6) with the turn block from §5.2.
- Tool cards stop being pushed as standalone bubbles and instead live inside the turn block. This is
  the one behavioural change to the list; §6 acceptance 4 pins it.
- Everything else untouched: avatars, timestamps, hover actions, scroll behaviour, the composer.

### 5.5 `web/src/contexts/AgentContext.tsx`
- Keep `segments` per turn alongside the existing state.
- `tool_call` / `tool_result` still update the message list as today, so the group can correlate
  results by id (fact 8); the difference is only where they render.
- No new fetch, no new endpoint.

### 5.6 i18n
`agentchat.tool_group_summary` (`{calls} 次工具调用 · {messages} 条消息`), `agentchat.tool_group_expand`,
`agentchat.tool_group_collapse`. `zh` + `en`; the other 29 fall back to English. Reuse the existing
`agentchat.thinking` label for the thinking `<details>`.

---

## 6. Acceptance criteria

1. A turn with N tool calls renders one collapsed row reading `N 次工具调用 · M 条消息`, with the
   final answer expanded below it.
2. Expanding the row shows the steps **in order**: each step's thinking, then its text, then its tool
   cards — matching the sequence the frames arrived in.
3. A step that issues several tool calls in one LLM call shows them **inside one step**, not as
   separate steps.
4. A turn with no tool calls renders as a plain agent bubble — no empty group, no header reading
   `0 次工具调用`.
5. Hydrated (reloaded) conversations render exactly as they do today: `plain` blocks, no group. This
   is the accepted live-only trade, asserted rather than tolerated.
6. `npm test` stays green, including the existing `turnStream.logic.test.ts` classification cases
   (reasoning-only, empty, tool-only) — this change must not alter `classifyCompletion`.
7. New unit tests: `groupMessages` (turn with tools, turn without, hydrated turn, notice rows,
   consecutive tool-only turns) and the step boundary (a `usage` frame closes a step; `tool_call`
   after `usage` attaches to that step).
8. Zero new network requests (same assertion as the composer plan's acceptance 5).
9. Only `zh` + `en` gain strings.

---

## 7. Decisions and open questions

**D1 — collapse default: expanded while streaming, collapse on `done` unless the user toggled it.**
Streaming output must be visible; a finished turn matches the reference by being collapsed. The
user's manual toggle wins and is remembered per turn. *Confirm.*

**D2 — `M 条消息` definition.** The reference's second number is not documented. Proposed: the count
of steps that carry thinking or text (i.e. the visible non-tool items inside the group). *Confirm —
if it means something else, only this one function changes.*

**D3 — where the group sits.** Above the final answer, matching the reference. The final answer never
goes inside the group, even when the last step produced text.

**Q1 — does the group wrap the whole turn or only the tool-calling prefix?** Proposal: only the
prefix before the final answer, which is what makes the reference's layout possible.

**Q2 — turns that end with a tool call and no final text.** Proposal: the group renders alone
(collapsed), with no empty bubble beneath it. The existing `classifyCompletion` `skip` outcome
already models "tools are the record".

---

## 8. Implementation order

1. `turnStream.logic.ts`: add `segments` + the `usage` frame + the three helpers. Tests first —
   this is where the step boundary lives, and it is pure.
2. `messageFlow.logic.ts` + tests: grouping, with the hydrated-conversation case pinned.
3. `ToolCallGroup.tsx`.
4. `AgentChat.tsx`: render the turn block; retire the single streaming bubble.
5. `AgentContext.tsx`: wire `usage` → `closeStep`, and the tool frames into segments.
6. i18n `zh` + `en`.
7. Test sweep; manual check against a real multi-step turn (a 7-tool-call turn is the reference case).

---

## 9. Dependency on the composer plan

This plan **requires** the `usage` frame from `webui-composer-parity-plan.md` §5.1 — it is the step
boundary (fact 3). That frame is added for the composer's step counter; this plan reuses it at no
extra cost.

**If the composer work is deferred, this plan needs a fallback.** The only alternative is inferring
boundaries client-side from `tool_call` frames, which fact 3's ordering makes unreliable for
tool-free steps and for parallel calls. Recommendation: keep the two plans together, or land the
`usage` frame first on its own — it is ~15 lines and independently testable.
