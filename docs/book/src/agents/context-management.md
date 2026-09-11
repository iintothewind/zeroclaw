# Context management

Context management in ZeroClaw is, at its core, **cache management**. Every
byte that changes in the message prefix replayed to a provider forces that
provider to re-prefill the prefix, which costs both latency and money. The
runtime therefore measures context against a single, shared budget and prunes
history so the replayed prefix stays as byte-stable as possible. Trimming is
engaged by a single token water-line: when replayed context exceeds
`trim_threshold`, the runtime prefers the most recent `keep_recent_turns`
whole turns, then cascades down to `kept_turns == 1` until under the fit
budget (or hard-fails). The trigger is token-based; the action is
turn-count-first with a token fit check. See [History management](./history-management.md).

This page is the reference for the knobs and the arithmetic. For how whole-turn
trimming physically drops messages, see [History management](./history-management.md);
for the provider send path, see [Runtime internals](./internals.md).

## The effective context window

Everything trims against one number: the agent's **effective context window**.
It is resolved in this order (first match wins):

1. **Explicit provider `context_window`** (`[providers.<id>].context_window`) —
   always wins. This is the operator's source of truth.
2. **Built-in per-model table** — a conservative lower bound keyed by normalized
   model id, so an unconfigured provider does not silently fall back to a stub.
   For example `llama-3.1-70b` resolves to `131,072` and `gpt-4o` to `128,000`.
   Where a family ships variants with different windows, the smallest widely
   available value is recorded; an explicit `context_window` is the knob to raise
   it when you know your deployment gets more.
3. **Fallback `32_000`** (`UNCONFIGURED_CONTEXT_WINDOW_FALLBACK`) — last resort
   for a genuinely unknown model. Treat this as "not configured", not as the
   model's real capacity.

The resolved window is then clamped by the `[context]` input ceiling:

```text
effective_context_window = model_window.min(max_input_tokens)   # max_input_tokens: None ⇒ no clamp
```

The same `effective_context_window` is the denominator for the context
utilization meter (the Zerocode / web UI `ctx: used / max` bar). Trim uses two
derived quantities from that window — the **water-line** (when to engage) and
the **send / fit budget** (whether retained history is enough) — described
below. They must not be conflated.

## Configuration: `[runtime_profiles.<alias>.context]`

Context policy lives in one table per runtime profile. It replaces the legacy
`[runtime_profiles.<alias>.history_pruning]` and
`[runtime_profiles.<alias>.context_compression]` tables. Range-checked fields
(`trim_threshold_percent`, `reserve_tokens`) are validated at load time
(`Config::validate`) and are **never silently clamped** — an invalid value is
a hard config error. `keep_recent_turns` is the exception: it is clamped at
resolution to `1..=10` (see the field table below).

```toml
[runtime_profiles.coder.context]
max_input_tokens = 100_000    # hard ceiling on request input tokens (optional)
trim_threshold_percent = 80    # where history trimming engages (default: 80)
reserve_tokens = 16_384        # output headroom held back from the threshold (optional)
keep_recent_turns = 5          # preferred whole turns retained when trimming fires (default: 5)
```

| Field | Type / default | Meaning |
|-------|----------------|---------|
| `max_input_tokens` | `Option<usize>`, default `None` | Hard ceiling on total request input tokens. `None` means the effective context window is the sole limit. This is the only override knob for providers whose real window is unknown. It is a **ceiling** and does **not** by itself trigger trimming. When set, it clamps the effective window (and thus the UI meter denominator) down via `model_window.min(max_input_tokens)`. |
| `trim_threshold_percent` | `usize`, default `80` | Percentage of the effective context window at which history trimming engages. Must be in `1..=100`; `0` or `>100` is a hard config error. |
| `reserve_tokens` | `Option<usize>`, default `None` | Output headroom. (1) Bounds the water-line from above: `trim_threshold ≤ window − reserve`. (2) Defines the **send / fit budget** after trim: `send_budget = window − reserve` (or the full window when unset). Must not exceed **50%** of the effective window — beyond that the reserve would strand most of the window, which is a hard config error checked against the resolved model window. |
| `keep_recent_turns` | `usize`, default `5` | Preferred how many of the newest **whole turns** the trim action retains when it fires. Clamped at resolution to `1..=10` (`0` becomes `1`, `>10` becomes `10`). If those turns still exceed **`send_budget`**, the runtime cascades down to `kept_turns == 1`; it never drops the newest turn. |

All four have defaults, so an existing `config.toml` needs no changes to keep
working: omit the table entirely and you get `max_input_tokens = None`,
`trim_threshold_percent = 80`, `reserve_tokens = None`, `keep_recent_turns = 5`.

