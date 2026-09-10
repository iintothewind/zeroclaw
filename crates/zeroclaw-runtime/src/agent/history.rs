use crate::agent::history_pruner::remove_orphaned_tool_messages;
use anyhow::Result;
use regex::Regex;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::LazyLock;
use zeroclaw_providers::ChatMessage;

static LOCAL_IMAGE_PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?:[A-Za-z]:[\\/]|\\\\[^\s<>'"`\]\)/\\]+[\\/]|/)[^\s<>'"`\]\)]+?\.(?i:png|jpe?g|webp|gif|bmp)"#,
    )
    .expect("valid image path regex")
});

/// Returns the largest UTF-8 character boundary at or before `index`.
///
/// This compatibility wrapper preserves the previously exported helper while
/// directing new callers to the standard-library implementation.
#[deprecated(since = "0.8.4", note = "use str::floor_char_boundary instead")]
pub fn floor_char_boundary(s: &str, index: usize) -> usize {
    // Keep downstream callers source-compatible without retaining duplicate boundary logic.
    s.floor_char_boundary(index)
}

/// Indicates which side of a truncated string a boundary belongs to when
/// nudging it away from a half-cut `[IMAGE:...]` marker.
#[derive(Clone, Copy)]
enum TruncationSide {
    /// Boundary is the end of the kept head; nudge backward (out of the marker).
    Head,
    /// Boundary is the start of the kept tail; nudge forward (out of the marker).
    Tail,
}

fn nudge_around_image_marker(s: &str, boundary: usize, side: TruncationSide) -> usize {
    const OPEN: &str = "[IMAGE:";
    if boundary == 0 || boundary >= s.len() {
        return boundary;
    }

    // Walk forward to find the most recent `[IMAGE:` whose `[` is strictly
    // before `boundary`. Searching forward (rather than `rfind` on a prefix)
    // correctly handles the case where `boundary` itself splits the literal
    // `[IMAGE:` token.
    let mut search_from = 0usize;
    let mut last_open: Option<usize> = None;
    while let Some(rel) = s[search_from..].find(OPEN) {
        let open_idx = search_from + rel;
        if open_idx >= boundary {
            break;
        }
        last_open = Some(open_idx);
        search_from = open_idx + OPEN.len();
    }
    let Some(open_idx) = last_open else {
        return boundary;
    };

    // First `]` after the opener closes the marker (canonicalize regex
    // forbids `]` inside paths, so this is unambiguous in practice).
    let close_idx = match s[open_idx..].find(']') {
        Some(rel) => open_idx + rel,
        None => return boundary, // malformed input — leave the boundary alone
    };

    if close_idx < boundary {
        return boundary; // marker fully closed before boundary — safe
    }

    match side {
        TruncationSide::Head => open_idx,
        TruncationSide::Tail => (close_idx + 1).min(s.len()),
    }
}

pub fn truncate_tool_result(output: &str, max_chars: usize) -> String {
    if max_chars == 0 || output.len() <= max_chars {
        return output.to_string();
    }
    let head_len = max_chars * 2 / 3;
    let tail_len = max_chars.saturating_sub(head_len);
    let head_end = output.floor_char_boundary(head_len);
    // ceil_char_boundary: find smallest byte index >= i on a char boundary
    let tail_start_raw = output.len().saturating_sub(tail_len);
    let tail_start = if tail_start_raw >= output.len() {
        output.len()
    } else {
        let mut pos = tail_start_raw;
        while pos < output.len() && !output.is_char_boundary(pos) {
            pos += 1;
        }
        pos
    };

    // Step boundaries away from any `[IMAGE:...]` marker they would bisect.
    // `[IMAGE:` and `]` are pure ASCII, so the adjusted indices land on
    // valid UTF-8 char boundaries.
    let head_end = nudge_around_image_marker(output, head_end, TruncationSide::Head);
    let tail_start = nudge_around_image_marker(output, tail_start, TruncationSide::Tail);

    // Guard against overlap when max_chars is very small
    if head_end >= tail_start {
        return output[..output.floor_char_boundary(max_chars)].to_string();
    }
    let truncated_chars = tail_start - head_end;
    format!(
        "{}\n\n[... {} characters truncated ...]\n\n{}",
        &output[..head_end],
        truncated_chars,
        &output[tail_start..]
    )
}

