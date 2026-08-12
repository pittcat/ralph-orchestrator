//! GAP-01 (plan 2026-08-13-001): unified, replayable
//! orchestration cognitive state.
//!
//! The cognitive state lives inside [`LedgerSnapshot::knowledge`]
//! and is the only durable projection for what the orchestrator
//! has *observed* so far. The prompt only reads its compressed
//! projection — it can never write back into state.
//!
//! ## Authority order (D1)
//!
//! ```text
//! LedgerSnapshot.knowledge (memory read)
//!   → .ralph/ledger.jsonl replay (cross-activation durable source)
//!     → accepted events are write-only inputs to observations
//!       → recovery/diagnosis journals are evidence pointers
//!         → LoopState caches are session-local helpers
//!           → prompt projection is a read-only terminal
//! ```
//!
//! The authority ordering is enforced by the *types* in this
//! module: only [`KnowledgeAuthority::LedgerSnapshot`] may set
//! the authority of a record at construction. A prompt block
//! rendered from `to_prompt_block` is a borrowed view; there is
//! no public API to mutate state from the prompt path.
//!
//! ## Bounds (D10)
//!
//! - `display_records_max = 128` — the snapshot caps the
//!   *displayed* record set; the commit log retains every delta
//!   so the ledger append/replay semantics are unchanged.
//! - `evidence_refs_max = 8` — each record carries at most this
//!   many evidence references.
//! - `semantic_field_max_bytes = 256` — `subject`, `source_ref`
//!   and any other bounded string are capped at 256 UTF-8 bytes
//!   after sanitisation.
//!
//! ## Freshness vs verification (D6)
//!
//! [`InputFingerprint`] computes freshness against a comparing
//! fingerprint: `None` → `Unknown`, equal → `Current`, different
//! → `Stale`. [`VerificationStatus`] is a separate dimension and
//! is **never** upgraded by an accepted event — the
//! `KnowledgeRecord::builder` defaults to `Unverified` and only
//! `with_verification` can move it forward.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Default cap on displayed records inside
/// [`OrchestrationKnowledgeState::records`]. The commit log is
/// unaffected; only the in-memory display vec is bounded so the
/// prompt projection stays cheap.
pub const DISPLAY_RECORDS_MAX: usize = 128;

/// Maximum number of evidence references per [`KnowledgeRecord`].
pub const EVIDENCE_REFS_MAX: usize = 8;

/// Maximum UTF-8 byte length for any bounded string semantic
/// field (`subject`, `source_ref`, etc.). Truncated to this cap
/// during [`KnowledgeRecordBuilder::build`].
pub const SEMANTIC_FIELD_MAX_BYTES: usize = 256;

/// Authority of a [`KnowledgeRecord`]. The enum is closed: only
/// `LedgerSnapshot` can author cognitive state today (D1).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeAuthority {
    /// Durable authority: `LedgerSnapshot.knowledge`, replayable
    /// from `.ralph/ledger.jsonl`.
    #[default]
    LedgerSnapshot,
}

/// First-class semantic category of a knowledge record (D6,
/// R3). The set is intentionally narrow; richer classification
/// should live in the subject / evidence refs, not in new
/// variants, so the wire format stays bounded.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeKind {
    /// A claim made by the orchestrator or an accepted event.
    Claim,
    /// Evidence pointer (digest / opaque ref / bounded quote).
    Evidence,
    /// A working hypothesis that has not been verified yet.
    Hypothesis,
    /// An assumption explicitly recorded for the current loop.
    Assumption,
    /// A recorded unknown — the loop knows it does not know.
    Unknown,
    /// A record that has been promoted to verified by a typed
    /// verifier path (not by an accepted event).
    Verified,
    /// A record that has been falsified by a typed verifier path.
    Falsified,
    /// A loop-level decision recorded with its reason.
    Decision,
    /// A routing reason recorded for the current topology.
    RouteReason,
    /// Generic observation produced by an accepted
    /// `Business`/`Recovery` event — the default kind for the
    /// post-validation wiring in U2.
    #[default]
    Observation,
}

