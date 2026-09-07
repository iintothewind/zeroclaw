//! Stable-prefix freezing.
//!
//! The system prompt and tool set are the leading, cacheable bytes of every
//! provider request. [`StablePrefix`] snapshots them once and hands back the
//! *same* value on every subsequent turn, so those bytes never change under the
//! provider's prefix cache — until the live state genuinely changes (a new
//! fingerprint) or [`StablePrefix::invalidate`] is called.

/// A frozen, fingerprinted snapshot of the system prompt + tool set.
///
/// `P` is the prefix payload (the system prompt and tool specs together, in
/// whatever shape the runtime serializes them). Generic so this crate stays
/// decoupled from the runtime's concrete types; the runtime adapter supplies a
/// `P: Serialize + Clone`.
#[derive(Debug, Clone)]
pub struct StablePrefix<P> {
    snapshot: Option<P>,
    fingerprint: Option<u64>,
    /// Bumped on every rebuild — surfaces can log/cache-miss on version churn.
    version: u64,
}

impl<P> Default for StablePrefix<P> {
    fn default() -> Self {
        Self {
            snapshot: None,
            fingerprint: None,
            version: 0,
        }
    }
}

impl<P: serde::Serialize + Clone> StablePrefix<P> {
    /// A fresh, unbuilt prefix.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a snapshot exists yet.
    #[must_use]
    pub fn is_built(&self) -> bool {
        self.snapshot.is_some()
    }

    /// Rebuild counter — increments each time the prefix actually changed.
    #[must_use]
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The current fingerprint, or `None` before the first build.
    #[must_use]
    pub fn fingerprint(&self) -> Option<u64> {
        self.fingerprint
    }

    /// Snapshot `live` if its fingerprint differs from the frozen one.
    ///
    /// Returns `true` when the prefix actually changed (a cache miss is
    /// imminent), `false` when the frozen copy is reused byte-for-byte. The
    /// clone is a full ownership break: later mutation of the live state can
    /// never leak into the frozen bytes.
    pub fn build(&mut self, live: &P) -> bool {
        let new_fp = fingerprint(live);
        if self.fingerprint == Some(new_fp) {
            return false;
        }
        self.snapshot = Some(live.clone());
        self.fingerprint = Some(new_fp);
        self.version += 1;
        true
    }

    /// Force the next [`build`](Self::build) to rebuild unconditionally — for
    /// MCP tool-set reloads or a model switch, where the frozen bytes are no
    /// longer valid even if a fingerprint happens to collide.
    pub fn invalidate(&mut self) {
        self.snapshot = None;
        self.fingerprint = None;
    }

    /// The frozen prefix. Panics only if queried before the first `build`;
    /// the runtime always builds before reading.
    #[must_use]
    pub fn snapshot(&self) -> &P {
        self.snapshot
            .as_ref()
            .expect("StablePrefix::snapshot called before build")
    }

    /// The frozen prefix, or `None` if never built / invalidated.
    #[must_use]
    pub fn snapshot_opt(&self) -> Option<&P> {
        self.snapshot.as_ref()
    }
}

/// Deterministic fingerprint over a value's canonical serialization. Delegates
/// to the crate-wide [`crate::digest`] so the prefix and the per-message digest
/// share one hash implementation.
fn fingerprint<T: serde::Serialize>(value: &T) -> u64 {
    crate::digest(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, serde::Serialize, PartialEq)]
    struct Prefix {
        system: Vec<String>,
        tools: Vec<String>,
    }

    fn prefix(system: &[&str], tools: &[&str]) -> Prefix {
        Prefix {
            system: system.iter().map(|s| (*s).to_string()).collect(),
            tools: tools.iter().map(|s| (*s).to_string()).collect(),
        }
    }

    #[test]
    fn first_build_snapshots_and_reports_change() {
        let mut sp = StablePrefix::<Prefix>::new();
        assert!(!sp.is_built());
        assert!(sp.build(&prefix(&["sys"], &["read"])));
        assert!(sp.is_built());
        assert_eq!(sp.version(), 1);
        assert_eq!(
            sp.snapshot(),
            &prefix(&["sys"], &["read"]),
            "snapshot must equal the live value it froze"
        );
    }

    #[test]
    fn identical_rebuild_reports_no_change_and_keeps_version() {
        let mut sp = StablePrefix::<Prefix>::new();
        sp.build(&prefix(&["sys"], &["read"]));
        // A logically identical live value (different allocation) reuses the
        // frozen bytes: no version bump, build() returns false.
        assert!(!sp.build(&prefix(&["sys"], &["read"])));
        assert_eq!(sp.version(), 1);
    }

    #[test]
    fn changed_tools_bump_version_and_rebuild() {
        let mut sp = StablePrefix::<Prefix>::new();
        sp.build(&prefix(&["sys"], &["read"]));
        // MCP adding a tool changes the fingerprint → rebuild.
        assert!(sp.build(&prefix(&["sys"], &["read", "write"])));
        assert_eq!(sp.version(), 2);
        assert_eq!(sp.snapshot().tools.len(), 2);
    }

    #[test]
    fn invalidate_forces_rebuild_even_for_identical_value() {
        let mut sp = StablePrefix::<Prefix>::new();
        sp.build(&prefix(&["sys"], &["read"]));
        sp.invalidate();
        assert!(!sp.is_built());
        assert!(sp.fingerprint().is_none());
        // The same value now rebuilds (MCP reconnect semantics), bumping version.
        assert!(sp.build(&prefix(&["sys"], &["read"])));
        assert_eq!(sp.version(), 2);
    }

    #[test]
    fn snapshot_is_decoupled_from_live_mutation() {
        // Mutating the live value AFTER build must not alter frozen bytes — the
        // ownership break the clone provides.
        let mut sp = StablePrefix::<Prefix>::new();
        let mut live = prefix(&["sys"], &["read"]);
        sp.build(&live);
        live.tools.push("write".to_string());
        assert_eq!(
            sp.snapshot().tools,
            vec!["read".to_string()],
            "frozen snapshot must not observe later live mutation"
        );
    }
}