fn is_existing_local_image_path(path: &str) -> bool {
    let candidate = Path::new(path);
    candidate.is_absolute()
        && candidate.is_file()
        && candidate
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                matches!(
                    ext.to_ascii_lowercase().as_str(),
                    "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp"
                )
            })
}

fn existing_marker_payloads(output: &str) -> std::collections::HashSet<&str> {
    const OPEN: &str = "[IMAGE:";
    let mut set = std::collections::HashSet::new();
    let mut from = 0usize;
    while let Some(rel) = output[from..].find(OPEN) {
        let inner_start = from + rel + OPEN.len();
        let Some(rel_end) = output[inner_start..].find(']') else {
            break;
        };
        let inner_end = inner_start + rel_end;
        set.insert(output[inner_start..inner_end].trim());
        from = inner_end + 1;
    }
    set
}

/// Rewrite real local image file paths in tool output into `[IMAGE:...]`
/// markers so the multimodal pipeline can normalize them before the next
/// provider call. This targets shell/skill outputs that print filesystem
/// paths directly rather than returning explicit media markers.
pub fn canonicalize_tool_result_media_markers(output: &str) -> String {
    let existing_markers = existing_marker_payloads(output);
    let mut rewritten = String::with_capacity(output.len());
    let mut cursor = 0usize;
    let mut changed = false;

    for mat in LOCAL_IMAGE_PATH_RE.find_iter(output) {
        let start = mat.start();
        let end = mat.end();
        let path = &output[start..end];

        // Skip paths that are already part of an explicit media marker.
        if output[..start].ends_with("[IMAGE:") {
            continue;
        }

        // Skip a bare path that already appears inside an explicit marker
        // elsewhere in the same output — promoting it would double-count the
        // image (see `existing_marker_payloads`).
        if existing_markers.contains(path) {
            continue;
        }

        if !is_existing_local_image_path(path) {
            continue;
        }

        rewritten.push_str(&output[cursor..start]);
        rewritten.push_str("[IMAGE:");
        rewritten.push_str(path);
        rewritten.push(']');
        cursor = end;
        changed = true;
    }

    if !changed {
        return output.to_string();
    }

    rewritten.push_str(&output[cursor..]);
    rewritten
}

fn is_path_listing_tool(tool_name: &str) -> bool {
    matches!(
        tool_name.to_ascii_lowercase().as_str(),
        "content_search" | "glob_search"
    )
}

pub fn canonicalize_tool_result_media_markers_for(tool_name: &str, output: &str) -> String {
    if is_path_listing_tool(tool_name) {
        output.to_string()
    } else {
        canonicalize_tool_result_media_markers(output)
    }
}

/// Truncate a tool message's content, preserving JSON structure when the
/// message stores `tool_call_id` alongside `content` (native tool-call
/// format). Without this, `truncate_tool_result` destroys the JSON envelope
/// and downstream model_providers receive a `null` `call_id`.
pub fn truncate_tool_message(msg_content: &str, max_chars: usize) -> String {
    if max_chars == 0 || msg_content.len() <= max_chars {
        return msg_content.to_string();
    }
    if let Ok(mut obj) =
        serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(msg_content)
        && obj.contains_key("tool_call_id")
        && let Some(serde_json::Value::String(inner)) = obj.get("content")
    {
        let truncated = truncate_tool_result(inner, max_chars);
        obj.insert("content".to_string(), serde_json::Value::String(truncated));
        return serde_json::to_string(&obj).unwrap_or_else(|_| msg_content.to_string());
    }
    truncate_tool_result(msg_content, max_chars)
}

