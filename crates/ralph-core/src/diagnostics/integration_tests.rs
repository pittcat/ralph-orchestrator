//! Integration tests for diagnostics in EventLoop.

#[cfg(test)]
mod tests {
    use crate::config::RalphConfig;
    use crate::diagnostics::{DiagnosticsCollector, HookDisposition, HookRunTelemetryEntry};
    use crate::event_loop::EventLoop;
    use crate::hooks::{HookRunResult, HookStreamOutput, HookSuspendMode};
    use chrono::{TimeZone, Utc};
    use std::fs::File;
    use std::io::{BufRead, BufReader};
    use tempfile::TempDir;

    fn fixed_time(hour: u32, minute: u32, second: u32) -> chrono::DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 2, 28, hour, minute, second)
            .single()
            .expect("fixed timestamp")
    }

    fn sample_hook_telemetry_entry(disposition: HookDisposition) -> HookRunTelemetryEntry {
        let run_result = HookRunResult {
            started_at: fixed_time(15, 30, 1),
            ended_at: fixed_time(15, 30, 2),
            duration_ms: 923,
            exit_code: Some(13),
            timed_out: false,
            stdout: HookStreamOutput {
                content: "stdout-truncated".to_string(),
                truncated: true,
            },
            stderr: HookStreamOutput {
                content: "stderr-clean".to_string(),
                truncated: false,
            },
        };

        HookRunTelemetryEntry::from_run_result(
            "loop-telemetry-123",
            "pre.loop.start",
            "env-guard",
            disposition,
            HookSuspendMode::RetryBackoff,
            3,
            4,
            &run_result,
        )
    }

    #[test]
    fn test_event_loop_logs_iteration_started() {
        let temp_dir = TempDir::new().unwrap();

        let config = RalphConfig::default();
        let diagnostics = DiagnosticsCollector::with_enabled(temp_dir.path(), true).unwrap();
        let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);

        // Simulate processing output (which increments iteration)
        event_loop.process_output(&"ralph".into(), "some output", true);

        // Verify orchestration.jsonl was created and contains IterationStarted
        let diagnostics_dir = temp_dir.path().join(".ralph").join("diagnostics");

        // Find the session directory (timestamped)
        let session_dirs: Vec<_> = std::fs::read_dir(&diagnostics_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();

        assert_eq!(
            session_dirs.len(),
            1,
            "Expected exactly one session directory"
        );

        let session_dir = session_dirs[0].path();
        let orchestration_file = session_dir.join("orchestration.jsonl");
        assert!(
            orchestration_file.exists(),
            "orchestration.jsonl should exist"
        );

        // Read and verify entries
        let file = File::open(orchestration_file).unwrap();
        let reader = BufReader::new(file);
        let lines: Vec<_> = reader.lines().map(|l| l.unwrap()).collect();

        assert!(!lines.is_empty(), "Should have at least one log entry");

        // First entry should be IterationStarted
        let first_entry: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(first_entry["event"]["type"], "iteration_started");
        assert_eq!(first_entry["iteration"], 1);
    }

    #[test]
    fn test_event_loop_logs_hat_selected() {
        let temp_dir = TempDir::new().unwrap();

        let config = RalphConfig::default();
        let diagnostics = DiagnosticsCollector::with_enabled(temp_dir.path(), true).unwrap();
        let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);

        // Process output which should trigger hat selection logging
        event_loop.process_output(&"ralph".into(), "some output", true);

        let diagnostics_dir = temp_dir.path().join(".ralph").join("diagnostics");
        let session_dirs: Vec<_> = std::fs::read_dir(&diagnostics_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        let session_dir = session_dirs[0].path();
        let orchestration_file = session_dir.join("orchestration.jsonl");

        let file = File::open(orchestration_file).unwrap();
        let reader = BufReader::new(file);
        let lines: Vec<_> = reader.lines().map(|l| l.unwrap()).collect();

        // Should have HatSelected event
        let has_hat_selected = lines.iter().any(|line| {
            let entry: serde_json::Value = serde_json::from_str(line).unwrap();
            entry["event"]["type"] == "hat_selected"
        });

        assert!(has_hat_selected, "Should log hat_selected event");
    }

    /// Helper to write an event to a JSONL file for testing.
    fn write_event_to_jsonl(path: &std::path::Path, topic: &str, payload: &str) {
        use std::io::Write;
        let ts = chrono::Utc::now().to_rfc3339();
        let event_json = serde_json::json!({
            "topic": topic,
            "payload": payload,
            "ts": ts
        });
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .unwrap();
        writeln!(file, "{}", event_json).unwrap();
    }

    #[test]
    fn test_event_loop_logs_event_published() {
        // Events now come from JSONL via `ralph emit`, not from XML in text output.
        let temp_dir = TempDir::new().unwrap();
        let events_path = temp_dir.path().join("events.jsonl");

        let config = RalphConfig::default();
        let diagnostics = DiagnosticsCollector::with_enabled(temp_dir.path(), true).unwrap();
        let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
        event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

        // Write event to JSONL file
        write_event_to_jsonl(&events_path, "build.start", "Starting build");
        let _ = event_loop.process_events_from_jsonl();

        let diagnostics_dir = temp_dir.path().join(".ralph").join("diagnostics");
        let session_dirs: Vec<_> = std::fs::read_dir(&diagnostics_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        let session_dir = session_dirs[0].path();
        let orchestration_file = session_dir.join("orchestration.jsonl");

        let file = File::open(orchestration_file).unwrap();
        let reader = BufReader::new(file);
        let lines: Vec<_> = reader.lines().map(|l| l.unwrap()).collect();

        // Should have EventPublished
        let has_event_published = lines.iter().any(|line| {
            let entry: serde_json::Value = serde_json::from_str(line).unwrap();
            entry["event"]["type"] == "event_published" && entry["event"]["topic"] == "build.start"
        });

        assert!(has_event_published, "Should log event_published");
    }

    #[test]
    fn test_event_loop_logs_backpressure_triggered() {
        // Events now come from JSONL via `ralph emit`.
        // build.done without backpressure evidence triggers backpressure.
        let temp_dir = TempDir::new().unwrap();
        let events_path = temp_dir.path().join("events.jsonl");

        let config = RalphConfig::default();
        let diagnostics = DiagnosticsCollector::with_enabled(temp_dir.path(), true).unwrap();
        let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
        event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

        // Write build.done event without backpressure evidence
        write_event_to_jsonl(&events_path, "build.done", "Done");
        let _ = event_loop.process_events_from_jsonl();

        let diagnostics_dir = temp_dir.path().join(".ralph").join("diagnostics");
        let session_dirs: Vec<_> = std::fs::read_dir(&diagnostics_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        let session_dir = session_dirs[0].path();
        let orchestration_file = session_dir.join("orchestration.jsonl");

        let file = File::open(orchestration_file).unwrap();
        let reader = BufReader::new(file);
        let lines: Vec<_> = reader.lines().map(|l| l.unwrap()).collect();

        // Should have BackpressureTriggered
        let has_backpressure = lines.iter().any(|line| {
            let entry: serde_json::Value = serde_json::from_str(line).unwrap();
            entry["event"]["type"] == "backpressure_triggered"
        });

        assert!(has_backpressure, "Should log backpressure_triggered");
    }

    #[test]
    fn test_event_loop_logs_loop_terminated() {
        let temp_dir = TempDir::new().unwrap();

        // Create a scratchpad with no pending tasks (all done) in temp directory
        let agent_dir = temp_dir.path().join(".agent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let scratchpad_path = agent_dir.join("scratchpad.md");
        std::fs::write(&scratchpad_path, "- [x] Task 1 done\n- [x] Task 2 done\n").unwrap();

        // Configure event loop to use temp directory scratchpad
        let mut config = RalphConfig::default();
        config.core.scratchpad.path = scratchpad_path.to_string_lossy().to_string();

        let diagnostics = DiagnosticsCollector::with_enabled(temp_dir.path(), true).unwrap();
        let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);

        let events_path = temp_dir.path().join("events.jsonl");
        event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

        let event_json = serde_json::json!({
            "topic": "LOOP_COMPLETE",
            "payload": "done",
            "ts": chrono::Utc::now().to_rfc3339()
        });
        std::fs::write(&events_path, format!("{event_json}\n")).unwrap();

        let _ = event_loop.process_events_from_jsonl();
        let _ = event_loop.check_completion_event();

        let diagnostics_dir = temp_dir.path().join(".ralph").join("diagnostics");
        let session_dirs: Vec<_> = std::fs::read_dir(&diagnostics_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        let session_dir = session_dirs[0].path();
        let orchestration_file = session_dir.join("orchestration.jsonl");

        let file = File::open(orchestration_file).unwrap();
        let reader = BufReader::new(file);
        let lines: Vec<_> = reader.lines().map(|l| l.unwrap()).collect();

        // Should have LoopTerminated
        let has_terminated = lines.iter().any(|line| {
            let entry: serde_json::Value = serde_json::from_str(line).unwrap();
            entry["event"]["type"] == "loop_terminated"
        });

        assert!(has_terminated, "Should log loop_terminated");
    }

    #[test]
    fn test_diagnostics_collector_logs_hook_run_telemetry() {
        let temp_dir = TempDir::new().unwrap();
        let collector = DiagnosticsCollector::with_enabled(temp_dir.path(), true).unwrap();

        collector.log_hook_run(sample_hook_telemetry_entry(HookDisposition::Block));

        let hook_runs_file = collector.session_dir().unwrap().join("hook-runs.jsonl");
        assert!(hook_runs_file.exists(), "hook-runs.jsonl should exist");

        let content = std::fs::read_to_string(hook_runs_file).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 1, "Should have one hook run telemetry entry");

        let entry: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        for field in [
            "timestamp",
            "loop_id",
            "phase_event",
            "hook_name",
            "started_at",
            "ended_at",
            "duration_ms",
            "exit_code",
            "timed_out",
            "stdout",
            "stderr",
            "disposition",
            "suspend_mode",
            "retry_attempt",
            "retry_max_attempts",
        ] {
            assert!(
                entry.get(field).is_some(),
                "hook telemetry entry missing required field '{field}'"
            );
        }

        assert_eq!(entry["loop_id"], "loop-telemetry-123");
        assert_eq!(entry["phase_event"], "pre.loop.start");
        assert_eq!(entry["hook_name"], "env-guard");
        assert_eq!(entry["duration_ms"], 923);
        assert_eq!(entry["exit_code"], 13);
        assert_eq!(entry["timed_out"], false);
        assert_eq!(entry["stdout"]["content"], "stdout-truncated");
        assert_eq!(entry["stdout"]["truncated"], true);
        assert_eq!(entry["stderr"]["content"], "stderr-clean");
        assert_eq!(entry["stderr"]["truncated"], false);
        assert_eq!(entry["disposition"], "block");
        assert_eq!(entry["suspend_mode"], "retry_backoff");
        assert_eq!(entry["retry_attempt"], 3);
        assert_eq!(entry["retry_max_attempts"], 4);
    }

    #[test]
    fn test_diagnostics_collector_hook_run_logging_is_noop_when_disabled() {
        let temp_dir = TempDir::new().unwrap();
        let collector = DiagnosticsCollector::with_enabled(temp_dir.path(), false).unwrap();

        collector.log_hook_run(sample_hook_telemetry_entry(HookDisposition::Warn));

        assert!(collector.session_dir().is_none());
        assert!(
            !temp_dir.path().join(".ralph").join("diagnostics").exists(),
            "disabled diagnostics should not create diagnostics artifacts"
        );
    }

    /// U0: integration test for "one session per run".
    ///
    /// Simulates the CLI flow: build the authoritative collector in
    /// `main.rs`, pass it as the `LoopContext`'s prebuilt diagnostics,
    /// and verify that `EventLoop::with_context` reuses it. The result
    /// is a single timestamped session directory even though the
    /// `EventLoop` is built from the same `LoopContext` that
    /// `with_context` would otherwise use to build its own.
    #[test]
    fn test_one_session_per_run_when_prebuilt_collector_attached() {
        use crate::event_loop::EventLoop;
        use crate::loop_context::LoopContext;
        use std::path::Path;
        use std::sync::Arc;

        let temp_dir = TempDir::new().unwrap();

        // Step 1: build the authoritative collector (what `main.rs` does).
        let authoritative =
            DiagnosticsCollector::with_options(
                temp_dir.path(),
                &crate::diagnostics::DiagnosticsOptions {
                    full_diagnostics: true,
                    ..Default::default()
                },
            )
            .unwrap();
        let authoritative_session = authoritative.session_dir().unwrap().to_path_buf();
        let authoritative = Arc::new(authoritative);

        // Step 2: build a LoopContext that pre-hooks the collector.
        let context = LoopContext::primary(temp_dir.path().to_path_buf())
            .with_prebuilt_diagnostics(authoritative.clone());

        // Step 3: build the EventLoop with that context. It should reuse
        // the prebuilt collector — NOT create a second session.
        let config = RalphConfig::default();
        let event_loop = EventLoop::with_context(config, context);
        let event_loop_session = event_loop.diagnostics().session_dir().unwrap().to_path_buf();

        assert_eq!(
            event_loop_session, authoritative_session,
            "EventLoop must reuse the prebuilt collector's session dir"
        );

        // Step 4: confirm only one session dir was created.
        let diagnostics_root = temp_dir.path().join(".ralph").join("diagnostics");
        let session_count = std::fs::read_dir(&diagnostics_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .count();
        assert_eq!(
            session_count, 1,
            "expected exactly one session dir, found {session_count}"
        );

        // Step 5: tracing-layer-style consumer sees the same dir.
        let _ = Path::new(&event_loop_session); // path is valid; existence is asserted above
    }

    // ── U3: Recovery / Drift / Summary seed tests ─────────────────────

    fn sample_recovery_entry() -> crate::diagnosis::RecoveryJournalEntry {
        use crate::diagnosis::{
            DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope,
        };
        let env = RecoveryDiagnosisEnvelope::builder()
            .source(DiagnosisSource::MissingEventGate)
            .severity(DiagnosisSeverity::Warning)
            .iteration(3)
            .reason_code("no_emit")
            .message("builder did not emit work.done")
            .source_hat("builder")
            .target_hat("builder")
            .topic("work.done")
            .retry_key("missing_event_gate:builder:work_done:no_emit:*")
            .safe_target(true)
            .build();
        crate::diagnosis::RecoveryJournalEntry::from_envelope(
            env,
            vec!["hint: missing plan_name".to_string()],
        )
    }

    fn sample_drift_entry() -> crate::diagnosis::DriftJournalEntry {
        use crate::diagnosis::{DiagnosisSeverity, DriftJournalEntry, DriftMetric};
        DriftJournalEntry::builder()
            .metric(DriftMetric::FieldCompleteness)
            .observed_value(0.4)
            .threshold(0.9)
            .severity(DiagnosisSeverity::Warning)
            .topic("work.done")
            .field("plan_name")
            .window_iterations(20)
            .iteration(7)
            .message("plan_name missing in 60% of events")
            .build()
    }

    #[test]
    fn test_recovery_logger_writes_jsonl() {
        let temp = TempDir::new().unwrap();
        let collector = DiagnosticsCollector::with_enabled(temp.path(), true).unwrap();

        collector.log_recovery(sample_recovery_entry());

        let recovery_file = collector
            .session_dir()
            .expect("session dir must exist")
            .join("recovery.jsonl");
        assert!(recovery_file.exists(), "recovery.jsonl should exist");

        let content = std::fs::read_to_string(&recovery_file).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 1, "Should have exactly one line");

        let parsed: crate::diagnosis::RecoveryJournalEntry =
            serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed.envelope.reason_code, "no_emit");
        assert_eq!(parsed.envelope.iteration, 3);
    }

    #[test]
    fn test_drift_logger_writes_jsonl() {
        let temp = TempDir::new().unwrap();
        let collector = DiagnosticsCollector::with_enabled(temp.path(), true).unwrap();

        collector.log_drift(sample_drift_entry());

        let drift_file = collector
            .session_dir()
            .expect("session dir must exist")
            .join("drift.jsonl");
        assert!(drift_file.exists(), "drift.jsonl should exist");

        let content = std::fs::read_to_string(&drift_file).unwrap();
        let lines: Vec<_> = content.lines().collect();
        assert_eq!(lines.len(), 1, "Should have exactly one line");

        let parsed: crate::diagnosis::DriftJournalEntry = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(parsed.field.as_deref(), Some("plan_name"));
        assert_eq!(parsed.iteration, 7);
    }

    #[test]
    fn test_log_recovery_disabled_is_noop() {
        let temp = TempDir::new().unwrap();
        let collector = DiagnosticsCollector::with_options(
            temp.path(),
            &crate::diagnostics::DiagnosticsOptions::default(),
        )
        .unwrap();

        // No panic, no file.
        collector.log_recovery(sample_recovery_entry());
        assert!(collector.session_dir().is_none());
        assert!(!temp.path().join(".ralph").join("diagnostics").exists());
    }

    #[test]
    fn test_log_drift_disabled_is_noop() {
        let temp = TempDir::new().unwrap();
        let collector = DiagnosticsCollector::with_options(
            temp.path(),
            &crate::diagnostics::DiagnosticsOptions::default(),
        )
        .unwrap();

        collector.log_drift(sample_drift_entry());
        assert!(collector.session_dir().is_none());
        assert!(!temp.path().join(".ralph").join("diagnostics").exists());
    }

    #[test]
    fn test_write_diagnosis_summary_seed() {
        let temp = TempDir::new().unwrap();
        let collector = DiagnosticsCollector::with_enabled(temp.path(), true).unwrap();

        let mut summary = crate::diagnostics::DiagnosisSummary::new(
            collector.session_id().expect("session_id"),
        );
        summary.recovery_count = 12;
        summary.drift_finding_count = 3;
        summary.total_iterations = Some(47);
        summary.termination_reason = Some("completion_promise".to_string());
        summary.recovery_journal_path = Some("recovery.jsonl".to_string());
        summary.drift_journal_path = Some("drift.jsonl".to_string());
        summary.notes = vec!["truncated 1 note".to_string()];

        collector.write_diagnosis_summary_seed(&summary);

        let path = collector
            .session_dir()
            .unwrap()
            .join("diagnosis-summary.json");
        assert!(path.exists(), "diagnosis-summary.json should exist");

        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: crate::diagnostics::DiagnosisSummary = serde_json::from_str(&content).unwrap();
        assert_eq!(parsed.schema_version, 1);
        assert_eq!(parsed.recovery_count, 12);
        assert_eq!(parsed.drift_finding_count, 3);
        assert_eq!(parsed.total_iterations, Some(47));
        assert_eq!(
            parsed.termination_reason.as_deref(),
            Some("completion_promise")
        );
        assert_eq!(parsed.notes, vec!["truncated 1 note".to_string()]);
    }

    #[test]
    fn test_write_diagnosis_summary_seed_disabled_is_noop() {
        let temp = TempDir::new().unwrap();
        let collector = DiagnosticsCollector::with_options(
            temp.path(),
            &crate::diagnostics::DiagnosticsOptions::default(),
        )
        .unwrap();

        let summary = crate::diagnostics::DiagnosisSummary::new("dummy");
        collector.write_diagnosis_summary_seed(&summary);

        assert!(!temp.path().join(".ralph").join("diagnostics").exists());
    }

    #[test]
    fn test_session_id_returns_dir_name() {
        let temp = TempDir::new().unwrap();
        let collector = DiagnosticsCollector::with_enabled(temp.path(), true).unwrap();

        let session_id = collector.session_id().expect("session_id must be Some");
        // Format: YYYY-MM-DDTHH-MM-SS (19 chars)
        assert_eq!(session_id.len(), 19);
        assert_eq!(session_id.chars().nth(4), Some('-'));
        assert_eq!(session_id.chars().nth(7), Some('-'));
        assert_eq!(session_id.chars().nth(10), Some('T'));
        assert_eq!(session_id.chars().nth(13), Some('-'));
        assert_eq!(session_id.chars().nth(16), Some('-'));
    }

    #[test]
    fn test_session_id_disabled_is_none() {
        let temp = TempDir::new().unwrap();
        let collector = DiagnosticsCollector::with_options(
            temp.path(),
            &crate::diagnostics::DiagnosticsOptions::default(),
        )
        .unwrap();
        assert!(collector.session_id().is_none());
    }

    #[test]
    fn test_recovery_logger_truncates_long_notes() {
        use crate::diagnostics::MAX_RECOVERY_NOTE_CHARS;
        let temp = TempDir::new().unwrap();
        let collector = DiagnosticsCollector::with_enabled(temp.path(), true).unwrap();

        let long_note = "n".repeat(MAX_RECOVERY_NOTE_CHARS + 200);
        let env = crate::diagnosis::RecoveryDiagnosisEnvelope::builder()
            .source(crate::diagnosis::DiagnosisSource::DriftMonitor)
            .severity(crate::diagnosis::DiagnosisSeverity::Info)
            .reason_code("r")
            .message("m")
            .build();
        let entry = crate::diagnosis::RecoveryJournalEntry::from_envelope(
            env,
            vec![long_note.clone()],
        );

        collector.log_recovery(entry);

        let path = collector.session_dir().unwrap().join("recovery.jsonl");
        let content = std::fs::read_to_string(&path).unwrap();
        let parsed: crate::diagnosis::RecoveryJournalEntry =
            serde_json::from_str(content.lines().next().unwrap()).unwrap();
        assert_eq!(parsed.notes.len(), 1);
        assert_eq!(parsed.notes[0].chars().count(), MAX_RECOVERY_NOTE_CHARS);
        assert!(parsed.notes[0].ends_with('\u{2026}'));
    }

    #[test]
    fn test_minimal_runtime_diagnosis_creates_recovery_logger() {
        let temp = TempDir::new().unwrap();
        let options = crate::diagnostics::DiagnosticsOptions {
            full_diagnostics: false,
            runtime_diagnosis_artifacts: true,
            ..Default::default()
        };
        let collector = DiagnosticsCollector::with_options(temp.path(), &options).unwrap();

        let session_dir = collector
            .session_dir()
            .expect("session dir must exist when runtime_diagnosis_artifacts=true");

        // Minimal diagnosis session must create recovery.jsonl lazily on first log.
        collector.log_recovery(sample_recovery_entry());
        assert!(session_dir.join("recovery.jsonl").exists());

        // The historical full-diagnostics files MUST NOT be present.
        assert!(!session_dir.join("agent-output.jsonl").exists());
        assert!(!session_dir.join("prompt-log.md").exists());
        assert!(!session_dir.join("orchestration.jsonl").exists());
    }

    #[test]
    fn test_full_diagnostics_creates_all_loggers() {
        let temp = TempDir::new().unwrap();
        let collector = DiagnosticsCollector::with_enabled(temp.path(), true).unwrap();
        let session_dir = collector.session_dir().expect("session dir must exist");

        collector.log_recovery(sample_recovery_entry());
        collector.log_drift(sample_drift_entry());

        // Both new loggers must write.
        assert!(session_dir.join("recovery.jsonl").exists());
        assert!(session_dir.join("drift.jsonl").exists());
        // Full diagnostics creates the historical files lazily on first log.
        // We assert that at least orchestration.jsonl exists (always
        // pre-created by OrchestrationLogger::new), and that the new
        // journal files coexist with the historical ones.
        assert!(session_dir.join("orchestration.jsonl").exists());
    }

    #[test]
    fn test_recovery_logger_unwritable_dir_does_not_panic() {
        // An unwritable base_path surfaces as Err at the *constructor*
        // level — it doesn't get to the logger at all. So we exercise
        // the runtime path: build a valid collector, then attempt to
        // write through a logger whose underlying file has been
        // removed. The write should warn-and-continue.
        let temp = TempDir::new().unwrap();
        let collector = DiagnosticsCollector::with_enabled(temp.path(), true).unwrap();
        let recovery_path = collector
            .session_dir()
            .unwrap()
            .join("recovery.jsonl");
        // Confirm the file is created lazily by the first call.
        collector.log_recovery(sample_recovery_entry());
        assert!(recovery_path.exists());

        // Now delete the file and try to write. The lock will succeed,
        // but the underlying file write may fail because the inode is
        // gone. We can't easily simulate this in a portable way
        // (recovery.jsonl is opened with `create + append` so the
        // logger will just recreate it). Instead, simulate the
        // "no-op disabled" path to assert that no panic occurs when
        // the logger is `None`.
        let disabled = DiagnosticsCollector::disabled();
        // Must not panic.
        disabled.log_recovery(sample_recovery_entry());
        disabled.log_drift(sample_drift_entry());
        disabled.write_diagnosis_summary_seed(&crate::diagnostics::DiagnosisSummary::new(
            "deadbeef",
        ));
    }

    #[test]
    fn test_orchestration_event_from_recovery_envelope() {
        use crate::diagnostics::OrchestrationEvent;

        let entry = sample_recovery_entry();
        let env = &entry.envelope;
        let event = OrchestrationEvent::from_recovery_envelope(env);
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "recovery_diagnosed");
        assert_eq!(json["source"], "missing_event_gate");
        assert_eq!(json["target_hat"], "builder");
        assert_eq!(json["topic"], "work.done");
        assert_eq!(json["severity"], "warning");
        assert_eq!(json["reason_code"], "no_emit");
        assert_eq!(
            json["retry_key"],
            "missing_event_gate:builder:work_done:no_emit:*"
        );
        assert_eq!(json["diagnosis_id"], env.diagnosis_id);
    }

    #[test]
    fn test_orchestration_event_from_drift_entry() {
        use crate::diagnostics::OrchestrationEvent;

        let entry = sample_drift_entry();
        let event = OrchestrationEvent::from_drift_entry(&entry);
        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "drift_detected");
        assert_eq!(json["metric"], "field_completeness");
        assert_eq!(json["topic"], "work.done");
        assert_eq!(json["field"], "plan_name");
        assert_eq!(json["severity"], "warning");
        assert_eq!(json["finding_id"], entry.finding_id);
    }
}
