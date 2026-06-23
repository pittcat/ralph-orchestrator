//! Persistent rejection log — `.ralph/recovery.jsonl` records
//! for the deterministic-correction path (U7a plan
//! 2026-06-21-002).
//!
//! The legacy `RecoveryDiagnosisEnvelope` flow already writes a
//! `recovery.jsonl` file under
//! `.ralph/diagnostics/<session>/recovery.jsonl`.  That file is
//! session-scoped and lives inside the diagnostics collector's
//! directory tree; `ralph diagnose` reads it for the per-session
//! summary.
//!
//! U7a introduces a *second* `recovery.jsonl` at the workspace
//! root (`.ralph/recovery.jsonl`) so:
//!
//!   1. The deterministic-correction path can persist the
//!      per-rejection record alongside the prompt block — even
//!      when the diagnostics collector is disabled
//!      (`RALPH_DIAGNOSTICS` unset).
//!   2. `ralph diagnose` (U8) can prefer the ledger-aligned
//!      log over the legacy session-scoped log for offline
//!      analysis.
//!   3. Bounded-retry bookkeeping survives session restarts:
//!      operators tail the file and see the per-key retry
//!      history.
//!
//! ## File format
//!
//! JSON Lines (one `RejectionRecord` per line).  Fields:
//!
//! | Field | Type | Notes |
//! |-------|------|-------|
//! | `ts` | RFC3339 string | When the rejection was recorded. |
//! | `hat` | string | Source hat (`"unknown"` when missing). |
//! | `topic` | string | Rejected topic. |
//! | `reason_code` | string | Stable code, e.g. `origin:missing_field`. |
//! | `retry_count` | u32 | Per-key counter (R2 + R3). |
//! | `terminal_reason` | Option<string> | Set when the rejection tripped escalation. |
//!
//! File I/O is best-effort: a write failure is logged but does
//! not abort the loop (matches the policy of the legacy
//! `RecoveryDiagnosisEnvelope` logger).

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Relative path of the rejection log file inside the workspace.
pub const RECOVERY_LOG_RELATIVE_PATH: &str = ".ralph/recovery.jsonl";

/// One line in the rejection log.  Serialised as JSON; mirrors
/// the field shape used by [`crate::diagnosis::RecoveryJournalEntry`]
/// for forward-compatibility with `ralph diagnose`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RejectionRecord {
    /// RFC3339 timestamp the record was written.
    pub ts: String,
    /// Source hat, or `"unknown"` when missing.
    pub hat: String,
    /// Rejected topic.
    pub topic: String,
    /// Stable reason code (`origin:missing_field`, etc.).
    pub reason_code: String,
    /// Retry count for this key at the time of the record.
    pub retry_count: u32,
    /// Optional terminal reason (R11).  `None` for records that
    /// did not trip escalation.
    pub terminal_reason: Option<String>,
    /// U6 (plan 2026-06-23-004): typed kind 字段。
    ///
    /// 与 `reason_code` 冗余存储但语义不同:
    /// - `reason_code` 是历史 grep 兼容字符串(`hat_handoff_filename_mismatch` 等)
    /// - `kind` 是 typed 字段(`RejectionKind::reason_code()` SSOT 化),
    ///   消费方可按 kind 做 typed 分桶聚合,无需字符串匹配。
    ///
    /// 老 envelope(无 `kind` 字段)反序列化时为 `None`,保持向前兼容。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
}

impl RejectionRecord {
    /// Convenience builder.  `ts` defaults to `now_rfc3339()`.
    pub fn new(
        hat: impl Into<String>,
        topic: impl Into<String>,
        reason_code: impl Into<String>,
        retry_count: u32,
    ) -> Self {
        Self {
            ts: now_rfc3339(),
            hat: hat.into(),
            topic: topic.into(),
            reason_code: reason_code.into(),
            retry_count,
            terminal_reason: None,
            kind: None,
        }
    }

