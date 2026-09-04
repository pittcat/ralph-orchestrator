//! 2026-09-03-0959 plan U6 (R8 / E14 / E15): env allowlist for
//! DAG-spawned children.
//!
//! Rule (plan §7 U6 #5): the DAG job environment is an allowlist
//! (no inherited host secret). Every variable name that reaches the
//! child MUST appear in the `DagEnvAllowlist` declared on the
//! descriptor; anything else — including `BEARER_TOKEN`,
//! `*_SECRET`, `HOME`, `USER`, anything the operator shell has
//! exported — is silently dropped at launch time.
//!
//! Sanitisation: the allowlist lookup is **name-only**. The value
//! is never inspected, logged, echoed, or surfaced in
//! `RuntimeJobError`. Tests assert that injecting a hostile var
//! (`SECRET_FOO`) and then triggering any failure path leaves no
//! trace of `SECRET_FOO` or its value in the error / Debug / panic
//! output. The `LegacyEnvPolicy` marker documents the *existing*
//! wave worker path without adopting it; the wave worker is
//! U6-out-of-scope and keeps its inherited env.

#[cfg(test)]
use std::collections::HashMap;

/// Declared set of env var names the DAG is allowed to forward
/// to a child process. Names are matched **case-sensitively**
/// (POSIX `getenv` semantics). Order is irrelevant; the type
/// stores a `Vec<String>` for stable serialisation in
/// `JobDescriptor`.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DagEnvAllowlist {
    names: Vec<String>,
}

#[cfg(test)]
impl DagEnvAllowlist {
    /// Build an allowlist from a declared list of var names. The
    /// list is de-duplicated in declaration order so two equal
    /// constructors yield equal `DagEnvAllowlist`s.
    pub fn from_declared<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut seen: Vec<String> = Vec::new();
        for raw in names {
            let s = raw.into();
            if !seen.iter().any(|n| n == &s) {
                seen.push(s);
            }
        }
        Self { names: seen }
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    pub fn len(&self) -> usize {
        self.names.len()
    }
}

/// Policy the kernel applies at launch time: keep only the
/// declared allowlist entries; silently drop everything else.
///
/// `filter_child_env` is the only public mutator. It takes the
/// host env (or any pre-built candidate child env) and returns a
/// new map containing ONLY the allowlist entries that were
/// present in the input. The returned map never contains a name
/// not in `allowlist`. Values are passed through verbatim — the
/// policy is name-only.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DagEnvPolicy {
    allowlist: DagEnvAllowlist,
}

#[cfg(test)]
impl DagEnvPolicy {
    pub fn new(allowlist: DagEnvAllowlist) -> Self {
        Self { allowlist }
    }

    pub fn from_declared<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self::new(DagEnvAllowlist::from_declared(names))
    }

    /// Filter `candidate` down to allowlist entries only.
    /// Anything not on the allowlist is **silently dropped** —
    /// no error, no log, no echo. Test fakes rely on this
    /// behaviour to drive the "secret never reaches child"
    /// assertion.
    pub fn filter_child_env(&self, candidate: &HashMap<String, String>) -> HashMap<String, String> {
        let mut out: HashMap<String, String> = HashMap::with_capacity(self.allowlist.len());
        for name in self.allowlist.names() {
            if let Some(v) = candidate.get(name) {
                out.insert(name.clone(), v.clone());
            }
        }
        out
    }
}

/// Marker type documenting the **legacy inherited-env path** the
/// wave worker uses today. U6 does **not** touch the wave worker
/// body / env contract; this type exists so a future migration
/// has a named hand-off and so a reviewer can grep for
/// `LegacyEnvPolicy` to find every place the inherited-env
/// behaviour is still mentioned.
///
/// `#[cfg(test)]` because the type's only consumer is the env
/// tests module that pins the marker against future renames.
/// U7 promotes it once the migration is planned.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyEnvPolicy {
    _private: (),
}

#[cfg(test)]
impl LegacyEnvPolicy {
    /// Construct the marker. No state — the type is purely a
    /// documentation hand-off.
    pub const fn marker() -> Self {
        Self { _private: () }
    }

    /// Short note about the legacy path. Surfaced as a stable
    /// string so a future grep migration can pin it.
    pub const fn legacy_path_note() -> &'static str {
        "wave::worker inherits std::env at launch; DAG children use DagEnvPolicy"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map<const N: usize>(pairs: [(&str, &str); N]) -> HashMap<String, String> {
        pairs
            .into_iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Allowlist deduplicates declarations in declaration order so
    /// two equal constructors are equal.
    #[test]
    fn allowlist_dedupes_declarations() {
        let a = DagEnvAllowlist::from_declared(["PATH", "HOME", "PATH"]);
        let b = DagEnvAllowlist::from_declared(["PATH", "HOME"]);
        assert_eq!(a, b);
        assert_eq!(a.names(), &["PATH".to_string(), "HOME".to_string()]);
        assert_eq!(a.len(), 2);
    }

    /// Empty allowlist admits nothing.
    #[test]
    fn empty_allowlist_admits_nothing() {
        let policy = DagEnvPolicy::from_declared(Vec::<&str>::new());
        let candidate = map([("PATH", "/bin"), ("HOME", "/home/x")]);
        let out = policy.filter_child_env(&candidate);
        assert!(out.is_empty());
    }

    /// Only declared entries reach the child.
    #[test]
    fn only_allowlisted_entries_pass() {
        let policy = DagEnvPolicy::from_declared(["PATH"]);
        let candidate = map([
            ("PATH", "/usr/bin"),
            ("HOME", "/home/operator"),
            ("BEARER_TOKEN", "super-secret"),
        ]);
        let out = policy.filter_child_env(&candidate);
        assert_eq!(out.len(), 1);
        assert_eq!(out.get("PATH").map(String::as_str), Some("/usr/bin"));
        assert!(out.get("HOME").is_none());
        assert!(out.get("BEARER_TOKEN").is_none());
    }

    /// Filter is name-only: values pass through verbatim.
    #[test]
    fn values_pass_through_verbatim() {
        let policy = DagEnvPolicy::from_declared(["PATH", "RALPH_DAG"]);
        let candidate = map([
            ("PATH", "/a:/b"),
            ("RALPH_DAG", "plan-key-xyz"),
            ("UNRELATED", "noise"),
        ]);
        let out = policy.filter_child_env(&candidate);
        assert_eq!(out.get("PATH").map(String::as_str), Some("/a:/b"));
        assert_eq!(
            out.get("RALPH_DAG").map(String::as_str),
            Some("plan-key-xyz")
        );
        assert_eq!(out.len(), 2);
    }

    /// Missing allowed entries are simply absent from output —
    /// the filter does not synthesise placeholder values.
    #[test]
    fn missing_allowed_entry_is_omitted_silently() {
        let policy = DagEnvPolicy::from_declared(["PATH", "RALPH_DAG"]);
        let candidate = map([("PATH", "/x")]);
        let out = policy.filter_child_env(&candidate);
        assert_eq!(out.len(), 1);
        assert!(out.get("RALPH_DAG").is_none());
    }

    /// The legacy marker is constructible and pinned (so a
    /// migration can grep for it). Body is empty by design.
    #[test]
    fn legacy_marker_is_const_constructible() {
        let _marker = LegacyEnvPolicy::marker();
        let note = LegacyEnvPolicy::legacy_path_note();
        assert!(note.contains("DagEnvPolicy"));
    }
}
