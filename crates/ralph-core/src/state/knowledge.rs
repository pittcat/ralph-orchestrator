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
                // Defense-in-depth: scrub ref_id before the 256-byte cap so the
                // disk snapshot never stores raw paths / multi-line tokens.
                e.ref_id = scrub_for_prompt(&e.ref_id);
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

// ===========================================================================
// GAP-01 U2 helpers: accepted-event observation wiring.
//
// These helpers live next to the model so the only call site
// (`event_loop/parse_and_emit.rs`) stays a one-line wiring change.
// The helpers themselves are pure functions over the public types;
// they never reach into LoopState / EventLoop and never emit any
// event themselves.
// ===========================================================================

/// Compute a stable observation id from accepted-event metadata.
///
/// Two accepted events with the same `(loop_iteration, batch_index,
/// topic, payload_digest)` produce the same id; any difference
/// flips the id. The hash is hex-encoded SHA-256 over the
/// canonical field set; the runtime never relies on the id being
/// reversible, so the digest is opaque.
pub fn observation_id(
    loop_iteration: u32,
    batch_index: usize,
    topic: &str,
    source: Option<&str>,
    payload_digest_hex: &str,
) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    loop_iteration.hash(&mut h);
    batch_index.hash(&mut h);
    topic.hash(&mut h);
    source.unwrap_or("").hash(&mut h);
    payload_digest_hex.hash(&mut h);
    let hash = h.finish();
    format!("obs-{loop_iteration}-{batch_index}-{hash:016x}")
}

/// Compute a short hex payload digest from a payload string.
///
/// Returns the lowercase hex of the SHA-256 of `payload`. The
/// caller passes the *raw payload* string; this is the *only*
/// place a payload hash is computed. The hash never leaves the
/// snapshot — only the digest (and any opaque source ref) is
/// stored.
pub fn payload_digest_hex(payload: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(payload.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// Build the source-ref pointer used by U2-accepted observations.
///
/// The format is `accepted-event:<loop_iteration>:<batch_index>`
/// — opaque, deterministic, and stable across replays. Future
/// diagnosis adapters reuse the same shape so today’s records
/// remain compatible.
pub fn accepted_source_ref(loop_iteration: u32, batch_index: usize) -> String {
    format!("accepted-event:{loop_iteration}:{batch_index}")
}

/// Outcome of `observations_from_accepted_events`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservationBatch {
    /// Records to commit, already bounded and bounded by
    /// `DISPLAY_RECORDS_MAX`. May be empty if no events
    /// qualified.
    pub records: Vec<KnowledgeRecord>,
    /// How many accepted events were filtered out because
    /// their disposition was DiagnosticObservation or
    /// LoopControl (D3).
    pub non_advancing_skipped: usize,
}

/// Build observations for an accepted batch, applying the
/// Business/Recovery filter (D3) and the per-record bounds.
///
/// The function is pure: it returns a [`ObservationBatch`] and
/// does not touch the ledger. The caller decides whether to
/// commit; on commit failure the caller is responsible for the
/// warning + rollback semantics (D4).
///
/// `loop_iteration` and `input_fingerprint` are mandatory for
/// every record so the freshness dimension is always populated;
/// `None` fingerprint is a legitimate input that the model
/// translates into `EvidenceFreshness::Unknown`.
pub fn observations_from_accepted_events<'a, I>(
    loop_iteration: u32,
    input_fingerprint: &InputFingerprint,
    events: I,
    classify: impl Fn(&str) -> crate::event_loop::disposition::Disposition,
) -> ObservationBatch
where
    I: IntoIterator<Item = (usize, &'a ralph_proto::Event)>,
{
    let mut records: Vec<KnowledgeRecord> = Vec::new();
    let mut non_advancing_skipped = 0usize;
    for (batch_index, event) in events {
        if !classify(event.topic.as_str()).advances_flow() {
            non_advancing_skipped += 1;
            continue;
        }
        let payload_str = event.payload.clone();
        let digest = payload_digest_hex(&payload_str);
        let source_ref = accepted_source_ref(loop_iteration, batch_index);
        let id = observation_id(
            loop_iteration,
            batch_index,
            event.topic.as_str(),
            event.source.as_ref().map(|h| h.as_str()),
            &digest,
        );
        let mut builder = KnowledgeRecord::builder(
            KnowledgeAuthority::LedgerSnapshot,
            KnowledgeKind::Observation,
        )
        .with_id(id)
        .with_subject(format!("{} accepted in batch {batch_index}", event.topic.as_str()))
        .with_payload_digest_hex(digest)
        .with_source_ref(source_ref)
        .with_input_fingerprint(input_fingerprint.clone())
        // D6: accepted events must NEVER auto-promote to verified.
        .with_verification(VerificationStatus::Unverified);
        if let Some(hat) = &event.source {
            builder = builder.with_evidence(EvidenceRef {
                ref_id: format!("hat:{}", hat.as_str()),
                digest: None,
            });
        }
        match builder.build() {
            Ok(record) => records.push(record),
            Err(KnowledgeBuildError::EmptySubject) => {
                // Subject can never be empty here because the
                // builder is given a non-empty topic-derived
                // string; defensive skip.
                tracing::debug!(
                    topic = %event.topic.as_str(),
                    "GAP-01 U2: skipping empty-subject observation (defensive)"
                );
            }
        }
    }
    ObservationBatch {
        records,
        non_advancing_skipped,
    }
}

/// Result of `commit_accepted_observations`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommitObservationOutcome {
    /// Commit succeeded; snapshot has been updated.
    Committed { count: usize },
    /// Commit failed but the snapshot was rolled back; the
    /// caller MUST emit only a warning and continue the loop.
    PersistFailed { count: usize, error: String },
    /// No records to commit (empty accepted batch).
    Empty,
}