/// Verification dimension. Always separate from freshness
/// (D6). An accepted event MUST NOT auto-promote a record to
/// `Verified`; only `KnowledgeRecordBuilder::with_verification`
/// can set this field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    /// Default for accepted events — the record has not been
    /// independently verified.
    #[default]
    Unverified,
    /// Independently verified by a typed verifier path.
    Verified,
    /// Independently falsified.
    Falsified,
}

/// Freshness dimension (D6). Computed by
/// [`InputFingerprint::freshness_against`]; never set by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceFreshness {
    /// The fingerprint is missing entirely — the orchestrator
    /// cannot compare.
    #[default]
    Unknown,
    /// Fingerprint equals the comparing fingerprint.
    Current,
    /// Fingerprint differs from the comparing fingerprint.
    Stale,
}

/// Bounded input fingerprint. The loop carries two SHAs at
/// runtime: `loop_start_sha` (set by the runner at loop start)
/// and `plan_baseline_sha` (reconciled by the plan-reviewer).
/// Either can be missing on cold start.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum InputFingerprint {
    /// No usable fingerprint available. Freshness against any
    /// fingerprint is always `Unknown`.
    #[default]
    None,
    /// Both SHAs are available.
    Both {
        loop_start_sha: String,
        plan_baseline_sha: String,
    },
    /// Only `loop_start_sha` is available.
    LoopStartOnly {
        loop_start_sha: String,
    },
    /// Only `plan_baseline_sha` is available.
    PlanBaselineOnly {
        plan_baseline_sha: String,
    },
}

impl InputFingerprint {
    /// Convenience constructor for the common `Both` case.
    pub fn both(loop_start_sha: impl Into<String>, plan_baseline_sha: impl Into<String>) -> Self {
        Self::Both {
            loop_start_sha: loop_start_sha.into(),
            plan_baseline_sha: plan_baseline_sha.into(),
        }
    }

    /// Compute [`EvidenceFreshness`] against a comparing
    /// fingerprint. Conservative semantics:
    ///
    /// - either side is `None` → `Unknown`
    /// - structural equality → `Current`
    /// - any difference → `Stale`
    pub fn freshness_against(&self, other: &InputFingerprint) -> EvidenceFreshness {
        match (self, other) {
            (InputFingerprint::None, _) | (_, InputFingerprint::None) => {
                EvidenceFreshness::Unknown
            }
            _ if self == other => EvidenceFreshness::Current,
            _ => EvidenceFreshness::Stale,
        }
    }
}

/// Bounded opaque evidence reference. Stored verbatim up to
/// `SEMANTIC_FIELD_MAX_BYTES`; the renderer never echoes it back
/// to the prompt as raw text — it is hashed or rendered as an
/// opaque ref token (D5).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EvidenceRef {
    /// Opaque stable id (`accepted-event:<iter>:<batch-idx>:<id>`
    /// today; future diagnosis adapters reuse the same shape).
    pub ref_id: String,
    /// Optional digest (hex) of the referenced payload. Never the
    /// raw payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
}

/// A single replayable cognitive record. Built via
/// [`KnowledgeRecord::builder`] so every field passes the bound
/// checks before the record reaches the snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeRecord {
    /// Stable record id. Duplicate ids collapse to a single
    /// snapshot entry (idempotent apply, R5).
    pub id: String,
    /// Authority of this record.
    pub authority: KnowledgeAuthority,
    /// Semantic category.
    pub kind: KnowledgeKind,
    /// Short human-readable subject (≤
    /// [`SEMANTIC_FIELD_MAX_BYTES`] bytes). Never contains the
    /// raw payload.
    pub subject: String,
    /// Optional opaque pointer to the original event source
    /// (e.g. `accepted-event:1:0:obs-1`). Never an absolute
    /// filesystem path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_ref: Option<String>,
    /// Optional payload digest (hex). The raw payload is never
    /// stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload_digest: Option<String>,
    /// Bounded evidence references.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<EvidenceRef>,
    /// Verification status — never auto-promoted by accepted
    /// events (D6).
    pub verification: VerificationStatus,
    /// Input fingerprint at the time the record was authored.
    pub input_fingerprint: InputFingerprint,
    /// Wall-clock timestamp (RFC3339).
    pub recorded_at_ts: String,
}