    /// U6 (plan 2026-06-23-004): 从 typed rejection 构造工厂方法,
    /// 确保 `kind` 与 `reason_code` 都从同一个 `RejectionKind` SSOT 派生。
    pub fn from_typed_rejection(
        hat: impl Into<String>,
        topic: impl Into<String>,
        kind: crate::preset::engine::gates::RejectionKind,
        retry_count: u32,
    ) -> Self {
        let code = kind.reason_code().to_string();
        Self {
            ts: now_rfc3339(),
            hat: hat.into(),
            topic: topic.into(),
            reason_code: code.clone(),
            retry_count,
            terminal_reason: None,
            kind: Some(code),
        }
    }

    /// 2026-06-23 fix plan U6 (CB-3): legacy-tolerant factory
    /// used by correction paths that already have a
    /// `reason_code` string (the legacy string from
    /// `extract_reason_code(&violation)`) but no typed
    /// `RejectionKind` available. Calls
    /// [`RejectionKind::from_reason_code`] — when the string
    /// matches a known kind, builds via `from_typed_rejection`
    /// (typed kind field set); when no match, falls back to
    /// `new()` (kind=None, legacy shape).
    ///
    /// **P1-3 (CB-3 legacy envelope compat)**: callers SHOULD pass
    /// `kind` via `from_typed_rejection` directly. This factory is
    /// a soft fallback and silently swallows unknown reason_codes
    /// into `kind=None`. Use [`LegacyKindStatus`] (read path) or
    /// log warnings at the call site if caller intent is unknown.
    pub fn from_reason_code_or_legacy(
        hat: impl Into<String>,
        topic: impl Into<String>,
        reason_code: impl Into<String>,
        retry_count: u32,
    ) -> Self {
        let code: String = reason_code.into();
        match crate::preset::engine::gates::RejectionKind::from_reason_code(&code) {
            Some(kind) => Self::from_typed_rejection(hat, topic, kind, retry_count),
            None => Self::new(hat, topic, code, retry_count),
        }
    }

    /// 2026-06-23 fix plan P1-3 (CB-3 legacy envelope compat):
    /// round-trip helper that does NOT silently swallow unknown
    /// reason_codes. Returns a [`LegacyKindStatus`] the caller
    /// can match on for diagnostics. Pre-existing
    /// [`read_rejection_log`] is kept for backwards compatibility
    /// — this variant is the **explicit** path.
    ///
    /// `workspace` is the repo root (where `.ralph/recovery.jsonl`
    /// lives). `line_index` selects which record in the log to
    /// inspect; passes through `read_rejection_log` ordering.
    pub fn classify_legacy_envelope(
        workspace: &Path,
        line_index: usize,
    ) -> std::io::Result<Option<LegacyKindStatus>> {
        let records = read_rejection_log(workspace)?;
        let Some(record) = records.get(line_index) else {
            return Ok(None);
        };
        Ok(classify_record(&record))
    }

    /// Mark the record as terminal (R11 escalation).  Returns
    /// the mutated value.
    pub fn with_terminal_reason(mut self, reason: impl Into<String>) -> Self {
        self.terminal_reason = Some(reason.into());
        self
    }

    /// Stable retry key — `hat+topic+reason_code`.  Mirrors the
    /// shape used by `Rejection::compute_retry_key` minus the
    /// leading `stage:` prefix (the `reason_code` field already
    /// includes the stage).
    pub fn retry_key(&self) -> String {
        format!("{}:{}:{}", self.hat, self.topic, self.reason_code)
    }
}

/// 2026-06-23 fix plan P1-3 (CB-3 legacy envelope compat):
/// explicit status for legacy envelope round-trip — distinguishes
/// "typed kind present and matches" vs "reason_code parsed to known
/// kind" vs "reason_code unknown, kind=None is a SILENT LOSS".
/// Callers that need to surface unknown reason_codes (ops alerting)
/// should use this instead of the implicit `kind: Option<String>`
/// field, which hides the same info inside the deserialised struct.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LegacyKindStatus {
    /// Typed `kind` field was present and matched the
    /// reason_code (the strict SSOT path — both fields set).
    Typed(crate::preset::engine::gates::RejectionKind),
    /// `kind` field was absent in the envelope but the
    /// reason_code mapped to a known `RejectionKind` via
    /// [`RejectionKind::from_reason_code`]. Round-trip via
    /// `from_reason_code_or_legacy` succeeds.
    LegacyFromReasonCode(crate::preset::engine::gates::RejectionKind),
    /// reason_code did NOT match any known `RejectionKind`.
    /// Caller that uses `from_reason_code_or_legacy` will
    /// silently produce `kind=None`; this status signals to
    /// the caller that they should warn / escalate.
    UnknownReasonCode(String),
}

