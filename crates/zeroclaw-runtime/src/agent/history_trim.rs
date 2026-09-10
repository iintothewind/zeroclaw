//! Whole-turn history trimming.
//!
//! **Trigger** (caller's water-line): when to engage.
//! **Action** ([`trim_to_budget`] / [`trim_conversation_to_budget`]):
//! 1. Prefer keeping `preferred_keep` newest whole turns (default 5).
//! 2. If still over the **send** budget, choose the largest `kept_turns` in
//!    `1..=preferred` that fits (drop oldest whole turns in one compaction).
//! 3. If the floor still exceeds the send budget, set [`TrimResult::exceeds_budget`]
//!    — callers must alert and abort; never drop below one turn.
//!
//! [`trim_to_recent_turns`] remains the primitive keep-N cut used by the
//! cascade and by tests.

use crate::agent::history::{estimate_history_tokens, estimate_message_tokens};
use zeroclaw_providers::{ChatMessage, ConversationMessage};

const TOOL_RESULTS_PREFIX: &str = "[Tool results]";

/// Outcome of a trim pass. `trimmed` is true only when at least one whole turn
/// was dropped, in which case the caller emits a user-visible event and injects
/// a breadcrumb so the loss is never silent. `exceeds_budget` is true when the
/// retained floor (keep ≥ 1) still sits above the fit budget.
#[derive(Debug, Clone)]
pub struct TrimResult {
    pub history: Vec<ChatMessage>,
    pub dropped_messages: usize,
    pub dropped_turns: usize,
    pub kept_turns: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub trimmed: bool,
    /// After cascading to `kept_turns == 1`, history still exceeds `send_budget`.
    pub exceeds_budget: bool,
    /// True when a successful cascade kept fewer turns than `preferred_keep`.
    pub below_preferred_keep: bool,
}

/// Provider-authoritative context size, replacing the bare local estimate
/// wherever a trim decision is made after a billed response.
///
/// A provider reports the prompt size of a request **as it was sent**
/// (`usage.input_tokens` / `prompt_tokens`). That is the occupancy **support
/// fact** when present. Between that report and the next trim check, the loop
/// appends messages the provider has not billed yet — the assistant reply and
/// the tool results it just produced. Those unbilled rows are priced with the
/// script-aware local heuristic ([`estimate_message_tokens`]).
///
/// `cached_input_tokens` / `prompt_tokens_details.cached_tokens` are **not**
/// used here: for OpenAI-compatible backends they are a subset of
/// `input_tokens`, useful for cache metrics only.
///
/// Before any provider report exists (first iteration, or a provider that
/// emits no usage), [`ContextCalibration::current`] degrades to the plain
/// whole-history estimate.
#[derive(Debug, Clone, Default)]
pub struct ContextCalibration {
    /// Prompt tokens as reported by the provider for the request sent when the
    /// history had `calibrated_history_len` messages. `None` until a provider
    /// reports usage.
    reported_input_tokens: Option<usize>,
    /// `history.len()` at the moment `reported_input_tokens` was captured.
    calibrated_history_len: usize,
}

impl ContextCalibration {
    /// A fresh calibration with no provider anchor: every read is the plain
    /// whole-history estimate until [`Self::observe_reported`] lands.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a provider-authoritative prompt size for the current history.
    /// Called each time a response carries `usage.input_tokens`; the reported
    /// value is the full prompt the provider just saw, so the messages present
    /// now are all accounted for and become the new anchor.
    pub fn observe_reported(&mut self, reported_input_tokens: usize, history: &[ChatMessage]) {
        self.reported_input_tokens = Some(reported_input_tokens);
        self.calibrated_history_len = history.len();
    }

    /// Current context size: the authoritative anchor plus the estimated cost
    /// of any messages appended since it was captured. With no anchor yet, the
    /// whole history is estimated locally.
    ///
    /// A trim shrinks `history` below `calibrated_history_len`; the caller
    /// re-anchors via [`Self::observe_reported`] with the post-trim reported
    /// total, so this never double-counts. The `saturating_sub` here is only
    /// belt-and-suspenders for a stale anchor.
    #[must_use]
    pub fn current(&self, history: &[ChatMessage]) -> usize {
        match self.reported_input_tokens {
            None => estimate_history_tokens(history),
            Some(reported) => {
                let unbilled = history.len().saturating_sub(self.calibrated_history_len);
                let tail = history[history.len().saturating_sub(unbilled)..]
                    .iter()
                    .map(estimate_message_tokens)
                    .sum::<usize>();
                reported.saturating_add(tail)
            }
        }
    }
}

fn is_turn_boundary(msg: &ChatMessage) -> bool {
    msg.role == "user" && !msg.content.starts_with(TOOL_RESULTS_PREFIX)
}

