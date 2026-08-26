//! Plan 2026-08-15-1823 (fix empty channel activation observability)
//! Unit 2: emit a `hat_activation_outcome` row into the existing
//! `runtime-trace.jsonl` sidecar after every isolated hat activation
//! close. The row carries the bounded raw facts that
//! `ralph-run-diagnosis` needs to distinguish backend failure /
//! watchdog timeout / user interrupt / channel routing failure /
//! agent-success-but-no-emit from each other.
//!
//! The activation outcome is **observation only**: it never alters
//! `task.resume`, `missing-terminal recovery`, retry budgets, or
//! any other runtime decision. The `status` value reflects the raw
//! channel/backend facts captured *before* the runner decides on
//! recovery.
//!
//! Status mapping (single source of truth — keep in sync with
//! `ralph-run-diagnosis` references, plan §6 implementation
//! constraint 2):
//!
//! | status        | when                                                       |
//! |---------------|------------------------------------------------------------|
//! | `merged`      | pre-merge channel existed with >0 bytes and merge succeeded |
//! | `empty`       | pre-merge channel existed with 0 bytes                     |
//! | `missing`     | pre-merge channel path did not exist                       |
//! | `unreadable`  | pre-merge channel existed but metadata/read failed         |
//! | `merge_failed`| pre-merge channel existed with >0 bytes and merge returned Err (non-empty merge failure) |
//! | `interrupted` | interrupt path (operator abort / signal)                   |

use std::path::Path;

use ralph_core::diagnostics::{DiagnosticsCollector, RuntimeTraceEntry, RuntimeTracePhase};
use ralph_core::event_loop::ProcessedEvents;
use ralph_core::{EventLoop, LoopContext, TerminationReason};
use ralph_proto::HatId;
use serde_json::{Map, Value};
#[cfg(test)]
use tracing::warn;

use super::execution::ExecutionOutcome;

/// Stable kind tag for activation outcome rows.
pub const ACTIVATION_OUTCOME_KIND: &str = "hat_activation_outcome";

/// Status of the activation outcome. The set of values is fixed by
/// the `ralph-run-diagnosis` skill contract; adding a new value
/// requires updating `skills/ralph-run-diagnosis/references/...`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivationOutcomeStatus {
    Merged,
    Empty,
    Missing,
    Unreadable,
    MergeFailed,
    Interrupted,
}

impl ActivationOutcomeStatus {
    /// Stable on-disk tag.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Merged => "merged",
            Self::Empty => "empty",
            Self::Missing => "missing",
            Self::Unreadable => "unreadable",
            Self::MergeFailed => "merge_failed",
            Self::Interrupted => "interrupted",
        }
    }
}

/// `channel_readable` for a given status. Returns `false` only for
/// `Missing` (no channel path resolved) and `Unreadable` (path
/// present but metadata failed); all other statuses report a
/// readable channel. Used by `ActivationOutcomeFacts` so the
/// three-state `Missing` / `Empty` / `Unreadable` distinction is
/// preserved in the trace row.
pub fn channel_readable_for(status: ActivationOutcomeStatus) -> bool {
    !matches!(
        status,
        ActivationOutcomeStatus::Missing | ActivationOutcomeStatus::Unreadable
    )
}

/// `channel_exists` for a given status. Returns `false` only for
/// `Missing`; all other statuses (including `Unreadable`) report
/// an existing channel path that just could not be read.
pub fn channel_exists_for(status: ActivationOutcomeStatus) -> bool {
    !matches!(status, ActivationOutcomeStatus::Missing)
}

/// U11 (R10): strip the workspace prefix from a channel path so
/// the `source_ref` field in the activation outcome row does not
/// leak the operator's local absolute path. Falls back to the
/// stable marker when the channel is outside the workspace,
/// or to `"<unknown>"` for a `None` channel path. The
/// `inner.rs:3722` empty-channel `warn!` and the `entry.rs:128`
/// interrupt-path `warn!` mirror this helper.
pub fn channel_reference_for_log(
    path: Option<&std::path::Path>,
    workspace: &std::path::Path,
) -> Option<String> {
    let p = path?;
    match p.strip_prefix(workspace) {
        Ok(stripped) => Some(stripped.display().to_string()),
        Err(_) => Some("<outside-workspace>".to_string()),
    }
}