## The trim threshold (water-line) vs the send / fit budget

Two different numbers share the same window. Do not use the water-line as the
cascade fit target.

```text
trim_threshold (water-line) = min(window × trim_threshold_percent / 100,
                                  window − reserve_tokens)   # reserve term omitted when None

send_budget (fit)           = window − reserve_tokens        # or window when reserve is None
```

- **Water-line** — *when* trimming engages (`tokens > trim_threshold`). Default
  percent is `80`. This is **not** the post-trim success criterion.
- **Send / fit budget** — *whether* retained history is enough after the action.
  Ideal end state:

  ```text
  estimate(system + tool/skill prompt prefix + retained turns)
    ≤ send_budget
  ```

  Equivalently: preferred `keep_recent_turns` (default 5) should fit under
  `send_budget` together with the non-droppable prefix; if not, cascade further.

When replayed context tokens exceed `trim_threshold`, the trigger fires and the
trim action runs on **durable** agent/session history (not only a disposable
working copy): see [History management](./history-management.md).
`HistoryTrimmed` carries optional `tokens_after` so clients can refresh the
context meter immediately after a **successful** trim.

### How size is measured

Context size for the water-line and cascade uses two layers:

1. **Local heuristic** — `history::estimate_history_tokens` /
   `estimate_message_tokens`: a **script-aware** char walk (Latin ≈4 chars/token;
   CJK / kana / Hangul ≈1 char/token; other scripts mildly conservative) plus
   ~4 framing tokens per message. Not a provider tokenizer; good enough for
   cold start and unbilled tails. Working-copy `ChatMessage` values have only
   `role` + `content`; native tool calls and reasoning are serialized into
   `content` before append, so the content-only estimate already prices those
   payloads. Durable `ConversationMessage` history uses a structured estimate
   (or the provider view via `to_provider_messages`) instead.
2. **Provider support fact** — after an accepted response,
   `ContextCalibration` re-anchors on `usage.input_tokens` (`prompt_tokens`).
   Only the unbilled tail (assistant / tool rows appended since that report)
   is priced locally. Streaming paths request `stream_options.include_usage`
   when stream options are enabled; if usage is missing, the turn continues on
   the local estimate.

Optional `prompt_tokens_details.cached_tokens` (vLLM
`--enable-prompt-tokens-details`, OpenAI, etc.) maps to
`cached_input_tokens`. For OpenAI-compatible backends it is a **subset** of
`input_tokens` — used for cache hit / cost observability, **not** added into
context occupancy for trim or the meter. Absence of details is normal and must
not affect trim correctness.

## The trim action: prefer keep-N, then cascade to send budget

The token water-line above is the **only** trim trigger. When it fires, the
action is a three-stage cascade (`history_trim::trim_to_budget` /
`trim_conversation_to_budget`). Fit checks use **`send_budget`**, never
`trim_threshold_percent` / the water-line.

| Stage | Behavior | Stop when |
|-------|----------|-----------|
| 1. Turns first | Keep the newest `keep_recent_turns` whole turns (default `5`, clamped `1..=10`) plus leading system (tools/skills live in that non-droppable prefix) | Estimated / calibrated tokens **≤ `send_budget`** |
| 2. Budget cascade | Still over → choose the **largest** `kept_turns` in `1..=preferred` whose estimate fits `send_budget` (drop the oldest whole turns in one compaction; never keep a gap in the middle). **Warn** when the chosen keep is below preferred | A fitting `kept_turns` exists, **or** only `kept_turns == 1` remains to try |
| 3. Hard fail | Floor (system/tools/skills prefix + newest turn) still over `send_budget` | Abort the turn with an **error message only** (no `HistoryTrimmed` notice); never drop below one turn |

How stage 2 is decided does not matter to operators: the runtime may probe by decrementing keep, binary-search, or estimate turn sizes once. The contract is the outcome — maximal keep that fits, whole turns only, floor 1.

| | Value |
|---|---|
| Trigger | replayed context tokens `> trim_threshold` (water-line; default 80%) |
| Fit target | `send_budget = window − reserve` (full window if no reserve) |
| Preferred retain | `keep_recent_turns` newest whole turns |
| Floor | `kept_turns >= 1` (newest whole turn always retained; system never dropped) |

Trimming is always whole-turn — a turn is never cut in half. An oversized newest
turn alone may still exceed `send_budget`; that is a hard failure (stage 3),
not a license to slice the turn.

## Pathological windows: the floor cannot fit

