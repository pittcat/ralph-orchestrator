//! U9 (plan 2026-07-30-004): bounded artifact canonicalizer.
//!
//! Takes a raw artifact (YAML bytes, e.g. `execution-plan.yml`) and
//! produces a canonical, digest-stamped representation. Resource bounds
//! are enforced *before* any expensive work so a hostile or runaway
//! artifact cannot exhaust memory or the dependency graph:
//!
//! - Raw artifact ≤ 1 MiB ([`MAX_ARTIFACT_BYTES`])
//! - ≤ 512 Units ([`MAX_UNITS`])
//! - ≤ 4096 dependency edges ([`MAX_EDGES`])
//!
//! # Determinism
//!
//! The canonical form is independent of incidental input formatting:
//! mapping keys are sorted recursively and the normalized tree is
//! re-serialized, so two artifacts that differ only in key ordering,
//! indentation, or quoting produce the **same** [`CanonicalArtifact::digest`].
//! Sequence order is preserved because it is semantically meaningful.

use serde::{Deserialize, Serialize};
use serde_yaml::{Mapping, Value};
use sha2::{Digest, Sha256};
use std::fmt;

/// Maximum raw artifact size in bytes (1 MiB), per D9 of plan 2026-07-30-004.
///
/// The bound is inclusive: an artifact of exactly this size is accepted.
pub const MAX_ARTIFACT_BYTES: usize = 1024 * 1024;

/// Maximum number of Units an artifact may declare.
///
/// The bound is inclusive: exactly this many Units is accepted.
pub const MAX_UNITS: usize = 512;

/// Maximum number of dependency edges an artifact may declare.
///
/// The bound is inclusive: exactly this many edges is accepted.
pub const MAX_EDGES: usize = 4096;

/// Error returned by [`canonicalize`] when an artifact violates a bound
/// or cannot be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactError {
    /// The raw artifact exceeded [`MAX_ARTIFACT_BYTES`].
    TooLarge {
        /// Observed size in bytes.
        size: usize,
        /// The configured limit ([`MAX_ARTIFACT_BYTES`]).
        limit: usize,
    },
    /// The artifact declared more than [`MAX_UNITS`] Units.
    TooManyUnits {
        /// Observed Unit count.
        count: usize,
        /// The configured limit ([`MAX_UNITS`]).
        limit: usize,
    },
    /// The artifact declared more than [`MAX_EDGES`] dependency edges.
    TooManyEdges {
        /// Observed edge count.
        count: usize,
        /// The configured limit ([`MAX_EDGES`]).
        limit: usize,
    },
    /// The artifact was not valid YAML (or could not be re-serialized).
    ParseError {
        /// The underlying parser/serializer message.
        source: String,
    },
    /// The artifact used YAML anchors or aliases, which are forbidden in
    /// canonical artifacts.
    ///
    /// W6: the YAML parser expands anchors/aliases with no size cap, so a
    /// sub-1-MiB "billion laughs" alias bomb can expand to gigabytes and
    /// exhaust memory *after* the raw-size gate. Rejecting any document
    /// containing an anchor/alias is a fail-closed mitigation (machine-
    /// generated execution-plan artifacts never use them).
    AliasesForbidden,
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArtifactError::TooLarge { size, limit } => write!(
                f,
                "artifact is {size} bytes, exceeding the {limit}-byte (1 MiB) limit"
            ),
            ArtifactError::TooManyUnits { count, limit } => {
                write!(
                    f,
                    "artifact declares {count} units, exceeding the limit of {limit}"
                )
            }
            ArtifactError::TooManyEdges { count, limit } => write!(
                f,
                "artifact declares {count} dependency edges, exceeding the limit of {limit}"
            ),
            ArtifactError::ParseError { source } => {
                write!(f, "failed to parse artifact YAML: {source}")
            }
            ArtifactError::AliasesForbidden => write!(
                f,
                "artifact uses YAML anchors/aliases, which are forbidden in canonical artifacts"
            ),
        }
    }
}

impl std::error::Error for ArtifactError {}