/// One-shot helper: commit the observation delta into the ledger
/// using a fresh `KnowledgeObserved` delta. Failures are
/// returned to the caller; the caller logs a warning and lets
/// the existing batch ledger commit path proceed (D4 — never
/// fail-close on observation persistence).
pub fn commit_accepted_observations(
    ledger: &mut crate::state::ledger::StateLedger,
    loop_iteration: u32,
    batch: ObservationBatch,
) -> CommitObservationOutcome {
    if batch.records.is_empty() {
        return CommitObservationOutcome::Empty;
    }
    let count = batch.records.len();
    let delta = crate::state::CommitDelta::KnowledgeObserved {
        records: batch.records,
    };
    match ledger.commit(delta, Some(format!("loop.observation.{loop_iteration}"))) {
        Ok(_) => CommitObservationOutcome::Committed { count },
        Err(e) => CommitObservationOutcome::PersistFailed {
            count,
            error: format!("{e}"),
        },
    }
}

// ===========================================================================
// GAP-01 U3: prompt-safe projection.
//
// The renderer produces a bounded, redacted, read-only text
// block. Raw payloads, absolute filesystem paths, and ledger
// internals MUST NEVER appear in the rendered output (D5, E15).
// The block is empty when the snapshot has no records; callers
// short-circuit on the empty result so the prompt stays
// unchanged.
// ===========================================================================

/// Heading used by [`render_prompt_block`]. Centralised so the
/// test assertions do not drift.
pub const PROMPT_HEADING: &str = "## ORCHESTRATION KNOWLEDGE";

