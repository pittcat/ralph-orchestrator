use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use crate::diagnosis::{DriftJournalEntry, RecoveryDiagnosisEnvelope};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrchestrationEntry {
    pub timestamp: String,
    pub iteration: u32,
    pub hat: String,
    pub event: OrchestrationEvent,
    /// Stable loop id (R5: lets operators distinguish primary from worktree
    /// loops in the timeline). Set by the runner via `OrchestrationLogger::log_with_context`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_id: Option<String>,
    /// Absolute path of the workspace this entry was recorded in. Together
    /// with `loop_id` this is the canonical anchor for cross-worktree
    /// forensics (R5: cross-workspace task stores must not interleave).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<String>,
    /// Business/display hat — the hat the event was attributed to by the
    /// Coordinator-mode `display_hat` resolution. May differ from `hat`
    /// (the executor hat, typically `ralph`) when outputs are remapped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub business_hat: Option<String>,
    /// Executor hat that physically produced the output. In Coordinator
    /// mode this is usually `ralph`, but the entry's primary `hat` field
    /// stays as the business attribution for index stability.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_hat: Option<String>,
    /// Task id from the event payload, when present. Used by the loop
    /// isolation test (R5: the same task_id may exist in two worktrees;
    /// the workspace + loop_id prefix disambiguates them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OrchestrationEvent {
    IterationStarted,
    HatSelected {
        hat: String,
        reason: String,
    },
    EventPublished {
        topic: String,
    },
    BackpressureTriggered {
        reason: String,
    },
    LoopTerminated {
        reason: String,
    },
    TaskAbandoned {
        reason: String,
    },
    WaveStarted {
        wave_id: String,
        expected_total: u32,
        worker_hat: String,
        concurrency: u32,
    },
    WaveInstanceCompleted {
        wave_id: String,
        index: u32,
        duration_ms: u64,
        cost_usd: f64,
    },
    WaveInstanceFailed {
        wave_id: String,
        index: u32,
        error: String,
        duration_ms: u64,
    },
    WaveCompleted {
        wave_id: String,
        total_results: u32,
        total_failures: u32,
        timed_out: bool,
        duration_ms: u64,
    },
    /// Execution contract was rejected for an event (U6).
    ExecutionContractRejected {
        topic: String,
        violation_kind: String,
        message: String,
    },
    /// Targeted contract recovery was routed (or could not be routed) to a
    /// source hat (2026-06-04 plan U7). When `retry_target` is `None` and
    /// `no_retry_reason` is `Some`, the rejected event has no safe recovery
    /// path and operators must intervene.
    ContractRecoveryRouted {
        topic: String,
        retry_target: Option<String>,
        no_retry_reason: Option<String>,
    },
    // ── U3 diagnosis audit events ────────────────────────────────────
    //
    // These variants are the *high-level audit* counterpart of the
    // detail-heavy entries written to `recovery.jsonl` and
    // `drift.jsonl`. They carry enough information for the
    // orchestration timeline to show *what happened* without
    // re-parsing the journal files. The detail (full envelope,
    // evidence, notes) lives in the journal.
    /// A new recovery diagnosis was emitted (U4 integration point).
    /// Detail goes to `recovery.jsonl` via
    /// [`crate::diagnostics::DiagnosticsCollector::log_recovery`].
    RecoveryDiagnosed {
        diagnosis_id: String,
        source: String,
        target_hat: Option<String>,
        topic: Option<String>,
        severity: String,
        reason_code: String,
        retry_key: String,
    },
    /// A recovery diagnosis was escalated after repeated failures
    /// (U6). Detail goes to `recovery.jsonl`.
    RecoveryEscalated {
        diagnosis_id: String,
        retry_key: String,
        attempt: u32,
        reason: String,
    },
    /// A drift finding was emitted (U5). Detail goes to `drift.jsonl`
    /// via [`crate::diagnostics::DiagnosticsCollector::log_drift`].
    DriftDetected {
        finding_id: String,
        metric: String,
        topic: Option<String>,
        field: Option<String>,
        severity: String,
    },
}

impl OrchestrationEvent {
    /// Map a [`RecoveryDiagnosisEnvelope`] to the
    /// [`OrchestrationEvent::RecoveryDiagnosed`] high-level audit
    /// variant. Used by the collector's `log_recovery` path (and by
    /// U4 callers that emit both journal + audit events).
    #[must_use]
    pub fn from_recovery_envelope(env: &RecoveryDiagnosisEnvelope) -> Self {
        OrchestrationEvent::RecoveryDiagnosed {
            diagnosis_id: env.diagnosis_id.clone(),
            source: env.source.as_str().to_string(),
            target_hat: env.target_hat.clone(),
            topic: env.topic.clone(),
            severity: env.severity.as_str().to_string(),
            reason_code: env.reason_code.clone(),
            retry_key: env.retry_key.clone(),
        }
    }

