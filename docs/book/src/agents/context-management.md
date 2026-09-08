# Context management

Context management in ZeroClaw is, at its core, **cache management**. Every
byte that changes in the message prefix replayed to a provider forces that
provider to re-prefill the prefix, which costs both latency and money. The
runtime therefore measures context against a single, shared budget and prunes
history so the replayed prefix stays as byte-stable as possible. Trimming is
engaged by two orthogonal trigger lines — a token budget and a message-count cap —
checked every turn; [Two independent trigger lines](#two-independent-trigger-lines)
below.

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

The same `effective_context_window` is the denominator for **both** the context
utilization meter (the Zerocode / web UI `ctx: used / max` bar) and every
trim-budget computation, so the meter can never disagree with the runtime that
fills it.

## Configuration: `[runtime_profiles.<alias>.context]`

Context policy lives in one table per runtime profile. It replaces the legacy
`[runtime_profiles.<alias>.history_pruning]` and
`[runtime_profiles.<alias>.context_compression]` tables. Every field is
validated at load time (`Config::validate`) and is **never silently clamped at
resolution time** — an invalid value is a hard config error, not a reinterpret.

```toml
[runtime_profiles.coder.context]
max_input_tokens = 100_000    # hard ceiling on request input tokens (optional)
trim_threshold_percent = 80    # where history trimming engages (default: 80)
reserve_tokens = 16_384        # output headroom held back from the threshold (optional)
```

| Field | Type / default | Meaning |
|-------|----------------|---------|
| `max_input_tokens` | `Option<usize>`, default `None` | Hard ceiling on total request input tokens. `None` means the effective context window is the sole limit. This is the only override knob for providers whose real window is unknown. It is a **ceiling** and does **not** by itself trigger trimming. When set, it clamps the effective window (and thus the UI meter denominator) down via `model_window.min(max_input_tokens)`. |
| `trim_threshold_percent` | `usize`, default `80` | Percentage of the effective context window at which history trimming engages. Must be in `1..=100`; `0` or `>100` is a hard config error. |
| `reserve_tokens` | `Option<usize>`, default `None` | Output headroom subtracted from the window before the trim threshold is computed (`window − reserve` bounds the threshold from above). Must not exceed **50%** of the effective window — beyond that the reserve would strand most of the window, which is a hard config error checked against the resolved model window. `None` means no reserve. |

All three have defaults, so an existing `config.toml` needs no changes to keep
working: omit the table entirely and you get `max_input_tokens = None`,
`trim_threshold_percent = 80`, `reserve_tokens = None`.

## The trim threshold

The token count at which trimming engages is derived from the effective window
so both the operator-chosen percentage and the output reserve bound it:

```text
trim_threshold = min(window × trim_threshold_percent / 100,
                     window − reserve_tokens)        # reserve term omitted when reserve_tokens is None
```

When replayed context tokens exceed `trim_threshold`, the runtime trims oldest
whole turns until the estimate fits (see [History management](./history-management.md)).
Token counts are estimated by `history::estimate_history_tokens` (roughly four
characters per token plus framing tokens per message) — a heuristic, not a
provider tokenizer — and are re-anchored on the provider's authoritative
reported input size after each accepted response.

## Two independent trigger lines

Token utilization is not the only thing that engages trimming. Two **orthogonal**
limits are checked each turn, and whichever is crossed first trims oldest whole
turns until it is satisfied on its own terms:

| Trigger line | Measured against | Fires when | Trim target (what is retained) |
|---|---|---|---|
| Token budget | `trim_threshold` (above) | replayed context tokens `> trim_threshold` | oldest turns dropped until the estimate fits `trim_threshold` |
| Message count | `max_history_messages` | the non-system body exceeds the cap | oldest whole turns dropped until the body fits the cap |

The message-count cap is `max_history_messages` from the runtime profile. When
omitted, the legacy raw cap is `50`; a structured agent's effective cap is the
derived `max(50, 2 * max_tool_iterations + 2)`. See
[History management](./history-management.md#structured-message-count-limit) for
the full derivation. Both lines share the same floor: leading system messages and
the newest whole turn are never dropped, and trimming is always whole-turn (a turn
is never cut in half).

Because the two lines are independent, a very large window can still trim on
message count long before it nears its token threshold — on big-window deployments
the count line, not the percentage, is often the first to engage.

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
