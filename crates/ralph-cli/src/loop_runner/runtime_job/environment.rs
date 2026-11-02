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

use std::collections::HashMap;

/// Declared set of env var names the DAG is allowed to forward
/// to a child process. Names are matched **case-sensitively**
/// (POSIX `getenv` semantics). Order is irrelevant; the type
/// stores a `Vec<String>` for stable serialisation in
/// `JobDescriptor`.
///
/// Step 1+2(2026-09-03-0959 DAG 接线):promote 为生产可见;无 bin 侧
/// 生产调用方(EventLoop 接线属后续 Step),item 级
/// `#[allow(dead_code)]`——promote 义务见
/// `presets/en/parallel-forge-preset-author-notes.md`「promote 前置
/// 义务清单」#1/#2,接线落地后移除。
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DagEnvAllowlist {
    names: Vec<String>,
}

#[allow(dead_code)] // 同 `DagEnvAllowlist` 的 Step 1+2 promote 注释。
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
///
/// Step 1+2(2026-09-03-0959 DAG 接线):promote 为生产可见;无 bin 侧
/// 生产调用方,item 级 `#[allow(dead_code)]`(同 `DagEnvAllowlist`
/// 的 promote 注释)。
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DagEnvPolicy {
    allowlist: DagEnvAllowlist,
}

#[allow(dead_code)] // 同 `DagEnvAllowlist` 的 Step 1+2 promote 注释。
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
    #[cfg(not(windows))]
    pub fn filter_child_env(&self, candidate: &HashMap<String, String>) -> HashMap<String, String> {
        let mut out: HashMap<String, String> = HashMap::with_capacity(self.allowlist.len());
        for name in self.allowlist.names() {
            if let Some(v) = candidate.get(name) {
                out.insert(name.clone(), v.clone());
            }
        }
        out
    }

    /// Windows variant: names match **case-insensitively** (`Path`
    /// satisfies `PATH`), and a fixed baseline of OS-required
    /// variables is always admitted so children can resolve
    /// executables, write temp files and load system DLLs. Still
    /// name-only — values pass through verbatim, and secrets not on
    /// the baseline are still dropped.
    #[cfg(windows)]
    pub fn filter_child_env(&self, candidate: &HashMap<String, String>) -> HashMap<String, String> {
        use std::collections::HashSet;

        let allowed: HashSet<String> = self
            .allowlist
            .names()
            .iter()
            .map(|n| n.to_uppercase())
            .chain(WINDOWS_BASELINE_VARS.iter().map(|n| n.to_uppercase()))
            .collect();
        candidate
            .iter()
            .filter(|(k, _)| allowed.contains(&k.to_uppercase()))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect()
    }
}

/// Env vars every Windows child needs regardless of the declared
/// allowlist. Chosen conservatively: OS plumbing only, no user
/// secrets. Without `SystemRoot`/`windir` most tools fail to start;
/// without `COMSPEC`/`PATHEXT` `.cmd` resolution breaks; without
/// `TEMP`/`TMP` many libraries panic.
#[cfg(windows)]
const WINDOWS_BASELINE_VARS: &[&str] = &[
    "PATH",
    "PATHEXT",
    "COMSPEC",
    "SystemRoot",
    "windir",
    "TEMP",
    "TMP",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "APPDATA",
    "LOCALAPPDATA",
    "USERNAME",
    "USERDOMAIN",
    "OS",
    "PROCESSOR_ARCHITECTURE",
    "NUMBER_OF_PROCESSORS",
];

/// Marker type documenting the **legacy inherited-env path** the
/// wave worker uses today. U6 does **not** touch the wave worker
/// body / env contract; this type exists so a future migration
/// has a named hand-off and so a reviewer can grep for
/// `LegacyEnvPolicy` to find every place the inherited-env
/// behaviour is still mentioned.
///
/// Step 1+2(2026-09-03-0959 DAG 接线):promote 为生产可见;迁移落地
/// 前无 bin 侧消费方,item 级 `#[allow(dead_code)]`(同
/// `DagEnvAllowlist` 的 promote 注释)。
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LegacyEnvPolicy {
    _private: (),
}

