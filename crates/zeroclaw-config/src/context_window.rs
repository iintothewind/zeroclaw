//! Local per-model context-window table.
//!
//! Resolution chain for an agent's effective context window:
//! 1. explicit provider `context_window` (config.toml) — always wins;
//! 2. this table, keyed by normalized model id — best-effort for well-known
//!    models so an unconfigured provider does not fall back to a stub number;
//! 3. [`crate::schema::UNCONFIGURED_CONTEXT_WINDOW_FALLBACK`] — last resort.
//!
//! The table records **conservative lower bounds** for each model family:
//! when a family ships variants with different windows, the smallest widely
//! available value is used. An explicit `context_window` is the knob for
//! operators who know their deployment gets more.

/// Normalized `(model-family prefix, context window in tokens)` entries.
/// Matching is longest-prefix over normalized model ids (see
/// [`normalize_model_id`]), so `claude-sonnet-4-5-20250929` resolves via the
/// `claude-sonnet-4-5` entry.
const MODEL_CONTEXT_WINDOWS: &[(&str, usize)] = &[
    // ── Anthropic Claude ─────────────────────────────────────────────
    ("claude-opus-4-1", 200_000),
    ("claude-opus-4", 200_000),
    ("claude-sonnet-4-5", 200_000),
    ("claude-sonnet-4", 200_000),
    ("claude-haiku-4-5", 200_000),
    ("claude-3-7-sonnet", 200_000),
    ("claude-3-5", 200_000),
    ("claude-3-opus", 200_000),
    ("claude-3-sonnet", 200_000),
    ("claude-3-haiku", 200_000),
    ("claude-2.1", 200_000),
    ("claude-2", 100_000),
    // ── OpenAI ───────────────────────────────────────────────────────
    ("gpt-5", 400_000),
    ("gpt-4.1", 1_047_576),
    ("gpt-4o", 128_000),
    ("gpt-4-turbo", 128_000),
    ("gpt-3.5", 16_385),
    ("o3", 200_000),
    ("o4-mini", 200_000),
    // ── Google Gemini ────────────────────────────────────────────────
    ("gemini-3", 1_048_576),
    ("gemini-2.5-pro", 1_048_576),
    ("gemini-2.5-flash", 1_048_576),
    ("gemini-2.0", 1_048_576),
    // ── Qwen ─────────────────────────────────────────────────────────
    ("qwen3", 131_072),
    ("qwen2.5", 131_072),
    // ── Mistral ──────────────────────────────────────────────────────
    ("mistral-large", 131_072),
    ("mistral-small", 131_072),
    ("codestral", 262_144),
    // ── Meta Llama ───────────────────────────────────────────────────
    ("llama-4", 1_048_576),
    ("llama-3.3", 131_072),
    ("llama-3.1", 131_072),
    ("llama-3", 8_192),
    // ── DeepSeek ─────────────────────────────────────────────────────
    ("deepseek-chat", 65_536),
    ("deepseek-reasoner", 65_536),
    ("deepseek-v3", 65_536),
    ("deepseek-r1", 65_536),
    // ── xAI Grok ─────────────────────────────────────────────────────
    ("grok-4", 256_000),
    ("grok-3", 131_072),
    ("grok-2", 131_072),
    // ── Moonshot Kimi ────────────────────────────────────────────────
    ("kimi-k2", 131_072),
    ("kimi-latest", 131_072),
    // ── Zhipu GLM ────────────────────────────────────────────────────
    ("glm-4.6", 200_000),
    ("glm-4", 131_072),
];

/// Normalize a model id for table lookup: lowercase, strip any
/// `provider/`-style route prefix, and strip a trailing `-YYYYMMDD` date
/// suffix (with or without the leading dash).
fn normalize_model_id(model: &str) -> String {
    let mut id = model.trim().to_ascii_lowercase();
    if let Some((_, rest)) = id.split_once('/') {
        id = rest.to_string();
    }
    // Strip trailing date segments such as `-20250929` or `20250929`.
    let trimmed = id.trim_end_matches('-');
    let mut segments: Vec<&str> = trimmed.split('-').collect();
    while let Some(last) = segments.last() {
        let is_date = last.len() == 8 && last.bytes().all(|b| b.is_ascii_digit());
        if is_date {
            segments.pop();
        } else {
            break;
        }
    }
    segments.join("-")
}

/// Longest-prefix lookup of a normalized model id in the table. A prefix
/// entry only matches when followed by a `-` boundary, so `gpt-4o` never
/// matches an entry keyed `gpt-4`.
fn lookup_normalized(normalized: &str) -> Option<usize> {
    let mut best: Option<(usize, usize)> = None; // (key length, window)
    for (prefix, window) in MODEL_CONTEXT_WINDOWS {
        let matched = if normalized == *prefix {
            true
        } else if let Some(rest) = normalized.strip_prefix(prefix) {
            rest.starts_with('-')
        } else {
            false
        };
        if matched && best.is_none_or(|(len, _)| prefix.len() > len) {
            best = Some((prefix.len(), *window));
        }
    }
    best.map(|(_, window)| window)
}

/// Best-effort context window for a model id, or `None` when the table has
/// no entry for it.
#[must_use]
pub fn lookup_model_context_window(model: &str) -> Option<usize> {
    lookup_normalized(&normalize_model_id(model))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_route_prefix_and_date_suffix() {
        assert_eq!(
            normalize_model_id("anthropic/claude-sonnet-4-5-20250929"),
            "claude-sonnet-4-5"
        );
        assert_eq!(normalize_model_id("claude-3-5-sonnet-20241022"), "claude-3-5-sonnet");
        assert_eq!(normalize_model_id("gpt-5"), "gpt-5");
    }

    #[test]
    fn resolves_known_models_with_boundary_matching() {
        assert_eq!(lookup_model_context_window("gpt-4o"), Some(128_000));
        assert_eq!(lookup_model_context_window("gpt-4o-mini"), Some(128_000));
        assert_eq!(
            lookup_model_context_window("openai/gpt-4.1-2025-04-14"),
            Some(1_047_576)
        );
        assert_eq!(
            lookup_model_context_window("claude-3-5-haiku-latest"),
            Some(200_000)
        );
    }

    #[test]
    fn longest_prefix_wins() {
        // `gpt-4` alone must NOT match the `gpt-4o` entry via `-` boundary;
        // but `gpt-4o`'s own prefix match on `gpt-4o` must beat any shorter.
        assert_eq!(lookup_model_context_window("gpt-4"), None);
        assert_eq!(lookup_model_context_window("llama-3.1-70b"), Some(131_072));
    }

    #[test]
    fn unknown_models_return_none() {
        assert_eq!(lookup_model_context_window("my-custom-llama-v3"), None);
        assert_eq!(lookup_model_context_window(""), None);
    }
}
