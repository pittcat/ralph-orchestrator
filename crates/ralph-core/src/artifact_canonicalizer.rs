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
}

impl fmt::Display for ArtifactError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArtifactError::TooLarge { size, limit } => write!(
                f,
                "artifact is {size} bytes, exceeding the {limit}-byte (1 MiB) limit"
            ),
            ArtifactError::TooManyUnits { count, limit } => {
                write!(f, "artifact declares {count} units, exceeding the limit of {limit}")
            }
            ArtifactError::TooManyEdges { count, limit } => write!(
                f,
                "artifact declares {count} dependency edges, exceeding the limit of {limit}"
            ),
            ArtifactError::ParseError { source } => {
                write!(f, "failed to parse artifact YAML: {source}")
            }
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

    // 2. Parse.
    let value: Value = serde_yaml::from_slice(raw)
        .map_err(|e| ArtifactError::ParseError { source: e.to_string() })?;

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
        .map_err(|e| ArtifactError::ParseError { source: e.to_string() })?;

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

/// Count the artifact's Units: entries in the top-level `units` (preferred)
/// or `tasks` sequence. Absent/non-sequence fields count as zero.
fn count_units(value: &Value) -> usize {
    sequence_len(value, "units").or_else(|| sequence_len(value, "tasks")).unwrap_or(0)
}

/// Count the artifact's dependency edges: entries in the top-level `edges`
/// (preferred) or `dependencies` sequence. Absent/non-sequence fields count
/// as zero.
fn count_edges(value: &Value) -> usize {
    sequence_len(value, "edges").or_else(|| sequence_len(value, "dependencies")).unwrap_or(0)
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
            let mut entries: Vec<(Value, Value)> =
                map.iter().map(|(k, v)| (normalize(k), normalize(v))).collect();
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
        assert_eq!(s.len(), size, "scaffolding math produced the wrong byte count");
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
        assert_eq!(first.digest, second.digest, "same bytes must yield the same digest");
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
}