/// A canonical, digest-stamped artifact representation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalArtifact {
    /// SHA-256 hex digest of the canonical byte form.
    pub digest: String,
    /// Number of Units in the artifact.
    pub unit_count: usize,
    /// Number of dependency edges in the artifact.
    pub edge_count: usize,
    /// The canonical YAML bytes (recursively sorted keys, normalized).
    pub canonical_bytes: Vec<u8>,
}

/// Canonicalize a raw artifact.
///
/// Pipeline:
/// 1. Enforce the size bound (≤ 1 MiB) — before any parsing work.
/// 2. Parse the YAML into a generic [`Value`].
/// 3. Count Units and edges, enforcing [`MAX_UNITS`] / [`MAX_EDGES`].
/// 4. Produce the canonical form (recursively sorted keys).
/// 5. Compute the SHA-256 digest of the canonical bytes.
///
/// # Errors
///
/// Returns [`ArtifactError::TooLarge`] if the raw bytes exceed
/// [`MAX_ARTIFACT_BYTES`], [`ArtifactError::TooManyUnits`] /
/// [`ArtifactError::TooManyEdges`] if the declared counts exceed their
/// limits, or [`ArtifactError::ParseError`] if the input is not valid YAML.
pub fn canonicalize(raw: &[u8]) -> Result<CanonicalArtifact, ArtifactError> {
    // 1. Size bound — fail fast before touching the bytes as YAML.
    if raw.len() > MAX_ARTIFACT_BYTES {
        return Err(ArtifactError::TooLarge {
            size: raw.len(),
            limit: MAX_ARTIFACT_BYTES,
        });
    }

    // 1b. Reject YAML anchors/aliases before parsing (W6). The parser
    //     expands them with no size cap, so a sub-1-MiB alias bomb would
    //     otherwise blow past the raw-size bound during parse. Fail-closed.
    if has_yaml_anchor_or_alias(raw) {
        return Err(ArtifactError::AliasesForbidden);
    }

    // 2. Parse.
    let value: Value = serde_yaml::from_slice(raw).map_err(|e| ArtifactError::ParseError {
        source: e.to_string(),
    })?;

    // 3. Count and bound Units / edges.
    let unit_count = count_units(&value);
    let edge_count = count_edges(&value);

    if unit_count > MAX_UNITS {
        return Err(ArtifactError::TooManyUnits {
            count: unit_count,
            limit: MAX_UNITS,
        });
    }
    if edge_count > MAX_EDGES {
        return Err(ArtifactError::TooManyEdges {
            count: edge_count,
            limit: MAX_EDGES,
        });
    }

    // 4. Canonical form: recursively sort mapping keys, then serialize.
    let normalized = normalize(&value);
    let canonical_bytes = serde_yaml::to_string(&normalized)
        .map(String::into_bytes)
        .map_err(|e| ArtifactError::ParseError {
            source: e.to_string(),
        })?;

    // 5. Digest of the canonical bytes.
    let mut hasher = Sha256::new();
    hasher.update(&canonical_bytes);
    let digest = format!("{:x}", hasher.finalize());

    Ok(CanonicalArtifact {
        digest,
        unit_count,
        edge_count,
        canonical_bytes,
    })
}