fn is_system(msg: &ChatMessage) -> bool {
    msg.role == "system"
}

/// Drop oldest whole turns, retaining the `keep_turns` most recent whole
/// turns plus every leading system message. Primitive keep-N cut used by
/// [`trim_to_budget`]; callers that need the budget cascade should use that.
/// `exceeds_budget` is always false — this primitive ignores token budgets.
pub fn trim_to_recent_turns(history: Vec<ChatMessage>, keep_turns: usize) -> TrimResult {
    // At least the most recent turn always survives; `keep_turns == 0` is
    // normalised to 1 so the slice index below can never run past the end.
    let keep = keep_turns.max(1);
    let total_turns = count_turns(&history);
    let tokens_before = estimate_history_tokens(&history);

    let leading_system = history.iter().take_while(|m| is_system(m)).count();
    let system: Vec<ChatMessage> = history[..leading_system].to_vec();
    let body = &history[leading_system..];

    let boundaries: Vec<usize> = body
        .iter()
        .enumerate()
        .filter(|(_, m)| is_turn_boundary(m))
        .map(|(i, _)| i)
        .collect();

    // Nothing to drop when the body already fits within `keep` turns, or
    // when there is only a single turn (the newest must always survive).
    if boundaries.len() <= keep {
        return TrimResult {
            history,
            dropped_messages: 0,
            dropped_turns: 0,
            kept_turns: total_turns,
            tokens_before,
            tokens_after: tokens_before,
            trimmed: false,
            exceeds_budget: false,
            below_preferred_keep: false,
        };
    }

    // Retain the `keep` most recent turns: start the body at the boundary that
    // begins the keep-th turn counted from the end.
    let start = boundaries[boundaries.len() - keep];
    let dropped_messages = start;
    let dropped_turns = boundaries.iter().filter(|&&b| b < start).count();
    let mut kept = system;
    kept.extend_from_slice(&body[start..]);
    let kept_turns = total_turns - dropped_turns;
    let tokens_after = estimate_history_tokens(&kept);

    TrimResult {
        history: kept,
        dropped_messages,
        dropped_turns,
        kept_turns,
        tokens_before,
        tokens_after,
        trimmed: true,
        exceeds_budget: false,
        below_preferred_keep: false,
    }
}

/// When `context_send_budget` is unset (`0`), fall back to the water-line budget
/// so fit checks still run in tests and legacy call sites.
#[must_use]
pub fn resolve_send_budget(context_send_budget: usize, context_token_budget: usize) -> usize {
    if context_send_budget > 0 {
        context_send_budget
    } else {
        context_token_budget
    }
}

/// Prefer `preferred_keep` newest turns, then choose the largest keep in
/// `1..=preferred` that fits under `send_budget`. `send_budget == 0` skips the fit check.
///
/// Cascade probing uses per-message token prefix/suffix sums (one estimate pass)
/// instead of cloning the full history on every keep step.
pub fn trim_to_budget(
    history: Vec<ChatMessage>,
    preferred_keep: usize,
    send_budget: usize,
) -> TrimResult {
    let tokens_before = estimate_history_tokens(&history);
    let preferred = preferred_keep.max(1);
    let chosen_keep = choose_keep_turns_for_budget(
        &history,
        preferred,
        send_budget,
        |m| estimate_message_tokens(m),
        is_system,
        is_turn_boundary,
    );

    let mut result = trim_to_recent_turns(history, chosen_keep);
    result.tokens_before = tokens_before;
    result.exceeds_budget = send_budget > 0 && result.tokens_after > send_budget;
    result.below_preferred_keep =
        result.trimmed && !result.exceeds_budget && result.kept_turns < preferred;
    result
}

fn count_turns(history: &[ChatMessage]) -> usize {
    history.iter().filter(|m| is_turn_boundary(m)).count()
}

/// Pick the largest `kept_turns` in `1..=preferred` that fits `send_budget`
/// using one per-message token pass + O(1) probes (no full-history clones).
fn choose_keep_turns_for_budget<T>(
    history: &[T],
    preferred: usize,
    send_budget: usize,
    message_tokens: impl Fn(&T) -> usize,
    is_system_msg: impl Fn(&T) -> bool,
    is_boundary: impl Fn(&T) -> bool,
) -> usize {
    let total_turns = history.iter().filter(|m| is_boundary(m)).count().max(1);
    let mut chosen_keep = preferred.max(1).min(total_turns);
    if send_budget == 0 {
        return chosen_keep;
    }

    let leading_system = history.iter().take_while(|m| is_system_msg(m)).count();
    let msg_tokens: Vec<usize> = history.iter().map(|m| message_tokens(m)).collect();
    let system_tokens: usize = msg_tokens[..leading_system].iter().copied().sum();

    let body = &history[leading_system..];
    let boundaries: Vec<usize> = body
        .iter()
        .enumerate()
        .filter(|(_, m)| is_boundary(m))
        .map(|(i, _)| i)
        .collect();
    if boundaries.is_empty() {
        return 1;
    }

    let n = msg_tokens.len();
    let mut suffix = vec![0usize; n + 1];
    for i in (0..n).rev() {
        suffix[i] = suffix[i + 1].saturating_add(msg_tokens[i]);
    }

    while chosen_keep > 1 {
        let start = boundaries[boundaries.len() - chosen_keep];
        let abs_start = leading_system + start;
        let after = system_tokens.saturating_add(suffix[abs_start]);
        if after <= send_budget {
            break;
        }
        chosen_keep -= 1;
    }
    chosen_keep
}