#[allow(dead_code)] // 同 `DagEnvAllowlist` 的 Step 1+2 promote 注释。
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
    #[cfg(not(windows))]
    #[test]
    fn empty_allowlist_admits_nothing() {
        let policy = DagEnvPolicy::from_declared(Vec::<&str>::new());
        let candidate = map([("PATH", "/bin"), ("HOME", "/home/x")]);
        let out = policy.filter_child_env(&candidate);
        assert!(out.is_empty());
    }

    /// Windows: an empty allowlist still admits the OS baseline vars
    /// present in the candidate (PATH is baseline, HOME is not).
    #[cfg(windows)]
    #[test]
    fn empty_allowlist_admits_only_baseline() {
        let policy = DagEnvPolicy::from_declared(Vec::<&str>::new());
        let candidate = map([("PATH", "/bin"), ("HOME", "/home/x")]);
        let out = policy.filter_child_env(&candidate);
        assert_eq!(out.len(), 1);
        assert_eq!(out.get("PATH").map(String::as_str), Some("/bin"));
        assert!(!out.contains_key("HOME"));
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
        assert!(!out.contains_key("HOME"));
        assert!(!out.contains_key("BEARER_TOKEN"));
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
        assert!(!out.contains_key("RALPH_DAG"));
    }

    /// Windows: names match case-insensitively and OS baseline vars
    /// are always admitted; non-baseline secrets are still dropped.
    #[cfg(windows)]
    #[test]
    fn windows_case_insensitive_matching_with_baseline() {
        let policy = DagEnvPolicy::from_declared(["PATH", "RALPH_DAG"]);
        let candidate = map([
            ("Path", "C:\\Windows"),
            ("SystemRoot", "C:\\Windows"),
            ("COMSPEC", "C:\\Windows\\system32\\cmd.exe"),
            ("RALPH_DAG", "key"),
            ("AWS_SECRET", "super-secret"),
        ]);
        let out = policy.filter_child_env(&candidate);
        assert_eq!(out.get("Path").map(String::as_str), Some("C:\\Windows"));
        assert_eq!(
            out.get("SystemRoot").map(String::as_str),
            Some("C:\\Windows")
        );
        assert_eq!(
            out.get("COMSPEC").map(String::as_str),
            Some("C:\\Windows\\system32\\cmd.exe")
        );
        assert_eq!(out.get("RALPH_DAG").map(String::as_str), Some("key"));
        assert!(!out.contains_key("AWS_SECRET"));
    }

    /// The legacy marker is constructible and pinned (so a
    /// migration can grep for it). Body is empty by design.
    #[test]
    fn legacy_marker_is_const_constructible() {
        let _marker = LegacyEnvPolicy::marker();
        let note = LegacyEnvPolicy::legacy_path_note();
        assert!(note.contains("DagEnvPolicy"));
    }

    /// Windows fuzz: for random candidate env sets the output must (a)
    /// contain only allowlist+baseline names matched case-insensitively,
    /// (b) pass values through verbatim, and (c) never leak
    /// non-baseline "secret-looking" names.
    #[cfg(windows)]
    #[test]
    fn windows_filter_fuzz_never_leaks_non_baseline() {
        use std::collections::HashSet;

        struct XorShift(u64);
        impl XorShift {
            fn next(&mut self) -> u64 {
                let mut x = self.0;
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                self.0 = x;
                x
            }
            fn below(&mut self, n: usize) -> usize {
                (self.next() % n as u64) as usize
            }
        }

        let baseline: HashSet<String> = WINDOWS_BASELINE_VARS
            .iter()
            .map(|n| n.to_uppercase())
            .collect();
        let allow = ["RALPH_DAG", "MY_CUSTOM_VAR", "MixedCase"];
        let policy = DagEnvPolicy::from_declared(allow);
        let allowed: HashSet<String> = allow
            .iter()
            .map(|n| n.to_uppercase())
            .chain(baseline.iter().cloned())
            .collect();

        let mut rng = XorShift(0xB529_7A4D_E951_3F17);
        let name_pool = [
            "PATH",
            "Path",
            "path",
            "RALPH_DAG",
            "ralph_dag",
            "MY_CUSTOM_VAR",
            "MixedCase",
            "MIXEDCASE",
            "SystemRoot",
            "systemroot",
            "TEMP",
            "AWS_SECRET_ACCESS_KEY",
            "PRIVATE_TOKEN",
            "MY_SECRET",
            "OS",
            "USERNAME",
            "NOT_ALLOWED",
            "RandomVar",
            "HOMEPATH",
        ];
        let value_pool = ["", "x", "C:\\a;b", "väl üé", "line\nbreak", "{}", "a b c"];

        for _ in 0..3_000 {
            let count = rng.below(name_pool.len());
            let candidate: HashMap<String, String> = (0..count)
                .map(|_| {
                    (
                        name_pool[rng.below(name_pool.len())].to_string(),
                        value_pool[rng.below(value_pool.len())].to_string(),
                    )
                })
                .collect();
            let out = policy.filter_child_env(&candidate);

            // (a) every emitted name is allowlist or baseline (CI match).
            for key in out.keys() {
                assert!(
                    allowed.contains(&key.to_uppercase()),
                    "leaked non-baseline env {key}"
                );
            }
            // (b) values pass verbatim.
            for (k, v) in &out {
                assert_eq!(candidate.get(k), Some(v));
            }
            // (c) explicit secret-looking names never appear.
            for secret in ["AWS_SECRET_ACCESS_KEY", "PRIVATE_TOKEN", "MY_SECRET"] {
                assert!(
                    !out.keys().any(|k| k.eq_ignore_ascii_case(secret)),
                    "secret {secret} leaked"
                );
            }
        }
    }
}