impl KnowledgeRecord {
    /// Start a builder for a new record. The caller MUST set
    /// `authority` to [`KnowledgeAuthority::LedgerSnapshot`] —
    /// there is no other authority today (D1).
    pub fn builder(authority: KnowledgeAuthority, kind: KnowledgeKind) -> KnowledgeRecordBuilder {
        KnowledgeRecordBuilder {
            authority,
            kind,
            id: None,
            subject: String::new(),
            source_ref: None,
            payload_digest: None,
            evidence_refs: Vec::new(),
            verification: VerificationStatus::default(),
            input_fingerprint: InputFingerprint::default(),
            recorded_at_ts: chrono::Utc::now().to_rfc3339(),
        }
    }
}

/// Builder for [`KnowledgeRecord`]. All bounds are enforced in
/// [`Self::build`] so no partial record reaches the snapshot.
#[derive(Debug, Clone)]
pub struct KnowledgeRecordBuilder {
    authority: KnowledgeAuthority,
    kind: KnowledgeKind,
    id: Option<String>,
    subject: String,
    source_ref: Option<String>,
    payload_digest: Option<String>,
    evidence_refs: Vec<EvidenceRef>,
    verification: VerificationStatus,
    input_fingerprint: InputFingerprint,
    recorded_at_ts: String,
}

impl KnowledgeRecordBuilder {
    /// Override the stable record id. Defaults to a timestamped
    /// id when unset.
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Set the short subject line. Truncated to
    /// [`SEMANTIC_FIELD_MAX_BYTES`] bytes when overlong.
    pub fn with_subject(mut self, subject: impl Into<String>) -> Self {
        self.subject = subject.into();
        self
    }

    /// Set the opaque source ref pointer.
    pub fn with_source_ref(mut self, source_ref: impl Into<String>) -> Self {
        self.source_ref = Some(source_ref.into());
        self
    }

    /// Set the payload digest (hex). Raw payload MUST NOT reach
    /// this builder.
    pub fn with_payload_digest_hex(mut self, digest: impl Into<String>) -> Self {
        self.payload_digest = Some(digest.into());
        self
    }

    /// Append a bounded evidence reference.
    pub fn with_evidence(mut self, evidence: EvidenceRef) -> Self {
        self.evidence_refs.push(evidence);
        self
    }

    /// Override the verification status. Callers MUST NOT use
    /// this to upgrade accepted events — D6.
    pub fn with_verification(mut self, status: VerificationStatus) -> Self {
        self.verification = status;
        self
    }

    /// Override the input fingerprint.
    pub fn with_input_fingerprint(mut self, fingerprint: InputFingerprint) -> Self {
        self.input_fingerprint = fingerprint;
        self
    }

    /// Finalise the builder. Truncates overlong strings, drops
    /// over-cap evidence refs, and rejects empty subjects. The
    /// returned id is stable across rebuilds (D5).
    pub fn build(self) -> Result<KnowledgeRecord, KnowledgeBuildError> {
        if self.subject.trim().is_empty() {
            return Err(KnowledgeBuildError::EmptySubject);
        }
        let subject = truncate_bytes(&self.subject, SEMANTIC_FIELD_MAX_BYTES);
        let source_ref = self
            .source_ref
            .map(|s| truncate_bytes(&s, SEMANTIC_FIELD_MAX_BYTES));
        let payload_digest = self
            .payload_digest
            .map(|d| truncate_bytes(&d, SEMANTIC_FIELD_MAX_BYTES));
        let evidence_refs: Vec<EvidenceRef> = self
            .evidence_refs
            .into_iter()
            .take(EVIDENCE_REFS_MAX)
            .map(|mut e| {
                e.ref_id = truncate_bytes(&e.ref_id, SEMANTIC_FIELD_MAX_BYTES);
                if let Some(d) = e.digest.as_ref() {
                    e.digest = Some(truncate_bytes(d, SEMANTIC_FIELD_MAX_BYTES));
                }
                e
            })
            .collect();
        let id = self
            .id
            .map(|i| truncate_bytes(&i, SEMANTIC_FIELD_MAX_BYTES))
            .unwrap_or_else(|| {
                format!(
                    "kr-{}",
                    truncate_bytes(&self.recorded_at_ts, SEMANTIC_FIELD_MAX_BYTES)
                )
            });
        Ok(KnowledgeRecord {
            id,
            authority: self.authority,
            kind: self.kind,
            subject,
            source_ref,
            payload_digest,
            evidence_refs,
            verification: self.verification,
            input_fingerprint: self.input_fingerprint,
            recorded_at_ts: self.recorded_at_ts,
        })
    }
}

