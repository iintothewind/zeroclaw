# History management

The runtime keeps conversation history for each agent session and sends a
provider-facing working history to the model. **Token-trigger trimming** is the
sole mechanism: when the replayed context exceeds the token water-line, it drops
oldest whole turns so only the newest `keep_recent_turns` turns remain.

Trim is **durable**: it compacts the agent's stored conversation history and the
session backend transcript (via `SessionBackend::replace_messages`), not only a
per-turn working copy. Clients that show the message stream must drop purged
bubbles when they receive `HistoryTrimmed` (the existing trim notice remains).

Trimming retains turns atomically. A turn starts at a real user message and
includes the assistant response and any tool calls and tool results before the
next user message. Trimming therefore does not split a tool call from its
result.

## Whole-turn retention

`history_trim::trim_to_recent_turns` (provider `ChatMessage` view) and
`history_trim::trim_conversation_to_recent_turns` (durable
`ConversationMessage` history) implement the same keep-N whole-turn action.

Leading system messages are retained. When no trim is needed, message order and
shape are left unchanged.

## Token trigger

The trim trigger is `ResolvedRuntime::trim_threshold_tokens()`, derived from the
agent's effective context window (provider `context_window` → built-in per-model
table → `32_000` fallback, clamped by `max_input_tokens`):

- `trim_threshold = min(window × trim_threshold_percent / 100, window −
  reserve_tokens)`, with the reserve term omitted when `reserve_tokens` is unset.
  Both knobs live in `[runtime_profiles.<alias>.context]`; the percentage
  defaults to `80`, so one profile trims at the same relative point for models of
  any window size. See [Context management](./context-management.md) for the full
  reference.

Token counts are estimated by `history::estimate_history_tokens`: roughly four
characters per token plus four framing tokens per message. This is a heuristic,
not a provider tokenizer.

When replayed context exceeds `trim_threshold`, the trigger fires and the trim
action keeps the newest `keep_recent_turns` whole turns. `keep_recent_turns`
defaults to `5` and is clamped to `1..=10` at resolution (`0` becomes `1`,
`>10` becomes `10`). The token count of the retained turns is **not** a budget:
keeping N whole turns is the action, and the token water-line is the only
trigger. The newest whole turn is always retained; a single oversized turn may
still exceed the provider window, by design.

Trimming runs before the first provider call of a turn when history already
exceeds the effective threshold and at provider-call boundaries between
tool-loop iterations, including reactively when a provider reports that the
context window was exceeded. It retains whole turns, so it never splits a
tool exchange.

## Visible trimming

Whenever trimming drops older turns, the runtime:

1. Inserts a breadcrumb before the first retained turn so the model knows that
   earlier context was omitted.
2. Emits `HistoryTrimmed` with the number of dropped messages, retained turns,
   and a reason identifying the trim trigger.

The event is surfaced through the active client transport and through the
observer path used by dashboards and event subscribers. Trimming is therefore
not log-only and is not silent to either the model or connected clients.

## Pairing safety

Whole-turn retention is the primary tool-pairing guarantee: a tool call and its
result belong to the same turn and are retained or dropped together. The orphan
sweep remains a final safety net for histories that were already inconsistent,
such as restored or externally modified sessions.

Tool-result length limits are separate. `max_tool_result_chars` bounds an
individual result when it is recorded; it does not trim conversation history.
Provider-side context enforcement is also separate, though a provider overflow
can trigger the runtime's reactive token-trigger trim.