/// 2026-06-23 fix plan P1-3 (CB-3 legacy envelope compat):
/// classify a single record into a [`LegacyKindStatus`]. The
/// classifier is a free function so it can also be called from
/// `RejectionRecord::classify_legacy_envelope` and from tests.
fn classify_record(record: &RejectionRecord) -> Option<LegacyKindStatus> {
    // Strict path: typed kind field present.
    if let Some(kind_str) = record.kind.as_deref() {
        if let Some(kind) =
            crate::preset::engine::gates::RejectionKind::from_reason_code(kind_str)
        {
            return Some(LegacyKindStatus::Typed(kind));
        }
        // Typed kind present but unrecognised — treat as
        // UnknownReasonCode for safety (callers should be
        // alerted about a kind drift).
        return Some(LegacyKindStatus::UnknownReasonCode(
            record.reason_code.clone(),
        ));
    }
    // Legacy path: reason_code string.
    if let Some(kind) =
        crate::preset::engine::gates::RejectionKind::from_reason_code(&record.reason_code)
    {
        return Some(LegacyKindStatus::LegacyFromReasonCode(kind));
    }
    // Unknown reason_code — this is the silent-loss path the
    // CB-3 fix is designed to surface.
    Some(LegacyKindStatus::UnknownReasonCode(record.reason_code.clone()))
}

/// Resolve the workspace-rooted path of the rejection log.
/// Returns `<workspace>/.ralph/recovery.jsonl`.  The directory
/// is created on demand.
pub fn recovery_log_path(workspace: &Path) -> PathBuf {
    workspace.join(RECOVERY_LOG_RELATIVE_PATH)
}

/// Append a single record to the rejection log.  Best-effort:
/// any I/O error is returned so the caller can log it (the loop
/// runner calls this inside `tracing::warn!`).
pub fn append_rejection(workspace: &Path, record: &RejectionRecord) -> std::io::Result<()> {
    let path = recovery_log_path(workspace);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new().create(true).append(true).open(&path)?;
    let mut writer = std::io::BufWriter::new(file);
    serde_json::to_writer(&mut writer, record).map_err(std::io::Error::other)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

/// Read every record currently in the rejection log.  Returns
/// an empty `Vec` when the file does not exist or is empty.
/// Malformed lines are skipped (best-effort: the file is meant
/// for `tail -f` first, structured parsing second).
pub fn read_rejection_log(workspace: &Path) -> std::io::Result<Vec<RejectionRecord>> {
    let path = recovery_log_path(workspace);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = fs::File::open(&path)?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<RejectionRecord>(trimmed) {
            records.push(record);
        }
    }
    Ok(records)
}

/// Return the retry count for `retry_key` by counting records
/// with the same `(hat, topic, reason_code)` tuple.  Used by
/// `RecoveryResponder`-adjacent paths to recover the per-key
/// counter across restarts.
pub fn retry_count_for(workspace: &Path, retry_key: &str) -> u32 {
    read_rejection_log(workspace)
        .map(|records| {
            records
                .iter()
                .filter(|r| r.retry_key() == retry_key)
                .count() as u32
        })
        .unwrap_or(0)
}

/// Delete the rejection log.  Test-only helper.
#[cfg(test)]
pub fn reset_rejection_log(workspace: &Path) -> std::io::Result<()> {
    let path = recovery_log_path(workspace);
    if path.exists() {
        fs::remove_file(path)?;
    }
    Ok(())
}