    /// Map a [`DriftJournalEntry`] to the
    /// [`OrchestrationEvent::DriftDetected`] high-level audit
    /// variant. Used by the collector's `log_drift` path.
    #[must_use]
    pub fn from_drift_entry(entry: &DriftJournalEntry) -> Self {
        OrchestrationEvent::DriftDetected {
            finding_id: entry.finding_id.clone(),
            metric: entry.metric.as_str().to_string(),
            topic: entry.topic.clone(),
            field: entry.field.clone(),
            severity: entry.severity.as_str().to_string(),
        }
    }
}

pub struct OrchestrationLogger {
    writer: BufWriter<File>,
}

impl OrchestrationLogger {
    pub fn new(session_dir: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(session_dir.join("orchestration.jsonl"))?;
        Ok(Self {
            writer: BufWriter::new(file),
        })
    }

    pub fn log(
        &mut self,
        iteration: u32,
        hat: &str,
        event: OrchestrationEvent,
    ) -> std::io::Result<()> {
        let entry = OrchestrationEntry {
            timestamp: chrono::Local::now().to_rfc3339(),
            iteration,
            hat: hat.to_string(),
            event,
            loop_id: None,
            workspace: None,
            business_hat: None,
            executor_hat: None,
            task_id: None,
        };
        serde_json::to_writer(&mut self.writer, &entry)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }

    /// Variant of `log` that records loop/workspace/business hat attribution
    /// alongside the iteration.  Used by the runner when a Coordinator-mode
    /// `display_hat` is in play, or when entries are emitted from a known
    /// loop scope (R5: the timeline must carry enough metadata to point an
    /// operator at the right workspace + loop + business hat + task).
    pub fn log_with_context(
        &mut self,
        iteration: u32,
        hat: &str,
        event: OrchestrationEvent,
        ctx: &OrchestrationContext<'_>,
    ) -> std::io::Result<()> {
        let entry = OrchestrationEntry {
            timestamp: chrono::Local::now().to_rfc3339(),
            iteration,
            hat: hat.to_string(),
            event,
            loop_id: ctx.loop_id.map(str::to_string),
            workspace: ctx.workspace.map(str::to_string),
            business_hat: ctx.business_hat.map(str::to_string),
            executor_hat: ctx.executor_hat.map(str::to_string),
            task_id: ctx.task_id.map(str::to_string),
        };
        serde_json::to_writer(&mut self.writer, &entry)?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()?;
        Ok(())
    }
}