/// Front breadcrumb injected after the system messages so the model SEES that
/// earlier turns were cut and cannot confabulate dropped work as present.
pub fn breadcrumb() -> ChatMessage {
    ChatMessage::user(crate::i18n::get_required_cli_string("history-trim-breadcrumb").as_str())
}

/// Insert the trim breadcrumb after the leading system messages, unless one is
/// already sitting there.
pub fn insert_breadcrumb_deduped(history: &mut Vec<ChatMessage>) {
    let system_count = history.iter().take_while(|m| is_system(m)).count();
    let crumb = breadcrumb();
    let already_present = history
        .get(system_count)
        .is_some_and(|m| m.role == crumb.role && m.content == crumb.content);
    if already_present {
        return;
    }
    history.insert(system_count, crumb);
}

/// Outcome of trimming durable [`ConversationMessage`] history. Mirrors
/// [`TrimResult`] accounting so Agent / session / UI stay aligned with the
/// provider-facing cascade.
#[derive(Debug, Clone)]
pub struct ConversationTrimResult {
    pub history: Vec<ConversationMessage>,
    pub dropped_messages: usize,
    pub dropped_turns: usize,
    pub kept_turns: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub trimmed: bool,
    pub exceeds_budget: bool,
    pub below_preferred_keep: bool,
}

fn is_conversation_system(msg: &ConversationMessage) -> bool {
    matches!(msg, ConversationMessage::Chat(m) if m.role == "system")
}

fn is_conversation_turn_boundary(msg: &ConversationMessage) -> bool {
    match msg {
        ConversationMessage::Chat(m) => is_turn_boundary(m),
        ConversationMessage::AssistantToolCalls { .. } | ConversationMessage::ToolResults(_) => {
            false
        }
    }
}

fn estimate_conversation_tokens(history: &[ConversationMessage]) -> usize {
    // Price via the same script-aware heuristic as ChatMessage so durable and
    // provider estimates stay aligned.
    use crate::agent::history::estimate_text_tokens;
    history
        .iter()
        .map(|msg| match msg {
            ConversationMessage::Chat(m) => estimate_message_tokens(m),
            ConversationMessage::AssistantToolCalls {
                text,
                tool_calls,
                reasoning_content,
            } => {
                let mut n = 4usize;
                if let Some(t) = text {
                    n = n.saturating_add(estimate_text_tokens(t));
                }
                if let Some(r) = reasoning_content {
                    n = n.saturating_add(estimate_text_tokens(r));
                }
                for call in tool_calls {
                    n = n
                        .saturating_add(estimate_text_tokens(&call.name))
                        .saturating_add(estimate_text_tokens(&call.arguments))
                        .saturating_add(4);
                }
                n
            }
            ConversationMessage::ToolResults(results) => results
                .iter()
                .map(|r| estimate_text_tokens(&r.content).saturating_add(4))
                .sum(),
        })
        .sum()
}

fn count_conversation_turns(history: &[ConversationMessage]) -> usize {
    history
        .iter()
        .filter(|m| is_conversation_turn_boundary(m))
        .count()
}

