//! Whole-turn history trimming. One rule: when a trim fires, keep the most
//! recent N whole turns (plus every leading system message) and drop the rest,
//! never cutting a turn in half. The token water-line decides *when* to trim;
//! `keep_turns` decides *what stays* — the retained size is not itself a budget.

use crate::agent::history::{estimate_history_tokens, estimate_message_tokens};
use zeroclaw_providers::ChatMessage;

const TOOL_RESULTS_PREFIX: &str = "[Tool results]";

/// Outcome of a trim pass. `trimmed` is true only when at least one whole turn
/// was dropped, in which case the caller emits a user-visible event and injects
/// a breadcrumb so the loss is never silent.
#[derive(Debug, Clone)]
pub struct TrimResult {
    pub history: Vec<ChatMessage>,
    pub dropped_messages: usize,
    pub dropped_turns: usize,
    pub kept_turns: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub trimmed: bool,
}

/// Provider-authoritative context size, replacing the bare `len()/4 + 4`
/// estimate wherever a trim decision is made.
///
/// A provider reports the prompt size of a request **as it was sent**. Between
/// that report and the next trim check, the loop appends messages the provider
/// has not billed yet — the assistant reply and the tool results it just
/// produced. Those are unbilled and must be priced back with the local
/// heuristic. This is the Rust analogue of omp's
/// `calculateContextTokens` (`contextTokens − orchestration`): the reported
/// number is the authoritative anchor, the local estimate only prices the
/// un-billed tail.
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
/// turns plus every leading system message. This is the sole trim *action*:
/// the trigger (token water-line) lives in the pre-send kernel, and once a
/// trim fires it always keeps exactly `keep_turns` newest turns regardless of
/// the resulting token count — a single oversized retained turn may still be
/// too big for the provider, by design. When the body already fits within
/// `keep_turns` turns (or there is nothing to drop) the history is returned
/// untouched.
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
    }
}

fn count_turns(history: &[ChatMessage]) -> usize {
    history.iter().filter(|m| is_turn_boundary(m)).count()
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
            !r.history.iter().any(|m| m.content == "t1" || m.content == "t2"),
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
            !r.history.iter().any(|m| m.content.starts_with("[Tool results]")),
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
}