/// Optional context carried by an orchestration entry.  Every field is
/// optional so existing callers (which only know iteration + hat) can keep
/// using `OrchestrationLogger::log`.  Runners that emit diagnostics in
/// Coordinator mode should prefer `log_with_context` and pass at minimum
/// the business hat so forensics don't have to re-derive it from the bus.
#[derive(Debug, Clone, Default)]
pub struct OrchestrationContext<'a> {
    pub loop_id: Option<&'a str>,
    pub workspace: Option<&'a str>,
    pub business_hat: Option<&'a str>,
    pub executor_hat: Option<&'a str>,
    pub task_id: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use tempfile::TempDir;

    #[test]
    fn test_all_event_types_serialize() {
        let events = vec![
            OrchestrationEvent::IterationStarted,
            OrchestrationEvent::HatSelected {
                hat: "ralph".to_string(),
                reason: "pending_events".to_string(),
            },
            OrchestrationEvent::EventPublished {
                topic: "build.start".to_string(),
            },
            OrchestrationEvent::BackpressureTriggered {
                reason: "tests failed".to_string(),
            },
            OrchestrationEvent::LoopTerminated {
                reason: "completion_promise".to_string(),
            },
            OrchestrationEvent::TaskAbandoned {
                reason: "max_iterations".to_string(),
            },
            OrchestrationEvent::WaveStarted {
                wave_id: "w-abc12345".to_string(),
                expected_total: 3,
                worker_hat: "reviewer".to_string(),
                concurrency: 4,
            },
            OrchestrationEvent::WaveInstanceCompleted {
                wave_id: "w-abc12345".to_string(),
                index: 0,
                duration_ms: 5000,
                cost_usd: 0.05,
            },
            OrchestrationEvent::WaveInstanceFailed {
                wave_id: "w-abc12345".to_string(),
                index: 1,
                error: "backend timeout".to_string(),
                duration_ms: 30000,
            },
            OrchestrationEvent::WaveCompleted {
                wave_id: "w-abc12345".to_string(),
                total_results: 2,
                total_failures: 1,
                timed_out: false,
                duration_ms: 35000,
            },
            OrchestrationEvent::RecoveryDiagnosed {
                diagnosis_id: "diag-id".to_string(),
                source: "missing_event_gate".to_string(),
                target_hat: Some("builder".to_string()),
                topic: Some("work.done".to_string()),
                severity: "warning".to_string(),
                reason_code: "no_emit".to_string(),
                retry_key: "missing_event_gate:builder:work_done:no_emit:*".to_string(),
            },
            OrchestrationEvent::RecoveryEscalated {
                diagnosis_id: "diag-id".to_string(),
                retry_key: "stall_recovery:builder:*:*:stall:*".to_string(),
                attempt: 3,
                reason: "retry_window_exhausted".to_string(),
            },
            OrchestrationEvent::DriftDetected {
                finding_id: "find-id".to_string(),
                metric: "field_completeness".to_string(),
                topic: Some("work.done".to_string()),
                field: Some("plan_name".to_string()),
                severity: "warning".to_string(),
            },
        ];

        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let _: OrchestrationEvent = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn test_iteration_and_hat_captured() {
        let temp_dir = TempDir::new().unwrap();
        let mut logger = OrchestrationLogger::new(temp_dir.path()).unwrap();

        logger
            .log(
                5,
                "builder",
                OrchestrationEvent::HatSelected {
                    hat: "builder".to_string(),
                    reason: "tasks_ready".to_string(),
                },
            )
            .unwrap();

        drop(logger);

        let file = File::open(temp_dir.path().join("orchestration.jsonl")).unwrap();
        let reader = BufReader::new(file);
        let line = reader.lines().next().unwrap().unwrap();
        let entry: OrchestrationEntry = serde_json::from_str(&line).unwrap();

        assert_eq!(entry.iteration, 5);
        assert_eq!(entry.hat, "builder");
    }

    #[test]
    fn test_immediate_flush() {
        let temp_dir = TempDir::new().unwrap();
        let mut logger = OrchestrationLogger::new(temp_dir.path()).unwrap();

        logger
            .log(1, "ralph", OrchestrationEvent::IterationStarted)
            .unwrap();

        // Don't drop logger - verify file has content immediately
        let file = File::open(temp_dir.path().join("orchestration.jsonl")).unwrap();
        let reader = BufReader::new(file);
        let lines: Vec<_> = reader.lines().collect();
        assert_eq!(lines.len(), 1);
    }

    #[test]
    fn test_log_with_context_records_loop_workspace_business_hat() {
        // R5: when a Coordinator-mode runner writes a recovery diagnosis, the
        // entry must carry loop_id / workspace / business_hat / executor_hat
        // / task_id so a postmortem can point to the exact run, not just
        // the exact hat.
        let temp_dir = TempDir::new().unwrap();
        let mut logger = OrchestrationLogger::new(temp_dir.path()).unwrap();

        let ctx = OrchestrationContext {
            loop_id: Some("primary-20260606-002000"),
            workspace: Some("/worktrees/sleek-sparrow"),
            business_hat: Some("executor"),
            executor_hat: Some("ralph"),
            task_id: Some("task-anon-001"),
        };
        logger
            .log_with_context(
                3,
                "executor",
                OrchestrationEvent::ExecutionContractRejected {
                    topic: "work.done".into(),
                    violation_kind: "MissingPayloadField".into(),
                    message: "missing plan_path".into(),
                },
                &ctx,
            )
            .unwrap();

        drop(logger);

        let file = File::open(temp_dir.path().join("orchestration.jsonl")).unwrap();
        let reader = BufReader::new(file);
        let line = reader.lines().next().unwrap().unwrap();
        let entry: OrchestrationEntry = serde_json::from_str(&line).unwrap();

        assert_eq!(entry.iteration, 3);
        assert_eq!(entry.hat, "executor");
        assert_eq!(entry.loop_id.as_deref(), Some("primary-20260606-002000"));
        assert_eq!(
            entry.workspace.as_deref(),
            Some("/worktrees/sleek-sparrow")
        );
        assert_eq!(entry.business_hat.as_deref(), Some("executor"));
        assert_eq!(entry.executor_hat.as_deref(), Some("ralph"));
        assert_eq!(entry.task_id.as_deref(), Some("task-anon-001"));
    }

    #[test]
    fn test_log_with_context_omits_unset_fields_in_json() {
        // Backwards compat: callers that pass only some context fields
        // must produce a JSON line that omits the unset keys (so old
        // tooling that parses the file isn't confused by nulls).
        let temp_dir = TempDir::new().unwrap();
        let mut logger = OrchestrationLogger::new(temp_dir.path()).unwrap();

        let ctx = OrchestrationContext {
            loop_id: Some("loop-x"),
            workspace: None,
            business_hat: None,
            executor_hat: None,
            task_id: None,
        };
        logger
            .log_with_context(
                0,
                "ralph",
                OrchestrationEvent::IterationStarted,
                &ctx,
            )
            .unwrap();
        drop(logger);

        let raw = std::fs::read(temp_dir.path().join("orchestration.jsonl")).unwrap();
        let line = std::str::from_utf8(&raw).unwrap();
        assert!(line.contains("\"loop_id\":\"loop-x\""));
        assert!(!line.contains("\"workspace\""));
        assert!(!line.contains("\"business_hat\""));
        assert!(!line.contains("\"executor_hat\""));
        assert!(!line.contains("\"task_id\""));
    }

    #[test]
    fn test_u3_event_variants_serialize_roundtrip() {
        // The new U3 variants must round-trip through serde so the
        // audit timeline written by orchestration.jsonl can be
        // replayed by U7.
        let events = vec![
            OrchestrationEvent::RecoveryDiagnosed {
                diagnosis_id: "diag-1".to_string(),
                source: "missing_event_gate".to_string(),
                target_hat: Some("builder".to_string()),
                topic: Some("work.done".to_string()),
                severity: "warning".to_string(),
                reason_code: "no_emit".to_string(),
                retry_key: "missing_event_gate:builder:work_done:no_emit:*".to_string(),
            },
            OrchestrationEvent::RecoveryEscalated {
                diagnosis_id: "diag-2".to_string(),
                retry_key: "stall_recovery:builder:*:*:stall:*".to_string(),
                attempt: 3,
                reason: "retry_window_exhausted".to_string(),
            },
            OrchestrationEvent::DriftDetected {
                finding_id: "find-1".to_string(),
                metric: "field_completeness".to_string(),
                topic: Some("work.done".to_string()),
                field: Some("plan_name".to_string()),
                severity: "warning".to_string(),
            },
        ];
        for event in events {
            let json = serde_json::to_string(&event).unwrap();
            let v: serde_json::Value = serde_json::from_str(&json).unwrap();
            let tag = v.get("type").and_then(|t| t.as_str()).unwrap();
            assert!(
                matches!(
                    tag,
                    "recovery_diagnosed" | "recovery_escalated" | "drift_detected"
                ),
                "unexpected type tag: {tag}"
            );
            // round-trip
            let _: OrchestrationEvent = serde_json::from_str(&json).unwrap();
        }
    }

    #[test]
    fn test_from_recovery_envelope_maps_fields() {
        let env = RecoveryDiagnosisEnvelope::builder()
            .source(crate::diagnosis::DiagnosisSource::MissingEventGate)
            .severity(crate::diagnosis::DiagnosisSeverity::Warning)
            .iteration(4)
            .reason_code("no_emit")
            .message("builder missed work.done")
            .source_hat("builder")
            .target_hat("builder")
            .topic("work.done")
            .retry_key("missing_event_gate:builder:work_done:no_emit:*")
            .safe_target(true)
            .build();

        let event = OrchestrationEvent::from_recovery_envelope(&env);
        let entry = OrchestrationEntry {
            timestamp: "ts".to_string(),
            iteration: 4,
            hat: "builder".to_string(),
            event,
            loop_id: None,
            workspace: None,
            business_hat: None,
            executor_hat: None,
            task_id: None,
        };
        let json = serde_json::to_string(&entry).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["event"]["type"], "recovery_diagnosed");
        assert_eq!(v["event"]["source"], "missing_event_gate");
        assert_eq!(v["event"]["target_hat"], "builder");
        assert_eq!(v["event"]["topic"], "work.done");
        assert_eq!(v["event"]["severity"], "warning");
        assert_eq!(v["event"]["reason_code"], "no_emit");
        assert_eq!(
            v["event"]["retry_key"],
            "missing_event_gate:builder:work_done:no_emit:*"
        );
        assert_eq!(v["event"]["diagnosis_id"], env.diagnosis_id);
    }

    #[test]
    fn test_from_drift_entry_maps_fields() {
        use crate::diagnosis::{DiagnosisSeverity, DriftJournalEntry, DriftMetric};

        let entry = DriftJournalEntry::builder()
            .metric(DriftMetric::CoordJoinRate)
            .observed_value(0.3)
            .threshold(0.8)
            .severity(DiagnosisSeverity::Error)
            .topic("work.done")
            .to_topic("review.wave.ready")
            .from_topic("work.done")
            .field("plan_name")
            .window_iterations(20)
            .iteration(7)
            .message("coord join dropped to 30%")
            .build();
        let event = OrchestrationEvent::from_drift_entry(&entry);
        let json = serde_json::to_string(&event).unwrap();
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["type"], "drift_detected");
        assert_eq!(v["metric"], "coord_join_rate");
        assert_eq!(v["topic"], "work.done");
        assert_eq!(v["field"], "plan_name");
        assert_eq!(v["severity"], "error");
        assert_eq!(v["finding_id"], entry.finding_id);
    }
}