When the effective context window is so small that the **floor** — leading
system messages plus the one newest whole turn that trimming must always
retain — does not fit, there is **no path below keep=1**: the runtime never
truncates the system prompt and never drops the newest turn. It fails
explicitly instead, with these guards:

| Condition | Where | Behavior |
|---|---|---|
| System floor alone ≥ send budget | turn start (`iteration == 0`) | **Warning only** (log `error_key: context_floor_exceeds_budget`): remediation points at raising the window / shrinking tools. Treated as a configuration problem — the turn may still proceed until cascade / provider overflow hard-fails. |
| Cascade to `kept_turns == 1` still exceeds **send budget** | pre-send / reported-budget / overflow recovery | **Hard failure**: error message to the caller (CLI / UI); **no** successful-trim `HistoryTrimmed` notice. |

The overflow-recovery path is reached when a provider rejects the request with
a context-window error (matched by `reliable::is_context_window_exceeded` across
patterns such as `"exceeds the context window"`, `"maximum context length"`,
`"prompt is too long"`). Recovery runs the same keep-N → cascade action; it
retries only when the cascade lands under **`send_budget`**.

In short: `keep_recent_turns` is the **preferred** retain count; the cascade may
keep fewer turns (down to 1) to fit. When the window cannot hold the floor, the
correct fixes are operational — raise
`[runtime_profiles.<name>].context.max_input_tokens` (or the provider
`context_window`), disable unused integrations to shrink the tool surface, or
point the agent at a larger-window model.

## Cache-economics feedback

Trimming is cache-aware. The provider send path reconciles the message list
through an append-only log that preserves the longest byte-stable prefix, so
pruning rewrites only the tail past the first divergent message and earlier
bytes stay cacheable across turns. The cacheable prefix length is emitted at
`DEBUG` (`"provider prefix cache: N of M messages byte-stable"`), and the
provider-reported prefix-cache hit count is surfaced as a first-class metric
`zeroclaw_tokens_cached_input_total` (labels `model_provider`, `model`) and the
OTel span attribute `zeroclaw.usage.cached_input_tokens`. Together they let you
compare re-prefill cost directly against prefix-cache hits. Providers that
report the field include Anthropic, OpenAI, OpenRouter, compatible, and
OpenAI-Codex; providers that do not (Bedrock, Gemini, Azure, Copilot, Ollama)
report `None` and the metric simply stays at zero for them.

## Troubleshooting the meter

- **The denominator looks wrong (too small).** The agent's `max_input_tokens` is
  clamping the window. Check `[runtime_profiles.<alias>.context].max_input_tokens`.
- **The denominator is `32,000`.** No provider `context_window` is set and the
  model id is not in the per-model table. Set an explicit
  `[providers.<id>].context_window`, or fix the model id so it matches a table
  entry.
- **Config fails to load with `reserve_tokens` / `trim_threshold_percent`.**
  These are hard validation errors, not clamps: `trim_threshold_percent` must be
  `1..=100`, and `reserve_tokens` must not exceed 50% of the resolved model
  window.

## Locked prompt / catalog decisions

These choices are intentional and should not be "fixed" without an ADR:

- **No runtime wall-clock injection.** The runtime does **not** inject the
  current date or time into the system prompt, channel turn-context preamble,
  channel history user turns, or embedded-agent user turns. Put calendar /
  timezone guidance in workspace instructions (`AGENTS.md`) or load it via
  tools when needed. This keeps the replayed prefix byte-stable for provider
  prompt caching. Display-only strip helpers may still recognize legacy
  `[CURRENT DATE & TIME: …]` prefixes in old transcripts.
- **`HEARTBEAT.md` is load-on-use.** It is **not** part of the interactive
  system prompt. The heartbeat engine reads it from disk when heartbeat is
  enabled; if the file is missing, that path is skipped. Agent startup does
  **not** unconditionally seed `HEARTBEAT.md` (the personality editor still
  exposes it for manual setup).
- **Skills and Deferred catalogs are session-frozen and name-sorted.** Within
  a session the catalog text is treated as a stable cacheable prefix. Rendering
  sorts skills by name and deferred MCP stubs by `prefixed_name` so catalog
  order does not depend on discovery order.
- **Deferred: multi-breakpoint / Project Context physical split.** Splitting
  Project Context across multiple physical prompt breakpoints is explicitly
  out of scope for now; keep a single Project Context block until that design
  lands.
- **Compact skills.** In compact injection mode, skill summaries stay in the
  system prompt; full skill bodies are loaded via `read_skill` into history,
  not re-inlined into the system prefix.