/// Snapshot of the pre-merge channel state. The runner captures this
/// *before* invoking `merge_hat_channel` so the activation outcome
/// row can describe the raw state even when `merge_hat_channel`
/// deletes the channel file or returns an error.
#[derive(Debug, Clone)]
pub struct ChannelSnapshot {
    pub status: ActivationOutcomeStatus,
    /// Pre-merge channel bytes. `None` for `Missing` / `Unreadable`.
    pub bytes: Option<u64>,
    /// Short, workspace-relative reference to the channel (or its
    /// marker) for the trace row's `source_ref` field.
    pub reference: Option<String>,
}

/// Build a snapshot from a raw metadata read. Distinguishes
/// `missing` (path absent) from `unreadable` (path present but
/// metadata failed) per plan §3 D2 / §6 implementation constraint
/// 2.
#[cfg(test)]
pub fn snapshot_channel(channel_path: Option<&Path>) -> ChannelSnapshot {
    snapshot_channel_with_workspace(channel_path, None)
}

/// Build a snapshot while retaining only a workspace-relative or stable
/// short reference for the trace row.
pub fn snapshot_channel_with_workspace(
    channel_path: Option<&Path>,
    workspace: Option<&Path>,
) -> ChannelSnapshot {
    let Some(path) = channel_path else {
        return ChannelSnapshot {
            status: ActivationOutcomeStatus::Missing,
            bytes: None,
            reference: None,
        };
    };
    let reference = Some(match workspace {
        Some(workspace) => channel_reference_for_log(Some(path), workspace)
            .unwrap_or_else(|| "<unknown>".to_string()),
        None => path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "<unknown>".to_string()),
    });
    match std::fs::metadata(path) {
        Ok(meta) => {
            let bytes = meta.len();
            // After `prepare_hat_channel` the file is zero bytes; we
            // classify that as `empty`, not `merged`. The status is
            // *pre-merge* and only flips to `merged` after the merge
            // helper reports success AND bytes > 0 (see
            // `outcome_from_merge_result`).
            if bytes == 0 {
                ChannelSnapshot {
                    status: ActivationOutcomeStatus::Empty,
                    bytes: Some(0),
                    reference,
                }
            } else {
                // We do not yet know whether merge will succeed; the
                // caller refines this via `outcome_from_merge_result`.
                ChannelSnapshot {
                    status: ActivationOutcomeStatus::Empty, // placeholder, refined below
                    bytes: Some(bytes),
                    reference,
                }
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => ChannelSnapshot {
            status: ActivationOutcomeStatus::Missing,
            bytes: None,
            reference,
        },
        Err(_) => ChannelSnapshot {
            status: ActivationOutcomeStatus::Unreadable,
            bytes: None,
            reference,
        },
    }
}

/// Refine a snapshot after `merge_hat_channel` returns. The
/// merge-error branch distinguishes empty merge error
/// (`merge_hat_channel` returned Err AND pre-merge bytes were 0)
/// from non-empty merge error (pre-merge bytes > 0 but the
/// write/append failed).
pub fn refine_after_merge(mut snapshot: ChannelSnapshot, merge_succeeded: bool) -> ChannelSnapshot {
    if merge_succeeded {
        // If bytes > 0 we now know the merge happened; promote
        // to `merged`. For `Missing` / `Unreadable` / `Empty`
        // snapshots the merge cannot have succeeded, so the
        // status stays as captured.
        if matches!(snapshot.status, ActivationOutcomeStatus::Empty)
            && snapshot.bytes.map(|b| b > 0).unwrap_or(false)
        {
            snapshot.status = ActivationOutcomeStatus::Merged;
        }
    } else if matches!(snapshot.status, ActivationOutcomeStatus::Empty)
        && snapshot.bytes.map(|b| b > 0).unwrap_or(false)
    {
        snapshot.status = ActivationOutcomeStatus::MergeFailed;
    }
    // `merge_failed_on_interrupt` callers override the status
    // explicitly via `refine_for_interrupt`.
    snapshot
}

/// Override the snapshot status for interrupt-path calls. The
/// interrupt path never reaches the normal merge close, so the
/// status must be `interrupted` regardless of pre-merge state.
pub fn refine_for_interrupt(snapshot: ChannelSnapshot) -> ChannelSnapshot {
    ChannelSnapshot {
        status: ActivationOutcomeStatus::Interrupted,
        bytes: snapshot.bytes,
        reference: snapshot.reference,
    }
}

/// Bounded scalar facts for the activation outcome row.
///
/// The runner populates every field, then trims / bounds through
/// the existing trace logger cap. Fields that are not meaningful in
/// a given context (`processed_events` is `None` on interrupt)
/// become `null` in the serialized row — the contract allows `null`
/// for genuinely unavailable facts.
#[derive(Debug, Clone, Default)]
pub struct ActivationOutcomeFacts {
    pub loop_id: Option<String>,
    pub channel_exists: bool,
    pub channel_bytes: Option<u64>,
    pub channel_readable: bool,
    pub merge_succeeded: bool,
    pub backend_success: bool,
    pub backend_exit_code: Option<i32>,
    pub watchdog_timeout: bool,
    pub backend_termination: bool,
    pub output_bytes: u64,
    pub output_mentions_emit: bool,
    pub candidate_event_count: u64,
    pub accepted_event_count: u64,
    pub rejected_event_count: u64,
    pub wave_policy_rejection_count: u64,
    pub terminal_obligation_topics: Vec<String>,
}

impl ActivationOutcomeFacts {
    /// Build from processed events and wave-policy statistics
    /// pair, falling back to zeros / false when not provided
    /// (e.g. interrupt path).
    #[allow(dead_code)]
    pub fn from_processed(
        processed: Option<&ProcessedEvents>,
        wave_policy_rejections: usize,
        wave_raw_count: usize,
    ) -> Self {
        match processed {
            Some(processed) => {
                let accepted = processed.accepted_events.len() as u64;
                let rejected = (processed.had_rejected_events as u64)
                    + processed.contract_rejections.len() as u64
                    + wave_policy_rejections as u64;
                let raw = u64::from(processed.had_raw_events);
                let candidate = raw + wave_raw_count as u64;
                Self {
                    candidate_event_count: candidate,
                    accepted_event_count: accepted,
                    rejected_event_count: rejected,
                    wave_policy_rejection_count: wave_policy_rejections as u64,
                    ..Self::default()
                }
            }
            None => Self::default(),
        }
    }

    /// Single source of truth for the activation outcome row's
    /// bounded scalar fields. Replaces the 13-field literal that
    /// previously lived in `activation_outcome_close.rs` and
    /// `entry.rs::merge_isolated_channel_on_interrupt` so a
    /// future schema addition does not require syncing multiple
    /// construction sites.
    #[allow(dead_code)]
    #[allow(clippy::too_many_arguments)]
    pub fn from_runner(
        ctx: &LoopContext,
        event_loop: &EventLoop,
        hat: &HatId,
        snapshot: &ChannelSnapshot,
        merge_succeeded: bool,
        outcome: &ExecutionOutcome,
        output: &str,
        success: bool,
        backend_termination: Option<&TerminationReason>,
        output_mentions_emit: bool,
    ) -> Self {
        let refined = refine_after_merge(snapshot.clone(), merge_succeeded);
        let terminal_obligation_topics = event_loop
            .registry()
            .get_config(hat)
            .map(|hat| hat.terminal_events.clone())
            .unwrap_or_default();
        Self {
            loop_id: ctx.loop_id().map(|s| s.to_string()),
            channel_exists: channel_exists_for(refined.status),
            channel_bytes: refined.bytes,
            channel_readable: channel_readable_for(refined.status),
            merge_succeeded,
            backend_success: success,
            backend_exit_code: outcome.backend_exit_code,
            watchdog_timeout: outcome.watchdog_timeout,
            backend_termination: backend_termination.is_some(),
            output_bytes: output.len() as u64,
            output_mentions_emit,
            terminal_obligation_topics,
            ..Self::default()
        }
    }

    /// Project into a `serde_json::Value` matching the bounded
    /// scalar contract documented in plan §1.4. Field order is
    /// deterministic so the on-disk row is stable across runs.
    pub fn to_json(&self) -> Value {
        let mut obj = Map::new();
        if let Some(loop_id) = &self.loop_id {
            obj.insert("loop_id".into(), Value::String(loop_id.clone()));
        }
        obj.insert("channel_exists".into(), Value::Bool(self.channel_exists));
        obj.insert(
            "channel_bytes".into(),
            self.channel_bytes.map(Value::from).unwrap_or(Value::Null),
        );
        obj.insert(
            "channel_readable".into(),
            Value::Bool(self.channel_readable),
        );
        obj.insert("merge_succeeded".into(), Value::Bool(self.merge_succeeded));
        obj.insert("backend_success".into(), Value::Bool(self.backend_success));
        obj.insert(
            "backend_exit_code".into(),
            self.backend_exit_code
                .map(Value::from)
                .unwrap_or(Value::Null),
        );
        obj.insert(
            "watchdog_timeout".into(),
            Value::Bool(self.watchdog_timeout),
        );
        obj.insert(
            "backend_termination".into(),
            Value::Bool(self.backend_termination),
        );
        obj.insert("output_bytes".into(), Value::from(self.output_bytes));
        obj.insert(
            "output_mentions_emit".into(),
            Value::Bool(self.output_mentions_emit),
        );
        obj.insert(
            "candidate_event_count".into(),
            Value::from(self.candidate_event_count),
        );
        obj.insert(
            "accepted_event_count".into(),
            Value::from(self.accepted_event_count),
        );
        obj.insert(
            "rejected_event_count".into(),
            Value::from(self.rejected_event_count),
        );
        obj.insert(
            "wave_policy_rejection_count".into(),
            Value::from(self.wave_policy_rejection_count),
        );
        obj.insert(
            "terminal_obligation_topics".into(),
            Value::Array(
                self.terminal_obligation_topics
                    .iter()
                    .map(|s| Value::String(s.clone()))
                    .collect(),
            ),
        );
        Value::Object(obj)
    }
}

/// Append a single activation outcome row to the runtime trace
/// sidecar. Best-effort: a failure (logger `degraded`, missing
/// session dir, etc.) must never affect the loop. The function is
/// a thin wrapper around `RuntimeTraceLogger::append` so the cap,
/// schema_version, sequence and degraded-flip semantics stay in
/// one place.
pub fn log_activation_outcome_with_diagnostics(
    diagnostics: &DiagnosticsCollector,
    iteration: u64,
    hat: &str,
    snapshot: &ChannelSnapshot,
    facts: &ActivationOutcomeFacts,
) {
    if diagnostics.session_dir().is_none() {
        // Diagnostics disabled — do nothing. Plan §6 implementation
        // constraint 4: trace append failures must never change the
        // loop result.
        return;
    }
    let fields = facts.to_json();
    let mut entry = RuntimeTraceEntry::new(iteration, 0, RuntimeTracePhase::Activation)
        .with_kind(ACTIVATION_OUTCOME_KIND)
        .with_hat(hat)
        .with_status(snapshot.status.as_str())
        .with_fields(fields);
    if let Some(reference) = snapshot.reference.as_ref() {
        entry = entry.with_source_ref(reference.clone());
    }
    diagnostics.log_runtime_trace(entry);

    // Plan 2026-08-26-1104 U06: when the activation outcome is
    // abnormal (the channel could not be read or merged), flush
    // the bounded frozen evidence window so the boundary-coverage
    // reader (U7) and the attribution engine (U8) see the same
    // activation rows that preceeded the failure. Normal
    // statuses (`merged`, `empty`, `missing`, `interrupted`) do
    // not trigger the flush — those are expected and the
    // collector will keep accumulating evidence for a future
    // anomaly.
    if matches!(
        snapshot.status,
        ActivationOutcomeStatus::MergeFailed | ActivationOutcomeStatus::Unreadable,
    ) && let Err(err) = diagnostics.flush_evidence_window(
        ralph_core::diagnostics::AnomalyDescriptor {
            trigger_kind: ralph_core::diagnostics::trigger_kinds::ABNORMAL_ACTIVATION_OUTCOME
                .to_string(),
            ts: chrono::Utc::now().to_rfc3339(),
            iteration,
            details: Some(serde_json::json!({
                "hat": hat,
                "status": snapshot.status.as_str(),
            })),
        },
        vec![],
    )
    {
        warn!(
            target: "ralph_cli::loop_runner",
            iteration = iteration,
            error = %err,
            "failed to flush evidence-window on abnormal activation outcome",
        );
    }
}

/// Test-only compatibility helper for direct row-shape tests. Production
/// callers must use `log_activation_outcome_with_diagnostics` so writes go
/// through the collector's shared logger and degraded-state handling.
#[cfg(test)]
pub fn log_activation_outcome(
    session_dir: Option<&Path>,
    iteration: u64,
    hat: &str,
    snapshot: &ChannelSnapshot,
    facts: &ActivationOutcomeFacts,
) {
    let Some(dir) = session_dir else { return };
    let fields = facts.to_json();
    let mut entry = RuntimeTraceEntry::new(iteration, 0, RuntimeTracePhase::Activation)
        .with_kind(ACTIVATION_OUTCOME_KIND)
        .with_hat(hat)
        .with_status(snapshot.status.as_str())
        .with_fields(fields);
    if let Some(reference) = snapshot.reference.as_ref() {
        entry = entry.with_source_ref(reference.clone());
    }
    let Ok(mut logger) = ralph_core::diagnostics::RuntimeTraceLogger::new(dir) else {
        warn!(
            target: "ralph_cli::loop_runner",
            "failed to open runtime trace for activation outcome test helper"
        );
        return;
    };
    logger.append(entry);
}

/// Helper for tests: a one-liner that asserts the row was written
/// with the expected schema fields.
#[cfg(test)]
#[allow(dead_code)]
pub(crate) fn read_outcome_row(session_dir: &Path) -> Option<Value> {
    use std::io::{BufRead, BufReader};
    let path = session_dir.join("runtime-trace.jsonl");
    let file = std::fs::File::open(path).ok()?;
    for line in BufReader::new(file).lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if value.get("kind").and_then(Value::as_str) == Some(ACTIVATION_OUTCOME_KIND) {
            return Some(value);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_as_str_matches_contract() {
        // Lock the on-disk tag set. The diagnosis skill references
        // these strings; renaming one is a contract break.
        assert_eq!(ActivationOutcomeStatus::Merged.as_str(), "merged");
        assert_eq!(ActivationOutcomeStatus::Empty.as_str(), "empty");
        assert_eq!(ActivationOutcomeStatus::Missing.as_str(), "missing");
        assert_eq!(ActivationOutcomeStatus::Unreadable.as_str(), "unreadable");
        assert_eq!(
            ActivationOutcomeStatus::MergeFailed.as_str(),
            "merge_failed"
        );
        assert_eq!(ActivationOutcomeStatus::Interrupted.as_str(), "interrupted");
    }

    #[test]
    fn channel_readable_and_exists_truth_table() {
        // Missing → channel_exists=false, channel_readable=false.
        // Unreadable → channel_exists=true (path present), channel_readable=false.
        // Everything else → both true.
        let cases = [
            (ActivationOutcomeStatus::Merged, true, true),
            (ActivationOutcomeStatus::Empty, true, true),
            (ActivationOutcomeStatus::Missing, false, false),
            (ActivationOutcomeStatus::Unreadable, true, false),
            (ActivationOutcomeStatus::MergeFailed, true, true),
            (ActivationOutcomeStatus::Interrupted, true, true),
        ];
        for (status, expected_exists, expected_readable) in cases {
            assert_eq!(
                channel_exists_for(status),
                expected_exists,
                "channel_exists_for({status:?})",
            );
            assert_eq!(
                channel_readable_for(status),
                expected_readable,
                "channel_readable_for({status:?})",
            );
        }
    }

    #[test]
    fn channel_reference_for_log_strips_workspace_prefix() {
        let workspace = std::path::Path::new("/tmp/work");
        // Channel inside the workspace → workspace-relative path.
        let inner = std::path::Path::new("/tmp/work/.ralph/agent/events-hat-1.jsonl");
        let stripped = channel_reference_for_log(Some(inner), workspace);
        assert_eq!(
            stripped.as_deref(),
            Some(".ralph/agent/events-hat-1.jsonl"),
            "channel inside workspace must be stripped"
        );
        // Channel outside the workspace → use a stable non-sensitive marker.
        let outer = std::path::Path::new("/var/tmp/events-hat-1.jsonl");
        let fallback = channel_reference_for_log(Some(outer), workspace);
        assert_eq!(
            fallback.as_deref(),
            Some("<outside-workspace>"),
            "channel outside workspace must not expose an absolute path"
        );
        // None channel → None.
        assert!(channel_reference_for_log(None, workspace).is_none());
    }

    #[test]
    fn snapshot_channel_distinguishes_missing_unreadable_empty() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Missing: None path (no marker resolved) — the runner
        // produces this when `resolve_hat_channel_events_path`
        // returns None.
        let missing = snapshot_channel(None);
        assert_eq!(missing.status, ActivationOutcomeStatus::Missing);
        assert_eq!(missing.bytes, None);
        // Empty (zero bytes file)
        let empty_path = tmp.path().join("empty");
        std::fs::write(&empty_path, b"").unwrap();
        let empty = snapshot_channel(Some(&empty_path));
        assert_eq!(empty.status, ActivationOutcomeStatus::Empty);
        assert_eq!(empty.bytes, Some(0));
        // Missing: a resolved path whose file is absent.
        let non_existent_path = tmp.path().join("does-not-exist");
        let missing_file = snapshot_channel(Some(&non_existent_path));
        assert_eq!(
            missing_file.status,
            ActivationOutcomeStatus::Missing,
            "ENOENT must be Missing, not Unreadable"
        );
        assert_eq!(missing_file.bytes, None);
        // Unreadable: InvalidInput is a stable metadata failure that does
        // not depend on process privileges or platform file permissions.
        let invalid_path = std::path::Path::new("\0");
        let unreadable = snapshot_channel(Some(invalid_path));
        assert_eq!(unreadable.status, ActivationOutcomeStatus::Unreadable);
        assert_eq!(unreadable.bytes, None);
        // Non-empty (placeholder status, refined by caller)
        let non_empty_path = tmp.path().join("non-empty");
        std::fs::write(&non_empty_path, b"abc\n").unwrap();
        let non_empty = snapshot_channel(Some(&non_empty_path));
        assert_eq!(non_empty.bytes, Some(4));
        // Status is the placeholder before `refine_after_merge`;
        // the helper returns Empty here because bytes > 0 path
        // is just a placeholder branch the caller refines.
        assert!(
            matches!(
                non_empty.status,
                ActivationOutcomeStatus::Empty
                    | ActivationOutcomeStatus::Merged
                    | ActivationOutcomeStatus::MergeFailed
            ),
            "non-empty snapshot must be refineable to one of merged/empty/merge_failed"
        );
    }

    #[test]
    fn refine_after_merge_promotes_non_empty_on_success() {
        let snapshot = ChannelSnapshot {
            status: ActivationOutcomeStatus::Empty,
            bytes: Some(42),
            reference: Some("hat-channel:test".into()),
        };
        let refined = refine_after_merge(snapshot, true);
        assert_eq!(refined.status, ActivationOutcomeStatus::Merged);
        assert_eq!(refined.bytes, Some(42));
    }

    #[test]
    fn refine_after_merge_promotes_non_empty_on_failure_to_merge_failed() {
        let snapshot = ChannelSnapshot {
            status: ActivationOutcomeStatus::Empty,
            bytes: Some(42),
            reference: Some("hat-channel:test".into()),
        };
        let refined = refine_after_merge(snapshot, false);
        assert_eq!(refined.status, ActivationOutcomeStatus::MergeFailed);
    }

    #[test]
    fn refine_for_interrupt_overrides_status() {
        let snapshot = ChannelSnapshot {
            status: ActivationOutcomeStatus::Empty,
            bytes: Some(0),
            reference: Some("hat-channel:test".into()),
        };
        let refined = refine_for_interrupt(snapshot);
        assert_eq!(refined.status, ActivationOutcomeStatus::Interrupted);
        assert_eq!(refined.bytes, Some(0));
    }

    #[test]
    fn facts_to_json_carries_bounded_scalars() {
        let facts = ActivationOutcomeFacts {
            loop_id: Some("loop-7".into()),
            channel_exists: true,
            channel_bytes: Some(0),
            channel_readable: true,
            merge_succeeded: true,
            backend_success: true,
            backend_exit_code: Some(0),
            watchdog_timeout: false,
            backend_termination: false,
            output_bytes: 42,
            output_mentions_emit: false,
            candidate_event_count: 0,
            accepted_event_count: 0,
            rejected_event_count: 0,
            wave_policy_rejection_count: 0,
            terminal_obligation_topics: vec!["work.done".into()],
        };
        let json = facts.to_json();
        assert_eq!(json["loop_id"], "loop-7");
        assert_eq!(json["channel_exists"], true);
        assert_eq!(json["channel_bytes"], 0);
        assert_eq!(json["backend_exit_code"], 0);
        assert_eq!(json["output_bytes"], 42);
        assert_eq!(json["terminal_obligation_topics"][0], "work.done");
    }

    #[test]
    fn from_processed_carries_event_and_wave_counts() {
        let processed = ProcessedEvents {
            had_raw_events: true,
            had_rejected_events: true,
            accepted_events: Vec::new(),
            ..Default::default()
        };
        let facts = ActivationOutcomeFacts::from_processed(Some(&processed), 2, 3);
        assert_eq!(facts.candidate_event_count, 4);
        assert_eq!(facts.accepted_event_count, 0);
        assert_eq!(facts.rejected_event_count, 3);
        assert_eq!(facts.wave_policy_rejection_count, 2);
    }
}