/// Script-aware content token estimate (no per-message framing).
///
/// Weights are accumulated in quarter-token units then `div_ceil(4)`:
/// - Latin / ASCII: 1 unit (≈4 chars → 1 token) — matches the historical
///   byte heuristic for ASCII so English estimates stay stable.
/// - CJK ideographs, kana, Hangul: 4 units (≈1 char → 1 token) — corrects
///   the UTF-8 `bytes/4` under-count (~0.75 tok/char).
/// - Other (emoji, symbols, most non-Latin scripts): 2 units (≈2 chars → 1
///   token) — mildly conservative.
///
/// This is a water-line / cascade heuristic, not a provider tokenizer.
/// Prefer [`crate::agent::history_trim::ContextCalibration`] once a provider
/// reports `input_tokens`.
#[must_use]
pub(crate) fn estimate_text_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    let mut units = 0usize;
    for ch in text.chars() {
        units = units.saturating_add(char_token_units(ch));
    }
    units.div_ceil(4)
}

fn char_token_units(ch: char) -> usize {
    if is_cjk_kana_or_hangul(ch) {
        4
    } else if ch.is_ascii() || is_latin_letter_extended(ch) {
        1
    } else {
        2
    }
}

/// CJK Unified (+ common extensions), kana, Hangul, and CJK punctuation /
/// compatibility blocks that typically tokenize near 1:1 with BPE families.
fn is_cjk_kana_or_hangul(ch: char) -> bool {
    matches!(
        ch,
        '\u{1100}'..='\u{11FF}' // Hangul Jamo
            | '\u{3000}'..='\u{303F}' // CJK Symbols and Punctuation
            | '\u{3040}'..='\u{309F}' // Hiragana
            | '\u{30A0}'..='\u{30FF}' // Katakana
            | '\u{3100}'..='\u{312F}' // Bopomofo
            | '\u{3130}'..='\u{318F}' // Hangul Compatibility Jamo
            | '\u{3400}'..='\u{4DBF}' // CJK Ext A
            | '\u{4E00}'..='\u{9FFF}' // CJK Unified
            | '\u{AC00}'..='\u{D7AF}' // Hangul Syllables
            | '\u{F900}'..='\u{FAFF}' // CJK Compatibility Ideographs
            | '\u{FF66}'..='\u{FF9D}' // Halfwidth katakana
            | '\u{20000}'..='\u{2FA1F}' // CJK Ext B–F (approx)
    )
}

fn is_latin_letter_extended(ch: char) -> bool {
    // Treat common Latin extensions as Latin (same 4 chars/token weight).
    matches!(
        ch,
        '\u{00C0}'..='\u{024F}' // Latin-1 letters through Extended-B
            | '\u{1E00}'..='\u{1EFF}' // Latin Extended Additional
    )
}

/// Estimate the token cost of a single message using the script-aware text
/// heuristic plus ~4 framing tokens (role, delimiters). Single-sourced so the
/// history and system-floor estimates stay in lock-step. `pub(crate)` so the
/// calibration in [`crate::agent::history_trim::ContextCalibration`] can price
/// the unbilled messages appended after a provider-reported usage snapshot.
///
/// `ChatMessage` has only `role` + `content` — there are no separate
/// `tool_calls` / `reasoning_content` fields. On the working-copy path, native
/// tool calls and reasoning are serialized into `content` by
/// [`crate::agent::turn::parse_response`], so this content-only estimate already
/// prices those payloads as text. Durable
/// [`zeroclaw_providers::ConversationMessage`] history uses
/// `estimate_conversation_tokens` / provider-view sizing instead.
pub(crate) fn estimate_message_tokens(message: &ChatMessage) -> usize {
    estimate_text_tokens(&message.content).saturating_add(4)
}

/// Estimate token count for a message history using the script-aware heuristic.
/// Includes a small overhead per message for role/framing tokens.
pub fn estimate_history_tokens(history: &[ChatMessage]) -> usize {
    history.iter().map(estimate_message_tokens).sum()
}