/// Drop oldest whole turns from durable conversation history, retaining the
/// `keep_turns` most recent whole turns plus every leading system chat message.
/// Same primitive as [`trim_to_recent_turns`], for [`ConversationMessage`].
pub fn trim_conversation_to_recent_turns(
    history: Vec<ConversationMessage>,
    keep_turns: usize,
) -> ConversationTrimResult {
    let keep = keep_turns.max(1);
    let total_turns = count_conversation_turns(&history);
    let tokens_before = estimate_conversation_tokens(&history);

    let leading_system = history
        .iter()
        .take_while(|m| is_conversation_system(m))
        .count();
    let system: Vec<ConversationMessage> = history[..leading_system].to_vec();
    let body = &history[leading_system..];

    let boundaries: Vec<usize> = body
        .iter()
        .enumerate()
        .filter(|(_, m)| is_conversation_turn_boundary(m))
        .map(|(i, _)| i)
        .collect();

    if boundaries.len() <= keep {
        return ConversationTrimResult {
            history,
            dropped_messages: 0,
            dropped_turns: 0,
            kept_turns: total_turns,
            tokens_before,
            tokens_after: tokens_before,
            trimmed: false,
            exceeds_budget: false,
            below_preferred_keep: false,
        };
    }

    let start = boundaries[boundaries.len() - keep];
    let dropped_messages = start;
    let dropped_turns = boundaries.iter().filter(|&&b| b < start).count();
    let mut kept = system;
    kept.extend_from_slice(&body[start..]);
    let kept_turns = total_turns - dropped_turns;
    let tokens_after = estimate_conversation_tokens(&kept);

    ConversationTrimResult {
        history: kept,
        dropped_messages,
        dropped_turns,
        kept_turns,
        tokens_before,
        tokens_after,
        trimmed: true,
        exceeds_budget: false,
        below_preferred_keep: false,
    }
}

/// Durable conversation cascade — same stages as [`trim_to_budget`].
pub fn trim_conversation_to_budget(
    history: Vec<ConversationMessage>,
    preferred_keep: usize,
    send_budget: usize,
) -> ConversationTrimResult {
    trim_conversation_to_budget_with(
        history,
        preferred_keep,
        send_budget,
        estimate_conversation_tokens,
    )
}

/// Like [`trim_conversation_to_budget`], but sizes history with `estimate`
/// (e.g. provider-view estimate via `to_provider_messages`).
///
/// Cascade probing prices each message once via `estimate(&[msg])` and selects
/// keep with suffix sums — no full-history clone per keep step.
pub fn trim_conversation_to_budget_with(
    history: Vec<ConversationMessage>,
    preferred_keep: usize,
    send_budget: usize,
    estimate: impl Fn(&[ConversationMessage]) -> usize,
) -> ConversationTrimResult {
    let tokens_before = estimate(&history);
    let preferred = preferred_keep.max(1);
    let chosen_keep = choose_keep_turns_for_budget(
        &history,
        preferred,
        send_budget,
        |m| estimate(std::slice::from_ref(m)),
        |m| is_conversation_system(m),
        |m| is_conversation_turn_boundary(m),
    );

    let mut result = trim_conversation_to_recent_turns(history, chosen_keep);
    result.tokens_before = tokens_before;
    result.tokens_after = estimate(&result.history);
    result.exceeds_budget = send_budget > 0 && result.tokens_after > send_budget;
    result.below_preferred_keep =
        result.trimmed && !result.exceeds_budget && result.kept_turns < preferred;
    result
}

fn warn_below_preferred_keep_inner(
    preferred_keep: usize,
    send_budget: usize,
    below_preferred_keep: bool,
    kept_turns: usize,
    tokens_after: usize,
) {
    if !below_preferred_keep {
        return;
    }
    ::zeroclaw_log::record!(
        WARN,
        ::zeroclaw_log::Event::new(module_path!(), ::zeroclaw_log::Action::Note)
            .with_category(::zeroclaw_log::EventCategory::Agent)
            .with_attrs(::serde_json::json!({
                "preferred_keep": preferred_keep,
                "kept_turns": kept_turns,
                "send_budget": send_budget,
                "tokens_after": tokens_after,
                "error_key": "context_trim_below_preferred_keep",
            })),
        format!(
            "Context trim kept {kept_turns} turn(s), below preferred keep_recent_turns={preferred_keep}, to fit send budget {send_budget}"
        )
    );
}

/// Log a warning when cascade kept fewer turns than the preferred keep.
pub fn warn_if_below_preferred_keep(
    preferred_keep: usize,
    send_budget: usize,
    result: &TrimResult,
) {
    warn_below_preferred_keep_inner(
        preferred_keep,
        send_budget,
        result.below_preferred_keep,
        result.kept_turns,
        result.tokens_after,
    );
}

/// Durable-history variant of [`warn_if_below_preferred_keep`].
pub fn warn_if_conversation_below_preferred_keep(
    preferred_keep: usize,
    send_budget: usize,
    result: &ConversationTrimResult,
) {
    warn_below_preferred_keep_inner(
        preferred_keep,
        send_budget,
        result.below_preferred_keep,
        result.kept_turns,
        result.tokens_after,
    );
}

/// Insert the trim breadcrumb after leading system conversation messages.
pub fn insert_conversation_breadcrumb_deduped(history: &mut Vec<ConversationMessage>) {
    let system_count = history
        .iter()
        .take_while(|m| is_conversation_system(m))
        .count();
    let crumb = breadcrumb();
    let already_present = history.get(system_count).is_some_and(|m| {
        matches!(
            m,
            ConversationMessage::Chat(c) if c.role == crumb.role && c.content == crumb.content
        )
    });
    if already_present {
        return;
    }
    history.insert(system_count, ConversationMessage::Chat(crumb));
}