/// Detect YAML anchor (`&name`) or alias (`*name`) tokens in raw bytes,
/// ignoring occurrences inside quoted scalars or comments.
///
/// W6: `serde_yaml` 0.9 (over `unsafe-libyaml`) expands anchors/aliases
/// during parsing with no size cap, so a sub-1-MiB "billion laughs" alias
/// bomb can expand to gigabytes and exhaust memory *after* the raw-size
/// gate. Alias expansion is impossible without an anchor *definition*
/// (`&name` in node position), so flagging any document that contains a
/// real anchor/alias sigil is a fail-closed, deterministic mitigation with
/// no false negatives for expansion bombs. Canonical execution-plan
/// artifacts are machine-generated and never use anchors/aliases.
///
/// The scan is position-aware to avoid false positives on `&`/`*` that are
/// not YAML node anchors/aliases: it tracks single/double-quoted scalars and
/// `#` comments, and only flags a sigil that is (a) outside quotes/comments,
/// (b) at a token boundary (start of line, or preceded by whitespace or one
/// of `[ ] { } ,`), and (c) immediately followed by an anchor-name start
/// char (`A-Za-z0-9_-`). Scalars like `cmd: "ls *.rs && echo a & b"`,
/// `glob: '*.yml'`, and unquoted `pattern: *.rs` therefore pass through.
fn has_yaml_anchor_or_alias(raw: &[u8]) -> bool {
    let is_name_start = |c: u8| c.is_ascii_alphanumeric() || c == b'_' || c == b'-';
    let is_boundary = |c: u8| matches!(c, b' ' | b'\t' | b'[' | b']' | b'{' | b'}' | b',');

    let mut in_single = false;
    let mut in_double = false;
    let mut in_comment = false;
    let mut at_token_start = true;

    let mut i = 0;
    while i < raw.len() {
        let c = raw[i];
        if in_comment {
            if c == b'\n' {
                in_comment = false;
                at_token_start = true;
            }
            i += 1;
            continue;
        }
        if in_single {
            if c == b'\'' {
                in_single = false;
            }
            at_token_start = false;
            i += 1;
            continue;
        }
        if in_double {
            if c == b'\\' {
                // Skip the escaped byte so a quoted \" does not end the string.
                i += 2;
                at_token_start = false;
                continue;
            }
            if c == b'"' {
                in_double = false;
            }
            at_token_start = false;
            i += 1;
            continue;
        }
        match c {
            b'\'' => {
                in_single = true;
                at_token_start = false;
            }
            b'"' => {
                in_double = true;
                at_token_start = false;
            }
            b'#' if at_token_start => in_comment = true,
            b'&' | b'*' if at_token_start => {
                if i + 1 < raw.len() && is_name_start(raw[i + 1]) {
                    return true;
                }
                at_token_start = false;
            }
            b'\n' => at_token_start = true,
            c if is_boundary(c) => at_token_start = true,
            _ => at_token_start = false,
        }
        i += 1;
    }
    false
}

/// Count the artifact's Units: entries in the top-level `units` (preferred)
/// or `tasks` sequence. Absent/non-sequence fields count as zero.
fn count_units(value: &Value) -> usize {
    sequence_len(value, "units")
        .or_else(|| sequence_len(value, "tasks"))
        .unwrap_or(0)
}

/// Count dependency edges from the top-level `edges`/`dependencies` sequence
/// when present, otherwise from each `units[].depends_on` list used by the
/// Parallel Forge execution-plan format.
fn count_edges(value: &Value) -> usize {
    sequence_len(value, "edges")
        .or_else(|| sequence_len(value, "dependencies"))
        .unwrap_or_else(|| {
            value
                .get("units")
                .and_then(Value::as_sequence)
                .map(|units| {
                    units
                        .iter()
                        .map(|unit| {
                            unit.get("depends_on")
                                .and_then(Value::as_sequence)
                                .map(Vec::len)
                                .unwrap_or(0)
                        })
                        .sum()
                })
                .unwrap_or(0)
        })
}

/// Return the length of the top-level sequence field `key`, if `value` is a
/// mapping and `key` maps to a sequence.
fn sequence_len(value: &Value, key: &str) -> Option<usize> {
    value.get(key).and_then(Value::as_sequence).map(Vec::len)
}

/// Recursively normalize a [`Value`] so that serialization is deterministic:
/// mapping keys are sorted by a stable string key, sequences keep order,
/// scalars are unchanged.
fn normalize(value: &Value) -> Value {
    match value {
        Value::Mapping(map) => {
            let mut entries: Vec<(Value, Value)> = map
                .iter()
                .map(|(k, v)| (normalize(k), normalize(v)))
                .collect();
            entries.sort_by_key(|a| key_repr(&a.0));
            let mut out = Mapping::new();
            for (k, v) in entries {
                out.insert(k, v);
            }
            Value::Mapping(out)
        }
        Value::Sequence(seq) => Value::Sequence(seq.iter().map(normalize).collect()),
        other => other.clone(),
    }
}