pub fn estimate_system_floor_tokens(history: &[ChatMessage]) -> usize {
    history
        .iter()
        .filter(|m| m.role == "system")
        .map(estimate_message_tokens)
        .sum()
}

#[must_use]
pub fn context_floor_remediation(system_floor: usize, budget: usize) -> String {
    let floor_s = system_floor.to_string();
    let budget_s = budget.to_string();
    crate::i18n::get_required_cli_string_with_args(
        "history-trim-floor-exceeds-budget",
        &[("floor", floor_s.as_str()), ("budget", budget_s.as_str())],
    )
}

/// Hard-fail when cascade still exceeds the send budget (typically at
/// `kept_turns == 1`, but callers must not assume that from the wording alone).
#[must_use]
pub fn context_overflow_unrecoverable_message(
    kept_turns: usize,
    preferred_keep: usize,
    tokens_after: usize,
    send_budget: usize,
) -> String {
    format!(
        "Context overflow unrecoverable: whole-turn trim cannot free enough context \
         (kept_turns={kept_turns}, preferred={preferred_keep}, tokens_after={tokens_after}, \
         send_budget={send_budget})"
    )
}

/// Provider rejected for context size, but local estimate is within the send
/// budget and whole-turn trim dropped nothing (history already ≤ preferred keep).
#[must_use]
pub fn context_overflow_nothing_droppable_message(
    kept_turns: usize,
    preferred_keep: usize,
    tokens_after: usize,
    send_budget: usize,
) -> String {
    format!(
        "Context overflow: provider rejected request but local estimate is within \
         send_budget and no whole turns were droppable \
         (kept_turns={kept_turns}, preferred={preferred_keep}, tokens_after={tokens_after}, \
         send_budget={send_budget})"
    )
}

/// Choose the unrecoverable vs nothing-droppable message from trim outcome flags.
#[must_use]
pub fn context_overflow_trim_fail_message(
    trimmed: bool,
    exceeds_budget: bool,
    kept_turns: usize,
    preferred_keep: usize,
    tokens_after: usize,
    send_budget: usize,
) -> String {
    if !trimmed && !exceeds_budget {
        context_overflow_nothing_droppable_message(
            kept_turns,
            preferred_keep,
            tokens_after,
            send_budget,
        )
    } else {
        context_overflow_unrecoverable_message(
            kept_turns,
            preferred_keep,
            tokens_after,
            send_budget,
        )
    }
}

/// Structured attrs for overflow hard-fail / nothing-droppable logs.
#[must_use]
pub fn context_overflow_trim_fail_attrs(
    kept_turns: usize,
    preferred_keep: usize,
    tokens_after: usize,
    send_budget: usize,
    trimmed: bool,
    exceeds_budget: bool,
) -> serde_json::Value {
    serde_json::json!({
        "kept_turns": kept_turns,
        "preferred_keep": preferred_keep,
        "tokens_after": tokens_after,
        "send_budget": send_budget,
        "trimmed": trimmed,
        "exceeds_budget": exceeds_budget,
        "error_key": if !trimmed && !exceeds_budget {
            "context_overflow_nothing_droppable"
        } else {
            "context_overflow_unrecoverable"
        },
    })
}

pub fn normalize_system_messages(history: &mut Vec<ChatMessage>) {
    let mut saw_system = false;
    let mut system_content = String::new();
    let mut non_system = Vec::with_capacity(history.len());

    for message in history.drain(..) {
        if message.role == "system" {
            saw_system = true;
            if !message.content.is_empty() {
                if !system_content.is_empty() {
                    system_content.push_str("\n\n");
                }
                system_content.push_str(&message.content);
            }
        } else {
            non_system.push(message);
        }
    }

    if saw_system && !system_content.is_empty() {
        history.push(ChatMessage::system(system_content));
    }
    history.extend(non_system);
}