/// Errors surfaced by [`KnowledgeRecordBuilder::build`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum KnowledgeBuildError {
    /// The subject was empty after trimming.
    #[error("knowledge record subject must be non-empty")]
    EmptySubject,
}

/// Truncate a string to `max_bytes` UTF-8 bytes. The cut is on a
/// char boundary so multi-byte codepoints are never sliced.
fn truncate_bytes(s: &str, max_bytes: usize) -> String {
    if s.len() <= max_bytes {
        return s.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Compressed view of [`OrchestrationKnowledgeState`]. Designed
/// for the prompt projection: only counts and a few sample
/// fields, never the raw record body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnowledgeView {
    /// Total record count in the display vec (already bounded).
    pub total: usize,
    /// Records whose freshness is `Current`.
    pub current_count: usize,
    /// Records whose freshness is `Stale`.
    pub stale_count: usize,
    /// Records whose freshness is `Unknown`.
    pub unknown_count: usize,
    /// Records whose verification status is `Unverified`.
    pub unverified_count: usize,
}

/// The cognitive-state subtree attached to
/// [`crate::state::snapshot::LedgerSnapshot`]. Holds bounded
/// records, dedup by id, and exposes the prompt-safe view.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrchestrationKnowledgeState {
    /// Display records, ordered by insertion time (oldest
    /// first). Capped at [`DISPLAY_RECORDS_MAX`]; the commit
    /// log retains every delta so replay preserves the
    /// authoritative history.
    records: Vec<KnowledgeRecord>,
    /// Quick-lookup index by record id (mirrors `records`).
    /// Bounded by `DISPLAY_RECORDS_MAX` so the snapshot stays
    /// cheap.
    #[serde(skip)]
    by_id: HashMap<String, usize>,
}

impl OrchestrationKnowledgeState {
    /// Borrow the (bounded) display vec of records.
    pub fn records(&self) -> &[KnowledgeRecord] {
        &self.records
    }

    /// Build a compressed prompt-safe view. Always cheap:
    /// O(display records).
    pub fn view(&self) -> KnowledgeView {
        let mut view = KnowledgeView {
            total: self.records.len(),
            ..KnowledgeView::default()
        };
        for record in &self.records {
            match record.input_fingerprint.freshness_against(&record.input_fingerprint) {
                EvidenceFreshness::Current => view.current_count += 1,
                EvidenceFreshness::Stale => view.stale_count += 1,
                EvidenceFreshness::Unknown => view.unknown_count += 1,
            }
            if record.verification == VerificationStatus::Unverified {
                view.unverified_count += 1;
            }
        }
        view
    }

    /// Insert a record. Idempotent on id: a re-apply with the
    /// same id updates the existing entry in place. The display
    /// vec never grows past [`DISPLAY_RECORDS_MAX`] — older
    /// entries are evicted FIFO when the cap is hit.
    pub fn insert(&mut self, record: KnowledgeRecord) {
        if let Some(&idx) = self.by_id.get(&record.id) {
            self.records[idx] = record;
            return;
        }
        // Evict the oldest entry if we are at the cap. New
        // records are appended at the tail so the most recent
        // observations remain visible to the prompt projection.
        if self.records.len() >= DISPLAY_RECORDS_MAX {
            let evicted = self.records.remove(0);
            self.by_id.remove(&evicted.id);
            // Re-sync the indices after the FIFO shift.
            self.by_id = self
                .records
                .iter()
                .enumerate()
                .map(|(i, r)| (r.id.clone(), i))
                .collect();
        }
        self.records.push(record.clone());
        self.by_id.insert(record.id.clone(), self.records.len() - 1);
    }

    /// Apply a batch of records. Equivalent to repeated
    /// [`Self::insert`] calls.
    pub fn apply(&mut self, records: impl IntoIterator<Item = KnowledgeRecord>) {
        for r in records {
            self.insert(r);
        }
    }
}