/// Stable sort key for a mapping key: the raw string for string keys, a
/// debug rendering otherwise (keys are scalars in practice).
fn key_repr(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal artifact with `n` Units and no edges.
    fn units_yaml(n: usize) -> Vec<u8> {
        let mut s = String::from("units:\n");
        for i in 0..n {
            s.push_str(&format!("  - id: u{i}\n"));
        }
        s.into_bytes()
    }

    /// Build a minimal artifact with `n` dependency edges and no Units.
    fn edges_yaml(n: usize) -> Vec<u8> {
        let mut s = String::from("edges:\n");
        for i in 0..n {
            s.push_str(&format!("  - e{i}\n"));
        }
        s.into_bytes()
    }

    /// Build a valid YAML artifact whose raw byte length is exactly `size`.
    ///
    /// The document is a mapping with an empty `units`/`edges` pair plus a
    /// `pad` string field sized to fill the remainder, so it always parses
    /// and always reports 0 Units / 0 edges.
    fn exact_size_yaml(size: usize) -> Vec<u8> {
        let header = "units: []\nedges: []\npad: \"";
        let footer = "\"\n";
        let pad_len = size
            .checked_sub(header.len() + footer.len())
            .expect("requested size too small to hold the YAML scaffolding");
        let mut s = String::with_capacity(size);
        s.push_str(header);
        s.push_str(&"a".repeat(pad_len));
        s.push_str(footer);
        assert_eq!(
            s.len(),
            size,
            "scaffolding math produced the wrong byte count"
        );
        s.into_bytes()
    }

    #[test]
    fn u9_canonicalizer_produces_deterministic_digest() {
        let raw = b"units:\n  - id: u0\nedges: []\n";

        let first = canonicalize(raw).expect("minimal artifact must canonicalize");
        assert_eq!(first.unit_count, 1);
        assert_eq!(first.edge_count, 0);
        assert!(!first.digest.is_empty());
        assert!(!first.canonical_bytes.is_empty());

        let second = canonicalize(raw).expect("re-canonicalizing must succeed");
        assert_eq!(
            first.digest, second.digest,
            "same bytes must yield the same digest"
        );
        assert_eq!(first.canonical_bytes, second.canonical_bytes);
    }

    #[test]
    fn u9_canonicalizer_is_key_order_independent() {
        // Same logical content, different top-level key order.
        let a = b"units:\n  - id: u0\nedges: []\n";
        let b = b"edges: []\nunits:\n  - id: u0\n";

        let ca = canonicalize(a).unwrap();
        let cb = canonicalize(b).unwrap();
        assert_eq!(ca.digest, cb.digest, "canonical form must ignore key order");
    }

    #[test]
    fn u9_rejects_artifact_over_1mib() {
        let raw = exact_size_yaml(MAX_ARTIFACT_BYTES + 1);
        let err = canonicalize(&raw).expect_err("1 MiB + 1 byte must be rejected");
        assert!(
            matches!(
                err,
                ArtifactError::TooLarge { size, limit }
                    if size == MAX_ARTIFACT_BYTES + 1 && limit == MAX_ARTIFACT_BYTES
            ),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn u9_rejects_artifact_with_513_units() {
        let raw = units_yaml(MAX_UNITS + 1);
        let err = canonicalize(&raw).expect_err("513 units must be rejected");
        assert!(
            matches!(
                err,
                ArtifactError::TooManyUnits { count, limit }
                    if count == MAX_UNITS + 1 && limit == MAX_UNITS
            ),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn u9_rejects_artifact_with_4097_edges() {
        let raw = edges_yaml(MAX_EDGES + 1);
        let err = canonicalize(&raw).expect_err("4097 edges must be rejected");
        assert!(
            matches!(
                err,
                ArtifactError::TooManyEdges { count, limit }
                    if count == MAX_EDGES + 1 && limit == MAX_EDGES
            ),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn u9_accepts_boundary_values() {
        // Exactly 1 MiB (inclusive boundary).
        let raw = exact_size_yaml(MAX_ARTIFACT_BYTES);
        let ok = canonicalize(&raw).expect("exactly 1 MiB must be accepted");
        assert_eq!(ok.unit_count, 0);
        assert_eq!(ok.edge_count, 0);

        // Exactly 512 Units (inclusive boundary).
        let raw = units_yaml(MAX_UNITS);
        let ok = canonicalize(&raw).expect("exactly 512 units must be accepted");
        assert_eq!(ok.unit_count, MAX_UNITS);

        // Exactly 4096 edges (inclusive boundary).
        let raw = edges_yaml(MAX_EDGES);
        let ok = canonicalize(&raw).expect("exactly 4096 edges must be accepted");
        assert_eq!(ok.edge_count, MAX_EDGES);
    }

    // ── W6: YAML anchor/alias billion-laughs rejection ──────────────

    /// The classic exponential alias bomb: a few hundred raw bytes that
    /// expand to ~gigabytes (nine levels of 9x aliasing).
    fn billion_laughs_yaml() -> Vec<u8> {
        let mut s = String::from(
            "a: &a [\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\",\"lol\"]\n",
        );
        for (name, prev) in [
            ("b", "a"),
            ("c", "b"),
            ("d", "c"),
            ("e", "d"),
            ("f", "e"),
            ("g", "f"),
            ("h", "g"),
            ("i", "h"),
        ] {
            s.push_str(&format!(
                "{name}: &{name} [*{prev},*{prev},*{prev},*{prev},*{prev},*{prev},*{prev},*{prev},*{prev}]\n"
            ));
        }
        s.into_bytes()
    }

    #[test]
    fn u6w_rejects_billion_laughs_fast_and_bounded() {
        let raw = billion_laughs_yaml();
        assert!(
            raw.len() < MAX_ARTIFACT_BYTES,
            "bomb raw size must be under the size gate"
        );
        let start = std::time::Instant::now();
        let err = canonicalize(&raw).expect_err("alias bomb must be rejected");
        assert!(
            matches!(err, ArtifactError::AliasesForbidden),
            "unexpected error: {err:?}"
        );
        assert!(
            start.elapsed() < std::time::Duration::from_secs(1),
            "rejection must be fast (no alias expansion)"
        );
    }

    #[test]
    fn u6w_rejects_simple_anchor_and_alias() {
        let anchor = b"units:\n  - id: &x u0\nedges: []\n";
        assert!(matches!(
            canonicalize(anchor).expect_err("anchor must be rejected"),
            ArtifactError::AliasesForbidden
        ));

        let alias = b"base: &b u0\nunits:\n  - id: *b\nedges: []\n";
        assert!(matches!(
            canonicalize(alias).expect_err("alias must be rejected"),
            ArtifactError::AliasesForbidden
        ));
    }

    #[test]
    fn u6w_normal_artifact_still_canonicalizes() {
        let raw = b"units:\n  - id: u0\nedges: []\n";
        let c = canonicalize(raw).expect("clean artifact must canonicalize");
        assert_eq!(c.unit_count, 1);
        assert_eq!(c.edge_count, 0);
        assert!(!c.digest.is_empty());
        // Determinism is unchanged by the pre-parse scanner.
        assert_eq!(c.digest, canonicalize(raw).unwrap().digest);
    }

    #[test]
    fn u6w_false_positive_guard() {
        // `&`/`*` in non-anchor positions must NOT be rejected.
        let cases: &[&[u8]] = &[
            b"cmd: \"ls *.rs && echo a & b\"\nunits: []\n", // quoted glob + && / &
            b"glob: '*.yml'\nunits: []\n",                  // single-quoted glob
            b"note: \"a & b\"\nunits: []\n",                // quoted ampersand
            b"math: 2*3\nunits: []\n",                      // unquoted '*' not at token start
            b"expr: a && b\nunits: []\n",                   // unquoted && (& then '&')
            b"path: /a/b#c\nunits: []\n",                   // '#' mid-scalar, not a comment
        ];
        for raw in cases {
            canonicalize(raw).unwrap_or_else(|e| {
                panic!(
                    "false positive on {:?}: {e:?}",
                    String::from_utf8_lossy(raw)
                )
            });
        }
    }
}