pub fn append_or_merge_system_message(history: &mut Vec<ChatMessage>, content: impl Into<String>) {
    let content = content.into();
    if content.is_empty() {
        normalize_system_messages(history);
        return;
    }

    if let Some(system_message) = history.iter_mut().find(|message| message.role == "system") {
        if !system_message.content.is_empty() {
            system_message.content.push_str("\n\n");
        }
        system_message.content.push_str(&content);
    } else {
        history.insert(0, ChatMessage::system(content));
    }
    normalize_system_messages(history);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InteractiveSessionState {
    pub version: u32,
    pub history: Vec<ChatMessage>,
}

impl InteractiveSessionState {
    fn from_history(history: &[ChatMessage]) -> Self {
        Self {
            version: 1,
            history: history.to_vec(),
        }
    }
}

pub fn load_interactive_session_history(
    path: &Path,
    system_prompt: &str,
) -> Result<Vec<ChatMessage>> {
    if !path.exists() {
        return Ok(vec![ChatMessage::system(system_prompt)]);
    }

    let raw = std::fs::read_to_string(path)?;
    let mut state: InteractiveSessionState = serde_json::from_str(&raw)?;
    if state.history.is_empty() {
        state.history.push(ChatMessage::system(system_prompt));
    } else if state.history.first().map(|msg| msg.role.as_str()) != Some("system") {
        state.history.insert(0, ChatMessage::system(system_prompt));
    }
    normalize_system_messages(&mut state.history);
    if state.history.first().map(|msg| msg.role.as_str()) != Some("system") {
        state.history.insert(0, ChatMessage::system(system_prompt));
    }

    remove_orphaned_tool_messages(&mut state.history);

    Ok(state.history)
}

pub fn save_interactive_session_history(path: &Path, history: &[ChatMessage]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let payload = serde_json::to_string_pretty(&InteractiveSessionState::from_history(history))?;
    std::fs::write(path, payload)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies the exported compatibility wrapper retains the legacy UTF-8 boundary contract.
    #[allow(deprecated)]
    #[test]
    fn floor_char_boundary_compatibility_wrapper_delegates_to_std() {
        let text = "abc😀def";

        assert_eq!(floor_char_boundary(text, 5), 3);
        assert_eq!(floor_char_boundary(text, usize::MAX), text.len());
    }

    #[test]
    fn estimate_system_floor_counts_only_system_messages() {
        let history = vec![
            ChatMessage::system("You are helpful."), // 16 chars -> 4 + 4 = 8
            ChatMessage::user("What is Rust?"),      // counted by history, not floor
            ChatMessage::assistant("A language."),   // counted by history, not floor
        ];
        // Floor = system message only; conversation turns are prunable.
        assert_eq!(estimate_system_floor_tokens(&history), 8);
        assert!(estimate_system_floor_tokens(&history) < estimate_history_tokens(&history));
    }

    #[test]
    fn estimate_system_floor_empty_and_no_system() {
        assert_eq!(estimate_system_floor_tokens(&[]), 0);
        let history = vec![ChatMessage::user("hi"), ChatMessage::assistant("yo")];
        assert_eq!(estimate_system_floor_tokens(&history), 0);
    }

    #[test]
    fn estimate_ascii_matches_legacy_four_chars_per_token() {
        // Pure ASCII stays at ~4 chars/token + framing so English water-lines
        // do not jump relative to the historical byte heuristic.
        let msg = "a".repeat(40);
        assert_eq!(estimate_message_tokens(&ChatMessage::user(&msg)), 14);
        assert_eq!(
            estimate_history_tokens(&[ChatMessage::user("hello world")]),
            7
        );
    }

    #[test]
    fn estimate_cjk_is_higher_than_utf8_byte_heuristic() {
        let cjk = "中文测试内容用于估算"; // 10 ideographs
        let legacy_bytes = cjk.len().div_ceil(4) + 4;
        let est = estimate_message_tokens(&ChatMessage::user(cjk));
        assert!(
            est > legacy_bytes,
            "CJK estimate ({est}) must exceed UTF-8 bytes/4 ({legacy_bytes})"
        );
        // 10 chars × 1 tok + framing 4
        assert_eq!(est, 14);
    }

    #[test]
    fn estimate_mixed_cjk_latin_between_pure_scripts() {
        let latin = "abcdefghij"; // 10 ASCII → ceil(10/4)+4 = 7
        let cjk = "中文测试内容用于估算"; // 10 CJK → 10+4 = 14
        let mixed = "abcde中文测试估"; // 5 ASCII + 5 CJK → ceil((5+20)/4)+4 = ceil(25/4)+4 = 7+4 = 11
        let latin_est = estimate_message_tokens(&ChatMessage::user(latin));
        let cjk_est = estimate_message_tokens(&ChatMessage::user(cjk));
        let mixed_est = estimate_message_tokens(&ChatMessage::user(mixed));
        assert_eq!(latin_est, 7);
        assert_eq!(cjk_est, 14);
        assert_eq!(mixed_est, 11);
        assert!(latin_est < mixed_est && mixed_est < cjk_est);
    }

    #[test]
    fn context_floor_remediation_names_budget_floor_and_runtime_profile_surface() {
        let msg = context_floor_remediation(2000, 100);
        // Names the resolved budget N the runtime actually used ...
        assert!(
            msg.contains("100"),
            "remediation must name the resolved budget: {msg}"
        );
        // ... and the measured system floor ...
        assert!(
            msg.contains("2000"),
            "remediation must name the system floor: {msg}"
        );
        // ... points at the config surface an operator can change ...
        assert!(
            msg.contains("[runtime_profiles"),
            "remediation must point at the runtime-profile surface: {msg}"
        );
        // ... and never at the inert agent-inline knob
        assert!(
            !msg.contains("agent.max_context_tokens"),
            "remediation must not reference the inert agent.max_context_tokens: {msg}"
        );
    }

    #[test]
    fn context_overflow_trim_fail_message_distinguishes_nothing_droppable() {
        let nothing = context_overflow_trim_fail_message(false, false, 5, 5, 1200, 8000);
        assert!(
            nothing.contains("no whole turns were droppable"),
            "expected nothing-droppable wording: {nothing}"
        );
        assert!(nothing.contains("kept_turns=5"));
        assert!(nothing.contains("preferred=5"));
        assert!(!nothing.contains("only one turn"));

        let unrecoverable = context_overflow_trim_fail_message(true, true, 1, 5, 9000, 8000);
        assert!(
            unrecoverable.contains("cannot free enough context"),
            "expected unrecoverable wording: {unrecoverable}"
        );
        assert!(unrecoverable.contains("kept_turns=1"));
        assert!(unrecoverable.contains("preferred=5"));
        assert!(!unrecoverable.contains("only one turn"));

        let attrs = context_overflow_trim_fail_attrs(5, 5, 1200, 8000, false, false);
        assert_eq!(
            attrs.get("error_key").and_then(|v| v.as_str()),
            Some("context_overflow_nothing_droppable")
        );
    }

    #[test]
    fn canonicalize_tool_result_media_markers_wraps_existing_local_image_path() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("generated.png");
        std::fs::write(&image, [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']).unwrap();

        let input = format!(
            "Image generated successfully.\nFile: {}",
            image.display().to_string()
        );
        let output = canonicalize_tool_result_media_markers(&input);

        assert!(output.contains("[IMAGE:"));
        assert!(output.contains(&format!("[IMAGE:{}]", image.display().to_string())));
    }

    #[test]
    fn canonicalize_tool_result_media_markers_ignores_missing_paths() {
        let input = "File: /tmp/definitely-missing-zeroclaw-image.png";
        let output = canonicalize_tool_result_media_markers(input);
        assert_eq!(output, input);
    }

    #[test]
    fn canonicalize_tool_result_media_markers_preserves_existing_markers() {
        let input = "Already tagged [IMAGE:/tmp/already-tagged.png]";
        let output = canonicalize_tool_result_media_markers(input);
        assert_eq!(output, input);
    }

    #[test]
    fn canonicalize_for_skips_path_listing_tools() {
        // A search/listing tool that surfaces a real image path must be left
        // untouched - promoting it to [IMAGE:...] would falsely trigger vision
        // routing
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("hit.png");
        std::fs::write(&image, [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']).unwrap();
        let input = format!("match: {}", image.display());

        for tool in ["content_search", "glob_search", "GLOB_SEARCH"] {
            let output = canonicalize_tool_result_media_markers_for(tool, &input);
            assert_eq!(output, input, "{tool} output must be left untouched");
            assert!(!output.contains("[IMAGE:"));
        }
    }

    #[test]
    fn canonicalize_for_wraps_image_producing_and_fetching_tools() {
        // Default-allow: image_gen (produces) and file_download (fetches) keep
        // canonicalization so a genuinely produced/fetched image still routes.
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("generated.png");
        std::fs::write(&image, [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']).unwrap();
        let input = format!("Saved to {}", image.display());
        let expected = format!("[IMAGE:{}]", image.display());

        for tool in ["image_gen", "file_download", "some_future_tool"] {
            let output = canonicalize_tool_result_media_markers_for(tool, &input);
            assert!(
                output.contains(&expected),
                "{tool} output should be canonicalized into a marker"
            );
        }
    }

    #[test]
    fn canonicalize_tool_result_media_markers_dedups_path_already_in_marker() {
        let input = "File: /tmp/pic.png\nFormat: png\n[IMAGE:/tmp/pic.png]";
        let output = canonicalize_tool_result_media_markers(input);
        assert_eq!(
            output, input,
            "bare path duplicating an existing marker must not be promoted"
        );
        assert_eq!(
            output.matches("[IMAGE:").count(),
            1,
            "exactly one image marker expected, got: {output}"
        );
    }

    #[test]
    fn truncate_tool_result_does_not_split_image_marker_at_head_boundary() {
        // 200-byte path → marker length 207 bytes. With max_chars=80 the
        // naive head_end (= 80 * 2 / 3 = 53) falls inside the marker.
        let path = format!("/tmp/{}.png", "a".repeat(200));
        let marker = format!("[IMAGE:{path}]");
        let output = format!("prefix-text {marker} trailing-text padding-padding");

        let truncated = truncate_tool_result(&output, 80);

        assert!(
            truncated.contains("[... ") && truncated.contains("characters truncated ...]"),
            "expected truncation marker in output, got: {truncated}"
        );
        // No half-`[IMAGE:` marker should leak into the surviving content.
        let stripped = truncated.replace(&marker, "");
        assert!(
            !stripped.contains("[IMAGE:"),
            "half-`[IMAGE:` marker leaked into truncated output: {truncated}"
        );
    }

    #[test]
    fn truncate_tool_result_does_not_split_image_marker_at_tail_boundary() {
        // Marker placed near the end so tail_start (~max_chars / 3 from the
        // end) lands inside it.
        let path = format!("/tmp/{}.png", "b".repeat(200));
        let marker = format!("[IMAGE:{path}]");
        let output = format!("{} preamble-content-line {marker} ending", "x".repeat(400));

        let truncated = truncate_tool_result(&output, 90);

        let stripped = truncated.replace(&marker, "");
        assert!(
            !stripped.contains("[IMAGE:") && !stripped.contains(".png]"),
            "half-`[IMAGE:` marker leaked into truncated output: {truncated}"
        );
    }

    #[test]
    fn truncate_tool_result_keeps_complete_marker_in_head() {
        let marker = "[IMAGE:/tmp/short.png]";
        let output = format!("{marker} {}", "y".repeat(500));

        let truncated = truncate_tool_result(&output, 200);

        assert!(
            truncated.starts_with(marker),
            "expected head to retain full marker, got: {truncated}"
        );
    }
}