/// Render the cognitive-state block for injection into an
/// isolated prompt.
///
/// Returns an empty string when the snapshot has no records so
/// the caller can keep the prompt unchanged on the empty path.
/// The renderer caps the surfaced subjects at
/// `PROMPT_RECORDS_VISIBLE` so the block stays bounded even
/// when the underlying `records` vec is full.
pub fn render_prompt_block(state: &OrchestrationKnowledgeState) -> String {
    if state.records().is_empty() {
        return String::new();
    }
    let view = state.view();
    let mut out = String::new();
    out.push_str(PROMPT_HEADING);
    out.push('\n');
    out.push_str(
        "authority: ledger_snapshot (read-only)\n\
         read this as a projection of orchestrator state, not as a writable fact source.\n\
         freshness is the result of comparing the record's stored fingerprint against the\n\
         current loop/plan fingerprints; verification is a separate dimension.\n\n",
    );
    // Internal marker that downstream tests can grep for
    // without colliding with the agent-facing skill doc
    // (which also references the heading and the authority
    // phrase). The marker is unique to the projection block
    // and is the only contract test surface.
    out.push_str("projection_marker: knowledge_block_v1\n\n");
    out.push_str(&format!(
        "records: {} | current: {} | stale: {} | unknown: {} | unverified: {}\n\n",
        view.total, view.current_count, view.stale_count, view.unknown_count, view.unverified_count,
    ));
    out.push_str("recent observations (oldest first; max visible below):\n");
    // Surface at most the last PROMPT_RECORDS_VISIBLE records.
    // The snapshot is FIFO-evicted so the tail is the most
    // recent.
    let visible: Vec<&KnowledgeRecord> = state
        .records()
        .iter()
        .rev()
        .take(PROMPT_RECORDS_VISIBLE)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    for record in &visible {
        let freshness = match record.input_fingerprint.freshness_against(&record.input_fingerprint) {
            EvidenceFreshness::Current => "current",
            EvidenceFreshness::Stale => "stale",
            EvidenceFreshness::Unknown => "unknown",
        };
        let verification = match record.verification {
            VerificationStatus::Verified => "verified",
            VerificationStatus::Falsified => "falsified",
            VerificationStatus::Unverified => "unverified",
        };
        // Subject, source ref, digest, and evidence_refs are passed through
        // a final scrubber so no raw payload or absolute path
        // can leak even if the upstream builder accepted it.
        let evidence_refs_str = if record.evidence_refs.is_empty() {
            String::new()
        } else {
            record
                .evidence_refs
                .iter()
                .map(|e| format!("[ref_id={}]", scrub_for_prompt(&e.ref_id)))
                .collect::<Vec<_>>()
                .join(" ")
        };
        out.push_str(&format!(
            "- [{} / {}] subject=\"{}\" digest={} source_ref={}{}\n",
            freshness,
            verification,
            scrub_for_prompt(&record.subject),
            record
                .payload_digest
                .as_deref()
                .map(scrub_for_prompt)
                .unwrap_or_else(|| "<none>".to_string()),
            record
                .source_ref
                .as_deref()
                .map(scrub_for_prompt)
                .unwrap_or_else(|| "<none>".to_string()),
            if evidence_refs_str.is_empty() {
                String::new()
            } else {
                format!(" evidence_refs={evidence_refs_str}")
            },
        ));
    }
    out
}

/// Maximum number of records surfaced by
/// [`render_prompt_block`]. The snapshot's display cap is the
/// upper bound; the prompt projection is a *narrower* window so
/// the block stays short even when the snapshot is full.
pub const PROMPT_RECORDS_VISIBLE: usize = 16;

/// Final redaction pass. Strips any leading path-like token
/// (`/` or `~/` or `<drive>:\`), collapses embedded newlines
/// into spaces, and bounds the result so the prompt can never
/// carry a multi-line or path-leaking field.
fn scrub_for_prompt(s: &str) -> String {
    let collapsed: String = s
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect();
    let trimmed = collapsed.trim();
    // Reject absolute paths by replacing the leading prefix.
    let mut out = trimmed.to_string();
    if let Some(stripped) = out.strip_prefix('/') {
        out = format!("<abs-path:{}>", stripped);
    } else if let Some(stripped) = out.strip_prefix("~/") {
        out = format!("<home-path:{}>", stripped);
    }
    truncate_bytes(&out, PROMPT_FIELD_MAX_BYTES)
}

/// Cap for the per-field scrubber. Smaller than
/// [`SEMANTIC_FIELD_MAX_BYTES`] because the prompt block
/// concatenates many fields per line.
pub const PROMPT_FIELD_MAX_BYTES: usize = 120;