/// RFC3339-ish timestamp using `chrono::Utc::now()`.  Kept
/// private so tests can substitute deterministic clocks in the
/// future without touching the public API.
fn now_rfc3339() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn append_creates_file_and_dir() {
        let dir = TempDir::new().unwrap();
        let record = RejectionRecord::new("executor", "work.done", "policy:missing_field", 1);
        append_rejection(dir.path(), &record).unwrap();
        let path = recovery_log_path(dir.path());
        assert!(path.exists(), "recovery.jsonl should be created");
        let content = fs::read_to_string(&path).unwrap();
        assert_eq!(content.lines().count(), 1);
    }

    #[test]
    fn read_returns_appended_records() {
        let dir = TempDir::new().unwrap();
        let r1 = RejectionRecord::new("a", "t.x", "policy:missing_field", 1);
        let r2 = RejectionRecord::new("b", "t.y", "origin:unknown_hat", 2);
        append_rejection(dir.path(), &r1).unwrap();
        append_rejection(dir.path(), &r2).unwrap();
        let records = read_rejection_log(dir.path()).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].hat, "a");
        assert_eq!(records[1].retry_count, 2);
    }

    #[test]
    fn read_skips_malformed_lines() {
        let dir = TempDir::new().unwrap();
        let path = recovery_log_path(dir.path());
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        let mut f = fs::File::create(&path).unwrap();
        f.write_all(b"{\"ts\":\"now\",\"hat\":\"a\",\"topic\":\"x\",\"reason_code\":\"r\",\"retry_count\":1,\"terminal_reason\":null}\n").unwrap();
        f.write_all(b"not-json\n").unwrap();
        f.write_all(b"{\"ts\":\"now\",\"hat\":\"b\",\"topic\":\"y\",\"reason_code\":\"r2\",\"retry_count\":2,\"terminal_reason\":null}\n").unwrap();
        drop(f);
        let records = read_rejection_log(dir.path()).unwrap();
        assert_eq!(records.len(), 2);
    }

    #[test]
    fn read_empty_log_returns_empty_vec() {
        let dir = TempDir::new().unwrap();
        let records = read_rejection_log(dir.path()).unwrap();
        assert!(records.is_empty());
    }

    #[test]
    fn retry_count_for_filters_by_key() {
        let dir = TempDir::new().unwrap();
        let r1 = RejectionRecord::new("a", "t.x", "policy:missing_field", 1);
        let r2 = RejectionRecord::new("a", "t.x", "policy:missing_field", 2);
        let r3 = RejectionRecord::new("a", "t.y", "policy:missing_field", 1);
        append_rejection(dir.path(), &r1).unwrap();
        append_rejection(dir.path(), &r2).unwrap();
        append_rejection(dir.path(), &r3).unwrap();
        let key_a_x = format!("{}:{}:{}", "a", "t.x", "policy:missing_field");
        assert_eq!(retry_count_for(dir.path(), &key_a_x), 2);
        let key_a_y = format!("{}:{}:{}", "a", "t.y", "policy:missing_field");
        assert_eq!(retry_count_for(dir.path(), &key_a_y), 1);
    }

    #[test]
    fn with_terminal_reason_serialises_field() {
        let r =
            RejectionRecord::new("a", "x", "r", 3).with_terminal_reason("retry budget exhausted");
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains("terminal_reason"));
        assert!(s.contains("retry budget exhausted"));
    }

    #[test]
    fn retry_key_shape_matches_rejection() {
        let r = RejectionRecord::new("executor", "work.done", "policy:missing_field", 1);
        assert_eq!(r.retry_key(), "executor:work.done:policy:missing_field");
    }

    // U6 (plan 2026-06-23-004): typed kind envelope 测试。
    mod recovery_envelope_typed {
        use super::*;
        use crate::preset::engine::gates::RejectionKind;

        #[test]
        fn factory_method_ssot_kind_matches_reason_code() {
            // from_typed_rejection: kind 与 reason_code 必从同一 kind SSOT 派生
            let r = RejectionRecord::from_typed_rejection(
                "executor",
                "work.ready",
                RejectionKind::HandoffFilenameMismatch,
                3,
            );
            assert_eq!(
                r.kind.as_deref(),
                Some("hat_handoff_filename_mismatch")
            );
            assert_eq!(
                r.reason_code, "hat_handoff_filename_mismatch",
                "kind and reason_code MUST come from the same RejectionKind SSOT"
            );
        }

        #[test]
        fn factory_method_covers_all_hat_handoff_kinds() {
            for kind in [
                RejectionKind::HandoffFilenameMismatch,
                RejectionKind::HandoffStructureInvalid,
                RejectionKind::HandoffIllegalEmitTopic,
            ] {
                let r = RejectionRecord::from_typed_rejection(
                    "executor",
                    "work.ready",
                    kind,
                    1,
                );
                assert_eq!(r.kind.as_deref(), Some(kind.reason_code()));
            }
        }

        #[test]
        fn legacy_record_without_kind_deserializes_with_none() {
            // 反序列化兼容:老 envelope(无 kind 字段)能反序列化,kind = None。
            let dir = TempDir::new().unwrap();
            let path = recovery_log_path(dir.path());
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            let legacy = b"{\"ts\":\"2026-06-23T00:00:00Z\",\"hat\":\"a\",\"topic\":\"x\",\"reason_code\":\"r\",\"retry_count\":1,\"terminal_reason\":null}\n";
            fs::write(&path, legacy).unwrap();
            let records = read_rejection_log(dir.path()).unwrap();
            assert_eq!(records.len(), 1);
            assert!(
                records[0].kind.is_none(),
                "legacy envelope without kind field MUST deserialize as None"
            );
        }

        #[test]
        fn typed_kind_serializes_for_grep() {
            // 消费侧可以按 kind grep:`jq 'select(.kind == "hat_handoff_filename_mismatch")'`
            let r = RejectionRecord::from_typed_rejection(
                "executor",
                "work.ready",
                RejectionKind::HandoffFilenameMismatch,
                5,
            );
            let s = serde_json::to_string(&r).unwrap();
            assert!(s.contains("\"kind\":\"hat_handoff_filename_mismatch\""));
        }

        #[test]
        fn append_and_read_round_trip_preserves_kind() {
            let dir = TempDir::new().unwrap();
            let r = RejectionRecord::from_typed_rejection(
                "executor",
                "work.ready",
                RejectionKind::HandoffStructureInvalid,
                2,
            );
            append_rejection(dir.path(), &r).unwrap();
            let records = read_rejection_log(dir.path()).unwrap();
            assert_eq!(records.len(), 1);
            assert_eq!(
                records[0].kind.as_deref(),
                Some("hat_handoff_structure_invalid")
            );
        }

        /// 2026-06-23 fix plan U6 (CB-3): legacy reason_code strings
        /// (from `extract_reason_code(&violation)` in
        /// `correction/mod.rs`) MUST round-trip through
        /// `from_reason_code_or_legacy` and surface the typed
        /// kind. Known kinds set `kind=Some(_)`; unknown strings
        /// fall back to `kind=None`.
        #[test]
        fn from_reason_code_or_legacy_typed_kind_for_known_reason_codes() {
            for reason in [
                "missing_field",
                "topic_ownership",
                "upstream_state",
                "handoff_artifact",
                "pre_check",
                "hat_handoff_filename_mismatch",
                "hat_handoff_structure_invalid",
                "hat_handoff_illegal_emit_topic",
            ] {
                let r = RejectionRecord::from_reason_code_or_legacy(
                    "executor",
                    "work.ready",
                    reason,
                    1,
                );
                assert_eq!(
                    r.kind.as_deref(),
                    Some(reason),
                    "known reason `{reason}` MUST surface as typed kind"
                );
                assert_eq!(r.reason_code, reason);
            }
        }

        #[test]
        fn from_reason_code_or_legacy_falls_back_for_unknown_reason() {
            // Unknown reason (legacy free-form) keeps kind=None.
            let r = RejectionRecord::from_reason_code_or_legacy(
                "executor",
                "work.ready",
                "totally_unknown_legacy_reason",
                1,
            );
            assert!(
                r.kind.is_none(),
                "unknown reason code MUST fall back to kind=None"
            );
            assert_eq!(r.reason_code, "totally_unknown_legacy_reason");
        }

        /// 2026-06-23 fix plan P1-3 (CB-3 legacy envelope compat):
        /// the `classify_legacy_envelope` helper MUST surface an
        /// `UnknownReasonCode` status for old envelopes whose
        /// reason_code is not in the known kind vocabulary —
        /// callers that previously silently produced
        /// `kind=None` can now match on the status and emit a
        /// `tracing::warn!` for ops visibility.
        #[test]
        fn legacy_envelope_round_trip_warns_on_unknown_reason_code() {
            use std::fs;
            let dir = TempDir::new().unwrap();
            let path = recovery_log_path(dir.path());
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            // Old envelope: NO `kind` field, reason_code is unknown.
            let legacy = b"{\"ts\":\"2026-06-23T00:00:00Z\",\"hat\":\"a\",\"topic\":\"x\",\"reason_code\":\"unknown_reason_xxx\",\"retry_count\":1,\"terminal_reason\":null}\n";
            fs::write(&path, legacy).unwrap();

            // The legacy tolerant path produces kind=None (existing
            // behaviour preserved for backwards compatibility).
            let records = read_rejection_log(dir.path()).unwrap();
            assert_eq!(records.len(), 1);
            assert!(
                records[0].kind.is_none(),
                "legacy envelope without kind field MUST keep kind=None on read"
            );

            // The explicit P1-3 path surfaces UnknownReasonCode so
            // callers can warn / escalate.
            let status = RejectionRecord::classify_legacy_envelope(dir.path(), 0).unwrap();
            match status {
                Some(LegacyKindStatus::UnknownReasonCode(reason)) => {
                    assert_eq!(
                        reason, "unknown_reason_xxx",
                        "UnknownReasonCode MUST carry the original reason_code for ops diagnostics"
                    );
                }
                other => panic!(
                    "P1-3: legacy envelope with unknown reason_code MUST surface UnknownReasonCode, got {other:?}"
                ),
            }
        }

        /// 2026-06-23 fix plan P1-3 (CB-3 legacy envelope compat):
        /// when the `kind` field IS present (typed path), the
        /// helper classifies as `Typed(_)` and the round-trip
        /// is lossless.
        #[test]
        fn legacy_envelope_with_typed_kind_classifies_as_typed() {
            use std::fs;
            let dir = TempDir::new().unwrap();
            let path = recovery_log_path(dir.path());
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            let typed = b"{\"ts\":\"2026-06-23T00:00:00Z\",\"hat\":\"a\",\"topic\":\"x\",\"reason_code\":\"hat_handoff_filename_mismatch\",\"kind\":\"hat_handoff_filename_mismatch\",\"retry_count\":1,\"terminal_reason\":null}\n";
            fs::write(&path, typed).unwrap();
            let status = RejectionRecord::classify_legacy_envelope(dir.path(), 0).unwrap();
            match status {
                Some(LegacyKindStatus::Typed(kind)) => {
                    assert_eq!(
                        kind,
                        crate::preset::engine::gates::RejectionKind::HandoffFilenameMismatch
                    );
                }
                other => panic!(
                    "P1-3: typed envelope MUST classify as Typed(_), got {other:?}"
                ),
            }
        }

        /// 2026-06-23 fix plan P1-3 (CB-3 legacy envelope compat):
        /// legacy envelope without `kind` field but with a known
        /// reason_code (e.g. `hat_handoff_structure_invalid`)
        /// MUST classify as `LegacyFromReasonCode` — caller can
        /// either rebuild the typed record or pass it through.
        #[test]
        fn legacy_envelope_without_kind_but_known_reason_classifies_legacy() {
            use std::fs;
            let dir = TempDir::new().unwrap();
            let path = recovery_log_path(dir.path());
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            let legacy_known = b"{\"ts\":\"2026-06-23T00:00:00Z\",\"hat\":\"a\",\"topic\":\"x\",\"reason_code\":\"hat_handoff_structure_invalid\",\"retry_count\":1,\"terminal_reason\":null}\n";
            fs::write(&path, legacy_known).unwrap();
            let status = RejectionRecord::classify_legacy_envelope(dir.path(), 0).unwrap();
            match status {
                Some(LegacyKindStatus::LegacyFromReasonCode(kind)) => {
                    assert_eq!(
                        kind,
                        crate::preset::engine::gates::RejectionKind::HandoffStructureInvalid
                    );
                }
                other => panic!(
                    "P1-3: legacy envelope with known reason_code MUST classify as LegacyFromReasonCode, got {other:?}"
                ),
            }
        }
    }
}