/// Build a [`zeroclaw_api::agent::TurnEvent::HistoryTrimmed`] from trim accounting.
#[must_use]
pub fn history_trimmed_turn_event(
    dropped_messages: usize,
    kept_turns: usize,
    reason: String,
    tokens_after: Option<usize>,
    tokens_before: Option<usize>,
    dropped_turns: Option<usize>,
) -> zeroclaw_api::agent::TurnEvent {
    zeroclaw_api::agent::TurnEvent::HistoryTrimmed {
        dropped_messages,
        kept_turns,
        reason,
        tokens_after,
        tokens_before,
        dropped_turns,
    }
}

/// Build an observer HistoryTrimmed event from trim accounting.
#[must_use]
pub fn history_trimmed_observer_event(
    dropped_messages: usize,
    kept_turns: usize,
    reason: String,
    tokens_after: Option<usize>,
    tokens_before: Option<usize>,
    dropped_turns: Option<usize>,
) -> zeroclaw_api::observability_traits::ObserverEvent {
    zeroclaw_api::observability_traits::ObserverEvent::HistoryTrimmed {
        dropped_messages,
        kept_turns,
        reason,
        tokens_after,
        tokens_before,
        dropped_turns,
        channel: None,
        agent_alias: None,
        turn_id: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sys(c: &str) -> ChatMessage {
        ChatMessage::system(c)
    }
    fn user(c: &str) -> ChatMessage {
        ChatMessage::user(c)
    }
    fn asst(c: &str) -> ChatMessage {
        ChatMessage::assistant(c)
    }
    fn tool(c: &str) -> ChatMessage {
        ChatMessage::tool(c)
    }

    fn first_kept_user_content(result: &TrimResult) -> String {
        result
            .history
            .iter()
            .find(|m| is_turn_boundary(m))
            .expect("kept history must retain at least one turn boundary")
            .content
            .clone()
    }

    // ── ContextCalibration ───────────────────────────────────────────

    #[test]
    fn calibration_without_anchor_falls_back_to_estimate() {
        let hist = vec![sys("base"), user("hello there"), asst("hi")];
        let cal = ContextCalibration::new();
        assert_eq!(cal.current(&hist), estimate_history_tokens(&hist));
    }

    #[test]
    fn calibration_anchors_on_reported_and_prices_the_unbilled_tail() {
        // Provider saw a 1_000-token prompt for a 2-message history.
        let mut hist = vec![sys("base"), user("first")];
        let mut cal = ContextCalibration::new();
        cal.observe_reported(1_000, &hist);
        // No growth yet: exactly the authoritative anchor.
        assert_eq!(cal.current(&hist), 1_000);
        // The assistant reply + tool result are unbilled: priced locally and
        // added on top of the anchor, never re-estimating the anchored prefix.
        hist.push(asst("reply body"));
        hist.push(tool("tool output"));
        let expected = 1_000
            + estimate_message_tokens(&asst("reply body"))
            + estimate_message_tokens(&tool("tool output"));
        assert_eq!(cal.current(&hist), expected);
    }

    #[test]
    fn calibration_ignores_the_anchored_prefix_in_the_estimate() {
        // A large anchored prompt dwarfs the local estimate of the same bytes;
        // `current` must trust the anchor, not re-estimate the prefix.
        let big = "x".repeat(400_000); // ~100k tokens by the /4 heuristic
        let mut hist = vec![sys("base"), user(&big)];
        let mut cal = ContextCalibration::new();
        cal.observe_reported(1_000, &hist); // provider says the prompt is tiny
        hist.push(asst("ok"));
        let current = cal.current(&hist);
        assert!(
            current < estimate_history_tokens(&hist) / 2,
            "anchored current ({current}) must be far below the naive whole-history estimate ({})",
            estimate_history_tokens(&hist)
        );
    }

    #[test]
    fn reanchor_after_trim_never_double_counts() {
        let mut hist = vec![sys("base"), user("a"), asst("b"), user("c"), asst("d")];
        let mut cal = ContextCalibration::new();
        cal.observe_reported(5_000, &hist);
        hist.push(tool("tail"));
        // Trim drops older turns AND the anchor is refreshed to the post-trim
        // reported total with the current length — the sim of a re-anchor.
        hist = vec![sys("base"), user("c"), asst("d"), tool("tail")];
        cal.observe_reported(2_000, &hist);
        assert_eq!(cal.current(&hist), 2_000);
    }

    // ── trim_to_recent_turns: keep N whole turns ─────────────────────

    #[test]
    fn under_keep_is_untouched() {
        let h = vec![sys("s"), user("t1"), asst("a1"), user("t2"), asst("a2")];
        let n = h.len();
        let r = trim_to_recent_turns(h, 5);
        assert!(!r.trimmed);
        assert_eq!(r.history.len(), n);
        assert_eq!(r.dropped_turns, 0);
    }

    #[test]
    fn exactly_at_keep_is_untouched() {
        // 2 turns, keep 2 → the body already fits, nothing to drop.
        let h = vec![sys("s"), user("t1"), asst("a1"), user("t2"), asst("a2")];
        let r = trim_to_recent_turns(h, 2);
        assert!(!r.trimmed);
    }

    #[test]
    fn single_turn_never_trimmed() {
        let h = vec![sys("s"), user("only"), asst("a")];
        let r = trim_to_recent_turns(h, 1);
        assert!(!r.trimmed);
    }

    #[test]
    fn zero_keep_normalises_to_one_and_keeps_last_turn() {
        let h = vec![
            sys("s"),
            user("t1"),
            asst("a1"),
            user("t2"),
            asst("a2"),
            user("t3"),
            asst("a3"),
        ];
        let r = trim_to_recent_turns(h, 0);
        assert!(r.trimmed);
        assert_eq!(r.kept_turns, 1);
        assert_eq!(r.dropped_turns, 2);
        assert_eq!(first_kept_user_content(&r), "t3");
    }

    #[test]
    fn keeps_most_recent_keep_turns() {
        let h = vec![
            sys("s"),
            user("t1"),
            asst("a1"),
            user("t2"),
            asst("a2"),
            user("t3"),
            asst("a3"),
            user("t4"),
            asst("a4"),
        ];
        let r = trim_to_recent_turns(h, 2);
        assert!(r.trimmed);
        assert_eq!(r.dropped_turns, 2);
        assert_eq!(r.kept_turns, 2);
        assert_eq!(first_kept_user_content(&r), "t3");
        assert!(r.history.iter().any(|m| m.content == "t4"));
        assert!(
            !r.history
                .iter()
                .any(|m| m.content == "t1" || m.content == "t2"),
            "dropped turns must be gone"
        );
    }

    #[test]
    fn keeps_leading_system_after_trim() {
        let h = vec![sys("sysA"), user("t1"), asst("a1"), user("t2"), asst("a2")];
        let r = trim_to_recent_turns(h, 1);
        assert!(r.trimmed);
        assert_eq!(r.history[0].role, "system");
        assert_eq!(r.history[0].content, "sysA");
        assert_eq!(first_kept_user_content(&r), "t2");
    }

    #[test]
    fn tool_results_pseudo_user_is_not_a_turn_boundary() {
        // A "[Tool results]" user message is a continuation, not a new turn.
        // Real turns are [A, B]; keep 1 must drop A together with its
        // trailing "[Tool results]" tail, keeping only B.
        let h = vec![
            sys("s"),
            user("turnA"),
            asst("calling"),
            user("[Tool results]\nresult of A"),
            asst("doneA"),
            user("turnB"),
            asst("doneB"),
        ];
        let r = trim_to_recent_turns(h, 1);
        assert!(r.trimmed);
        assert_eq!(r.kept_turns, 1);
        assert_eq!(first_kept_user_content(&r), "turnB");
        assert!(
            !r.history
                .iter()
                .any(|m| m.content.starts_with("[Tool results]")),
            "the dropped turn's tool-results tail must go with it"
        );
    }

    #[test]
    fn never_splits_tool_pair() {
        // The retained turn carries the tool pair; whole-turn slicing keeps
        // the tool row together with its turn head.
        let h = vec![
            sys("s"),
            user("old"),
            asst("a1"),
            user("recent"),
            asst("calling tool"),
            tool("call_1 result"),
            asst("done"),
        ];
        let r = trim_to_recent_turns(h, 1);
        assert!(r.trimmed);
        let mut seen_user = false;
        for m in &r.history {
            if is_turn_boundary(m) {
                seen_user = true;
            }
            if m.role == "tool" {
                assert!(seen_user, "tool result kept without its turn head");
            }
        }
        assert!(
            r.history.iter().any(|m| m.role == "tool"),
            "the recent turn's tool row must survive"
        );
    }

    #[test]
    fn oversized_retained_turn_is_still_kept_by_design() {
        // keep_recent_turns ignores the budget: one oversized newest turn is
        // retained even though it alone would exceed any provider window.
        let huge = "z".repeat(10_000);
        let h = vec![
            sys("system"),
            user("old"),
            asst("a"),
            user(&format!("recent {huge}")),
            asst("a2"),
        ];
        let r = trim_to_recent_turns(h, 1);
        assert!(r.trimmed);
        assert!(r.kept_turns >= 1);
        assert!(r.history.iter().any(|m| m.content.contains("recent")));
    }

    #[test]
    fn trimmed_reports_token_reduction() {
        let big = "x".repeat(2000);
        let h = vec![
            sys("system"),
            user(&format!("t1 {big}")),
            asst("a1"),
            user(&format!("t2 {big}")),
            asst("a2"),
            user("t3 short"),
            asst("a3"),
        ];
        let r = trim_to_recent_turns(h, 1);
        assert!(r.trimmed);
        assert!(r.tokens_before > r.tokens_after);
    }

    #[test]
    fn untouched_reports_equal_before_after() {
        let h = vec![sys("s"), user("hi"), asst("yo")];
        let r = trim_to_recent_turns(h, 5);
        assert!(!r.trimmed);
        assert_eq!(r.tokens_before, r.tokens_after);
    }

    #[test]
    fn dropped_messages_counts_all_dropped_body_rows() {
        // Dropped turn = user + assistant pair; dropping 2 turns removes 4
        // non-system rows before the kept boundary.
        let h = vec![
            sys("s"),
            user("t1"),
            asst("a1"),
            user("t2"),
            asst("a2"),
            user("t3"),
            asst("a3"),
        ];
        let r = trim_to_recent_turns(h, 1);
        assert!(r.trimmed);
        assert_eq!(r.dropped_messages, 4);
    }

    // ── breadcrumb ────────────────────────────────────────────────────

    #[test]
    fn breadcrumb_is_user_role() {
        assert_eq!(breadcrumb().role, "user");
    }

    #[test]
    fn insert_breadcrumb_deduped_does_not_stack() {
        let mut h = vec![sys("system"), user("turn1"), asst("a1")];
        insert_breadcrumb_deduped(&mut h);
        let after_first = h.len();
        insert_breadcrumb_deduped(&mut h);
        assert_eq!(
            h.len(),
            after_first,
            "a second trim must not stack another breadcrumb behind the system block"
        );
        let crumbs = h
            .iter()
            .filter(|m| m.role == breadcrumb().role && m.content == breadcrumb().content)
            .count();
        assert_eq!(crumbs, 1);
    }

    #[test]
    fn insert_breadcrumb_deduped_sits_after_leading_system() {
        let mut h = vec![sys("s1"), sys("s2"), user("turn1"), asst("a1")];
        insert_breadcrumb_deduped(&mut h);
        assert_eq!(h[0].role, "system");
        assert_eq!(h[1].role, "system");
        assert_eq!(h[2].role, breadcrumb().role);
        assert_eq!(h[2].content, breadcrumb().content);
    }

    // ── ConversationMessage trim ─────────────────────────────────────

    fn conv_sys(c: &str) -> ConversationMessage {
        ConversationMessage::Chat(sys(c))
    }
    fn conv_user(c: &str) -> ConversationMessage {
        ConversationMessage::Chat(user(c))
    }
    fn conv_asst(c: &str) -> ConversationMessage {
        ConversationMessage::Chat(asst(c))
    }

    #[test]
    fn conversation_trim_keeps_tool_structured_turn_intact() {
        let h = vec![
            conv_sys("s"),
            conv_user("old"),
            conv_asst("a1"),
            conv_user("recent"),
            ConversationMessage::AssistantToolCalls {
                text: Some("calling".into()),
                tool_calls: vec![zeroclaw_providers::ToolCall {
                    id: "1".into(),
                    name: "shell".into(),
                    arguments: "{}".into(),
                    extra_content: None,
                }],
                reasoning_content: None,
            },
            ConversationMessage::ToolResults(vec![zeroclaw_providers::ToolResultMessage {
                tool_call_id: "1".into(),
                content: "ok".into(),
                tool_name: "shell".into(),
            }]),
            conv_asst("done"),
        ];
        let r = trim_conversation_to_recent_turns(h, 1);
        assert!(r.trimmed);
        assert_eq!(r.kept_turns, 1);
        assert!(matches!(
            r.history
                .iter()
                .find(|m| matches!(m, ConversationMessage::AssistantToolCalls { .. })),
            Some(ConversationMessage::AssistantToolCalls { .. })
        ));
        assert!(matches!(
            r.history
                .iter()
                .find(|m| matches!(m, ConversationMessage::ToolResults(_))),
            Some(ConversationMessage::ToolResults(_))
        ));
        assert!(!r.history.iter().any(|m| matches!(
            m,
            ConversationMessage::Chat(c) if c.content == "old"
        )));
    }

    #[test]
    fn conversation_trim_under_keep_untouched() {
        let h = vec![
            conv_sys("s"),
            conv_user("t1"),
            conv_asst("a1"),
            conv_user("t2"),
            conv_asst("a2"),
        ];
        let n = h.len();
        let r = trim_conversation_to_recent_turns(h, 5);
        assert!(!r.trimmed);
        assert_eq!(r.history.len(), n);
    }

    // ── resolve_send_budget ──────────────────────────────────────────

    #[test]
    fn resolve_send_budget_prefers_explicit_send_budget() {
        assert_eq!(resolve_send_budget(500, 10_000), 500);
    }

    #[test]
    fn resolve_send_budget_falls_back_to_token_budget_when_send_unset() {
        assert_eq!(resolve_send_budget(0, 10_000), 10_000);
    }

    // ── trim_to_budget cascade ───────────────────────────────────────

    fn multi_turn_history(turns: usize, body: &str) -> Vec<ChatMessage> {
        let mut h = vec![sys("system")];
        for i in 0..turns {
            h.push(user(&format!("t{i} {body}")));
            h.push(asst(&format!("a{i} {body}")));
        }
        h
    }

    #[test]
    fn trim_to_budget_uses_send_budget_not_token_budget() {
        let big = "x".repeat(800);
        let h = multi_turn_history(6, &big);
        let keep2 = trim_to_recent_turns(h.clone(), 2);
        let send_budget = keep2.tokens_after;
        let token_budget = send_budget.saturating_mul(4);

        let send_trim = trim_to_budget(h.clone(), 5, send_budget);
        let token_trim = trim_to_budget(h, 5, token_budget);

        assert!(send_trim.trimmed);
        assert!(send_trim.tokens_after <= send_budget);
        assert!(
            send_trim.kept_turns <= token_trim.kept_turns,
            "a tighter send budget must retain no more turns than a looser fit budget"
        );
        assert!(
            send_trim.kept_turns < 5 || send_trim.tokens_after < token_trim.tokens_after,
            "send budget must drive the cascade, not the water-line token budget"
        );
    }

    #[test]
    fn budget_cascade_stops_at_preferred_when_under_budget() {
        let h = multi_turn_history(8, "short");
        let after_keep5 = trim_to_recent_turns(h.clone(), 5);
        assert!(after_keep5.trimmed);
        let fit = after_keep5.tokens_after;
        let r = trim_to_budget(h, 5, fit);
        assert!(r.trimmed);
        assert_eq!(r.kept_turns, 5);
        assert!(!r.exceeds_budget);
        assert!(r.tokens_after <= fit);
    }

    #[test]
    fn budget_cascade_drops_below_preferred_until_fit() {
        let big = "x".repeat(800);
        let h = multi_turn_history(6, &big);
        // Prefer 5; force a budget that only ~2 newest turns can satisfy.
        let keep2 = trim_to_recent_turns(h.clone(), 2);
        let fit = keep2.tokens_after;
        let r = trim_to_budget(h, 5, fit);
        assert!(r.trimmed);
        assert_eq!(r.kept_turns, 2);
        assert!(r.below_preferred_keep);
        assert!(!r.exceeds_budget);
        assert!(r.tokens_after <= fit);
    }

    #[test]
    fn budget_cascade_never_goes_below_one_turn() {
        let huge = "z".repeat(20_000);
        let h = multi_turn_history(4, &huge);
        let r = trim_to_budget(h, 5, 10);
        assert_eq!(r.kept_turns, 1);
        assert!(r.exceeds_budget);
        assert!(r.history.iter().any(|m| m.content.contains("t3")));
        assert!(!r.history.iter().any(|m| m.content.contains("t0")));
    }

    #[test]
    fn budget_cascade_oversized_single_turn_exceeds() {
        let huge = "z".repeat(20_000);
        let h = vec![sys("s"), user(&format!("only {huge}")), asst("a")];
        let r = trim_to_budget(h, 5, 50);
        assert!(!r.trimmed);
        assert_eq!(r.kept_turns, 1);
        assert!(r.exceeds_budget);
    }

    #[test]
    fn conversation_budget_cascade_matches_chat_semantics() {
        let big = "y".repeat(600);
        let h = vec![
            conv_sys("s"),
            conv_user(&format!("t0 {big}")),
            conv_asst("a0"),
            conv_user(&format!("t1 {big}")),
            conv_asst("a1"),
            conv_user(&format!("t2 {big}")),
            conv_asst("a2"),
            conv_user("t3 short"),
            conv_asst("a3"),
        ];
        let keep1 = trim_conversation_to_recent_turns(h.clone(), 1);
        let fit = keep1.tokens_after;
        let r = trim_conversation_to_budget(h, 5, fit);
        assert!(r.trimmed);
        assert_eq!(r.kept_turns, 1);
        assert!(!r.exceeds_budget);
    }
}
