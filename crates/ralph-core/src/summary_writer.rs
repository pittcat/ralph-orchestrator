//! Summary file generation for loop termination.
//!
//! Per spec: "On termination, the orchestrator writes `.ralph/agent/summary.md`"
//! with status, iterations, duration, task list, events summary, and commit info.

use crate::event_logger::EventHistory;
use crate::event_loop::{LoopState, TerminationReason};
use crate::landing::LandingResult;
use crate::loop_context::LoopContext;
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A single pointer to an operator-facing artifact, rendered as a
/// labelled bullet inside the `## Diagnostics` section appended by
/// [`SummaryWriter::append_diagnosis_hint`].
///
/// `label` is the human-readable name of the artifact (e.g.
/// `"Payload contract violation report"`); `relpath` is a path
/// relative to the workspace root so the link works whether the
/// operator reads the file from the repo or from a worktree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosisReference {
    /// Short human-readable label.
    pub label: String,
    /// Path to the artifact, relative to the workspace root.
    pub relpath: String,
}

/// Operator-facing hint appended to the end of `summary.md` when
/// diagnostics are available. The hint is intentionally small: it
/// never embeds the full report and never duplicates the on-disk
/// artifacts. The operator is expected to follow the links and run
/// `ralph diagnose` to drill down.
///
/// `None` (i.e. "no hint") is the only legal value when diagnostics
/// are disabled; the [`SummaryWriter::append_diagnosis_hint`] writer
/// treats that case as a no-op.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiagnosisHint {
    /// Diagnostics session directory, relative to the workspace root
    /// (e.g. `.ralph/diagnostics/2026-06-05T10-20-30`). `None` when
    /// the collector was not enabled for this run.
    pub session_relpath: Option<String>,
    /// Pre-formatted `ralph diagnose` command the operator can copy
    /// into a shell. `None` when there is no session to diagnose.
    pub diagnose_command: Option<String>,
    /// Additional pointers to root-level diagnostic files (e.g. a
    /// payload-contract violation report) that the operator should
    /// review alongside the session journal.
    pub references: Vec<DiagnosisReference>,
}

impl DiagnosisHint {
    /// True when the hint carries at least one operator-actionable
    /// piece of information (a session path, a command, or a
    /// reference). Used by the writer to decide whether emitting the
    /// `## Diagnostics` section is worthwhile at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.session_relpath.is_none()
            && self.diagnose_command.is_none()
            && self.references.is_empty()
    }
}

/// Writes the loop summary file on termination.
///
/// Per spec section "Exit Summary":
/// ```markdown
/// # Loop Summary
///
/// **Status:** Completed successfully
/// **Iterations:** 12
/// **Duration:** 23m 45s
///
/// ## Tasks
/// - [x] Add refresh token support
/// - [x] Update login endpoint
/// - [~] Add rate limiting (cancelled: out of scope)
///
/// ## Events
/// - 12 total events
/// - 6 build.task
/// - 5 build.done
/// - 1 build.blocked
///
/// ## Final Commit
/// abc1234: feat(auth): complete auth overhaul
/// ```
#[derive(Debug)]
pub struct SummaryWriter {
    path: PathBuf,
    /// Path to the events file for reading history.
    /// If None, uses the default path relative to current directory.
    events_path: Option<PathBuf>,
}

impl Default for SummaryWriter {
    fn default() -> Self {
        Self::new(".ralph/agent/summary.md")
    }
}

impl SummaryWriter {
    /// Creates a new summary writer with the given path.
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            events_path: None,
        }
    }

    /// Creates a summary writer using paths from a LoopContext.
    ///
    /// This ensures the writer outputs to the correct location and reads
    /// events from the correct events file when running in a worktree
    /// or other isolated workspace.
    pub fn from_context(context: &LoopContext) -> Self {
        Self {
            path: context.summary_path(),
            events_path: Some(context.resolve_events_path()),
        }
    }

    /// Writes the summary file based on loop state and termination reason.
    ///
    /// This is called by the orchestrator when the loop terminates.
    pub fn write(
        &self,
        reason: &TerminationReason,
        state: &LoopState,
        scratchpad_path: Option<&Path>,
        final_commit: Option<&str>,
    ) -> io::Result<()> {
        self.write_with_landing(reason, state, scratchpad_path, final_commit, None)
    }

    /// Writes the summary file with optional landing information.
    ///
    /// This is called by the orchestrator when the loop terminates with landing.
    pub fn write_with_landing(
        &self,
        reason: &TerminationReason,
        state: &LoopState,
        scratchpad_path: Option<&Path>,
        final_commit: Option<&str>,
        landing: Option<&LandingResult>,
    ) -> io::Result<()> {
        // Ensure parent directory exists
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }

        let content = self.generate_content_with_landing(
            reason,
            state,
            scratchpad_path,
            final_commit,
            landing,
        );
        fs::write(&self.path, content)
    }

    /// U6: append a `## Recovery Diagnosis` section to the summary
    /// file when the recovery responder produced a
    /// [`crate::diagnosis::TerminationHint`]. The section is
    /// advisory; it does not introduce a new termination reason.
    /// The hint's `retry_key` and `severity` are surfaced so the
    /// operator can jump to the matching `recovery.jsonl` entry
    /// without re-parsing the file.
    pub fn append_recovery_section(
        &self,
        hint: &crate::diagnosis::TerminationHint,
    ) -> io::Result<()> {
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut body = String::new();
        body.push_str("\n\n## Recovery Diagnosis\n\n");
        body.push_str("The runtime diagnosis responder escalated this loop to a pause/review state. The original termination reason above is the source of truth; this section is a pointer to the diagnosis journal.\n\n");
        body.push_str(&format!("- **Reason:** {}\n", hint.reason));
        body.push_str(&format!("- **Severity:** {}\n", hint.severity.as_str()));
        if let Some(retry_key) = &hint.retry_key {
            body.push_str(&format!("- **Retry key:** `{retry_key}`\n"));
        }
        body.push_str(
            "\nFor the full audit timeline (recoveries, escalations, drift), see \
             `.ralph/diagnostics/<session>/recovery.jsonl` and `orchestration.jsonl`.\n",
        );
        // Append-mode write so we do not clobber an existing summary.
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        std::io::Write::write_all(&mut file, body.as_bytes())?;
        Ok(())
    }

    /// U8: append a `## Diagnostics` section to the summary file when
    /// a [`DiagnosisHint`] is provided. The section is operator-facing
    /// and contains three things at most:
    ///
    /// 1. The diagnostics session directory (workspace-relative).
    /// 2. A copy-pasteable `ralph diagnose` command.
    /// 3. Zero or more labelled references to root-level diagnostic
    ///    files (e.g. a payload-contract violation report).
    ///
    /// The hint is intentionally additive: the existing summary body
    /// is preserved verbatim and the section is only emitted when at
    /// least one of the three fields is non-empty. When `hint` is
    /// `None` (the diagnostics-disabled case) the function is a
    /// no-op, so the loop runner can call it unconditionally.
    pub fn append_diagnosis_hint(&self, hint: Option<&DiagnosisHint>) -> io::Result<()> {
        let Some(hint) = hint else {
            return Ok(());
        };
        if hint.is_empty() {
            return Ok(());
        }

        let mut body = String::new();
        body.push_str("\n\n## Diagnostics\n\n");
        if let Some(session) = &hint.session_relpath {
            body.push_str(&format!("- Session: `{session}`\n"));
        }
        if let Some(cmd) = &hint.diagnose_command {
            body.push_str(&format!("- Run: `{cmd}`\n"));
        }
        for reference in &hint.references {
            body.push_str(&format!("- {}: `{}`\n", reference.label, reference.relpath));
        }

        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        // Append-mode write so we do not clobber an existing summary
        // written by [`Self::write`].
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        std::io::Write::write_all(&mut file, body.as_bytes())?;
        Ok(())
    }

    /// Generates the markdown content for the summary with optional landing info.
    fn generate_content_with_landing(
        &self,
        reason: &TerminationReason,
        state: &LoopState,
        scratchpad_path: Option<&Path>,
        final_commit: Option<&str>,
        landing: Option<&LandingResult>,
    ) -> String {
        let mut content = String::new();

        // Header
        content.push_str("# Loop Summary\n\n");

        // Status
        let status = self.status_text(reason);
        content.push_str(&format!("**Status:** {status}\n"));
        content.push_str(&format!("**Iterations:** {}\n", state.iteration));
        content.push_str(&format!(
            "**Duration:** {}\n",
            format_duration(state.elapsed())
        ));

        // Cost (if tracked)
        if state.cumulative_cost > 0.0 {
            content.push_str(&format!("**Est. cost:** ${:.2}\n", state.cumulative_cost));
        }

        // Rejection info (when stale-breaker triggered)
        if matches!(reason, TerminationReason::LoopStale) {
            if let Some(ref sig) = state.completion_rejection_signature {
                content.push_str(&format!(
                    "**Last rejection:** {} (repeated {} times)\n",
                    sig, state.consecutive_completion_rejections
                ));
            }
            content.push_str(
                "**Suggestion:** Review the rejection reason above. The loop was \
                stuck because the same completion rejection repeated without any meaningful \
                progress. Ensure the agent is making real progress (new business events, \
                task state changes, or workflow advancement) before emitting LOOP_COMPLETE.\n",
            );
        }

        // Tasks section (read from scratchpad if available)
        content.push('\n');
        content.push_str("## Tasks\n\n");
        if let Some(path) = scratchpad_path {
            if let Some(tasks) = self.extract_tasks(path) {
                content.push_str(&tasks);
            } else {
                content.push_str("_Scratchpad found, but no task section extracted._\n");
            }
        } else {
            content.push_str("_No scratchpad found._\n");
        }

        // Events section
        content.push('\n');
        content.push_str("## Events\n\n");
        content.push_str(&self.summarize_events());

        // Final commit section
        if let Some(commit) = final_commit {
            content.push('\n');
            content.push_str("## Final Commit\n\n");
            content.push_str(commit);
            content.push('\n');
        }

        // Landing section (if landing was performed)
        if let Some(landing_result) = landing {
            content.push('\n');
            content.push_str("## Landing\n\n");

            if landing_result.committed {
                content.push_str(&format!(
                    "- **Auto-committed:** Yes ({})\n",
                    landing_result.commit_sha.as_deref().unwrap_or("unknown")
                ));
            } else {
                content.push_str("- **Auto-committed:** No (working tree was clean)\n");
            }

            content.push_str(&format!(
                "- **Handoff:** `{}`\n",
                landing_result.handoff_path.display()
            ));

            if !landing_result.open_tasks.is_empty() {
                content.push_str(&format!(
                    "- **Open tasks:** {}\n",
                    landing_result.open_tasks.len()
                ));
            }

            if landing_result.stashes_cleared > 0 {
                content.push_str(&format!(
                    "- **Stashes cleared:** {}\n",
                    landing_result.stashes_cleared
                ));
            }

            content.push_str(&format!(
                "- **Working tree clean:** {}\n",
                if landing_result.working_tree_clean {
                    "Yes"
                } else {
                    "No"
                }
            ));
        }

        content
    }

    /// Returns a human-readable status based on termination reason.
    fn status_text(&self, reason: &TerminationReason) -> &'static str {
        match reason {
            TerminationReason::CompletionPromise => "Completed successfully",
            TerminationReason::MaxIterations => "Stopped: max iterations reached",
            TerminationReason::MaxRuntime => "Stopped: max runtime exceeded",
            TerminationReason::MaxCost => "Stopped: max cost exceeded",
            TerminationReason::ConsecutiveFailures => "Failed: too many consecutive failures",
            TerminationReason::LoopThrashing => "Failed: loop thrashing detected",
            TerminationReason::LoopStale => "Failed: stale loop detected",
            TerminationReason::ValidationFailure => "Failed: too many malformed JSONL events",
            TerminationReason::Stopped => "Stopped manually",
            TerminationReason::Interrupted => "Interrupted by signal",
            TerminationReason::RestartRequested => "Restarting by human request",
            TerminationReason::WorkspaceGone => "Failed: workspace directory removed",
            TerminationReason::Cancelled => "Cancelled gracefully (human rejection or timeout)",
            TerminationReason::PayloadContractViolation => "Failed: payload contract violation",
            TerminationReason::RecoveryExhausted { .. } => {
                "Failed: recovery retry window exhausted"
            }
            TerminationReason::ReviewFailed { .. } => {
                "Failed: review verdict failed and propagated to final mirror"
            }
            TerminationReason::ScopeViolationCircuitBreakerTripped { .. } => {
                "Failed: isolated scope violation circuit breaker tripped"
            }
            TerminationReason::RecoverablePayloadExhausted { .. } => {
                "Failed: recoverable-payload budget exhausted"
            }
        }
    }

    /// Extracts task lines from the scratchpad file.
    ///
    /// Looks for lines matching `- [ ]`, `- [x]`, or `- [~]` patterns.
    fn extract_tasks(&self, scratchpad_path: &Path) -> Option<String> {
        let content = fs::read_to_string(scratchpad_path).ok()?;
        let mut tasks = String::new();

        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("- [ ]")
                || trimmed.starts_with("- [x]")
                || trimmed.starts_with("- [~]")
            {
                tasks.push_str(trimmed);
                tasks.push('\n');
            }
        }

        if tasks.is_empty() { None } else { Some(tasks) }
    }

    /// Summarizes events from the event history file.
    fn summarize_events(&self) -> String {
        let history = match &self.events_path {
            Some(path) => EventHistory::new(path),
            None => EventHistory::default_path(),
        };

        if !history.path().exists() {
            return "_No event history file found._\n".to_string();
        }

        let records = match history.read_all() {
            Ok(r) => r,
            Err(_) => return "_Event history file exists but could not be read._\n".to_string(),
        };

        if records.is_empty() {
            return "_Event history file is empty._\n".to_string();
        }

        // Count events by topic
        let mut topic_counts: HashMap<String, usize> = HashMap::new();
        for record in &records {
            *topic_counts.entry(record.topic.clone()).or_insert(0) += 1;
        }

        let mut summary = format!("- {} total events\n", records.len());

        // Sort by count descending for consistent output
        let mut sorted: Vec<_> = topic_counts.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

        for (topic, count) in sorted {
            summary.push_str(&format!("- {} {}\n", count, topic));
        }

        summary
    }
}

/// Formats a duration as human-readable string (e.g., "23m 45s" or "1h 5m 30s").
fn format_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    let hours = total_secs / 3600;
    let minutes = (total_secs % 3600) / 60;
    let seconds = total_secs % 60;

    if hours > 0 {
        format!("{}h {}m {}s", hours, minutes, seconds)
    } else if minutes > 0 {
        format!("{}m {}s", minutes, seconds)
    } else {
        format!("{}s", seconds)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tempfile::TempDir;

    fn test_state() -> LoopState {
        LoopState {
            iteration: 12,
            consecutive_failures: 0,
            cumulative_cost: 1.50,
            started_at: Instant::now(),
            last_hat: None,
            consecutive_blocked: 0,
            last_blocked_hat: None,
            task_block_counts: std::collections::HashMap::new(),
            last_verdict_topic: None,
            abandoned_tasks: Vec::new(),
            abandoned_task_redispatches: 0,
            consecutive_malformed_events: 0,
            consecutive_hard_gates: 0,
            completion_requested: false,
            completion_honored: false,
            isolated_turn_business_event_accepted: false,
            hat_activation_counts: std::collections::HashMap::new(),
            exhausted_hats: std::collections::HashSet::new(),
            last_checkin_at: None,
            last_active_hat_ids: Vec::new(),
            last_activation_events: Vec::new(),
            seen_topics: std::collections::HashSet::new(),
            last_emitted_signature: None,
            consecutive_same_signature: 0,
            cancellation_requested: false,
            current_isolated_hat: None,
            workflow_progress: crate::event_loop::WorkflowProgress::new(),
            policy_runtime_state: None,
            state_machine_runtime_state: None,
            last_verdict_payload: None,
            last_upstream_verdict_payload: None,
            completion_rejection_signature: None,
            consecutive_completion_rejections: 0,
            // 2026-06-16-001 U5: progress-steward counters in the
            // test fixture. The default 0/0/false is the "fresh
            // state" the runtime expects at turn 0.
            consecutive_no_progress_turns: 0,
            consecutive_steward_activations: 0,
            steward_woken_this_turn: false,
            // 2026-06-16-001 U5: per-turn stall-detector flag,
            // reset at the start of every
            // `process_events_from_jsonl` call.
            stall_detector_had_events: false,
            last_rejection_fingerprint: 0,
            loop_start_sha: None,
            rejection_retry_counts: std::collections::HashMap::new(),
            scope_violation_circuit_breaker_tripped: None,
            rejection_last_iteration: std::collections::HashMap::new(),
            // 2026-06-18-001 plan U6: 测试 fixture 用空 digest
            recent_rejection_digest: std::collections::BTreeMap::new(),
            invariant_violation_count: 0,
            last_invariant_violation: None,
            review_step_tracker: crate::event_loop::review_step_state::ReviewStepTracker::default(),
            // WRC-U4: the test helper builds a default tracker; the
            // dispatch-deadline test (in event_loop/tests/handoff_dispatch.rs)
            // exercises a real deadline path.
            handoff_tracker: crate::workflow_contract::HandoffTracker::new(),
            // Unit 1 (2026-06-17-001 plan): test helper builds a
            // default empty registry; the dispatch-deadline test
            // (in event_loop/tests/handoff_dispatch.rs) exercises
            // a real deadline path.
            flow_lifecycle: crate::flow_lifecycle::FlowLifecycleRegistry::new(),
            stall_recovery_counts: std::collections::HashMap::new(),
            pending_recovery_hat: None,
            pending_synthesizer_timeout: None,
            last_ephemeral_relocations: Vec::new(),
            // Unit 3 (2026-06-16-002 plan) bootstrap gate flags.
            bootstrap_complete: false,
            bootstrap_failed: false,
            // Unit 2 (2026-06-16-002 plan) recoverable budget buffer.
            recoverable_exhaustion_buffer: Vec::new(),
            // U4 (2026-06-17-003 plan) work.done dedup set.
            work_done_seen_tasks: std::collections::HashSet::new(),
            // 2026-06-24 P1-2: fix-round counter starts empty.
            fix_round_counts: std::collections::HashMap::new(),
            // 2026-06-17-003 U1: state projector is lazily
            // initialised by the first enabled iteration; the
            // cache is empty until then.
            state_projection: None,
            // 2026-06-17-004 U2 (R3): per-hat activation clock.
            hat_activation_at: std::collections::HashMap::new(),
            // 2026-06-17-004 U3 (R4+R5): obligation-trigger
            // snapshot for the missing-event gate. Empty by default.
            pending_obligation_triggers: Vec::new(),
            // 2026-06-20-001 U4b: no lint hint on cold start;
            // populated by the CLI emit path or by the loop's
            // own engine gate when a topic is rejected.
            pending_lint_resume: None,
            // Plan 2026-06-20-001 KTD-7: cold start; no
            // circuit-breaker trip in the test fixture.
            consecutive_engine_gate_rejections: 0,
            lint_circuit_breaker_tripped: false,
            // 2026-06-23 fix: typed per-kind counters empty;
            // first rejection seeds a new bucket.
            consecutive_lint_rejections_by_kind: std::collections::HashMap::new(),
            // U3 (plan 2026-06-23-004): rejection stall 检测窗口
            // 在 test fixture 中保持空。
            stall_detector_rejection_window: Vec::new(),
            // U1 (plan 2026-06-21-002): the unified state ledger
            // is opt-in. The test fixture stays on the legacy
            // path by default.
            state_ledger: None,
            // U7a (plan 2026-06-21-002): deterministic
            // correction queue. Empty by default so the
            // existing test path stays green.
            prompt_context: crate::correction::PromptContext::default(),
            // 2026-06-23-005 F4: typed TerminationTrigger queue
            // stays empty in the test fixture (infrastructure-only;
            // `process_output` does not consume it).
            termination_triggers: std::collections::VecDeque::new(),
        }
    }

    /// RAII guard that switches the process cwd for the duration of a test
    /// and restores it on drop (including panic unwinds). Used by tests that
    /// depend on `EventHistory::default_path()` resolving to a path relative
    /// to cwd, so a leftover `.ralph/events.jsonl` in the workspace root
    /// (or any other directory) does not contaminate the assertion.
    struct CwdGuard {
        original: PathBuf,
    }

    impl CwdGuard {
        fn chdir(path: &Path) -> Self {
            let original = std::env::current_dir().expect("read current_dir");
            std::env::set_current_dir(path).expect("chdir for test isolation");
            Self { original }
        }
    }

    impl Drop for CwdGuard {
        fn drop(&mut self) {
            // Best-effort restore: if restoring fails the test environment
            // is already broken, and we cannot do anything useful here.
            let _ = std::env::set_current_dir(&self.original);
        }
    }

    #[test]
    fn test_status_text() {
        let writer = SummaryWriter::default();

        assert_eq!(
            writer.status_text(&TerminationReason::CompletionPromise),
            "Completed successfully"
        );
        assert_eq!(
            writer.status_text(&TerminationReason::MaxIterations),
            "Stopped: max iterations reached"
        );
        assert_eq!(
            writer.status_text(&TerminationReason::ConsecutiveFailures),
            "Failed: too many consecutive failures"
        );
        assert_eq!(
            writer.status_text(&TerminationReason::Interrupted),
            "Interrupted by signal"
        );
    }

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_secs(45)), "45s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m 5s");
        assert_eq!(format_duration(Duration::from_secs(3725)), "1h 2m 5s");
    }

    #[test]
    fn test_extract_tasks() {
        let tmp = TempDir::new().unwrap();
        let scratchpad = tmp.path().join("scratchpad.md");

        let content = r"# Tasks

Some intro text.

- [x] Implement feature A
- [ ] Implement feature B
- [~] Feature C (cancelled: not needed)

## Notes

More text here.
";
        fs::write(&scratchpad, content).unwrap();

        let writer = SummaryWriter::default();
        let tasks = writer.extract_tasks(&scratchpad).unwrap();

        assert!(tasks.contains("- [x] Implement feature A"));
        assert!(tasks.contains("- [ ] Implement feature B"));
        assert!(tasks.contains("- [~] Feature C"));
    }

    #[test]
    fn test_generate_content_basic() {
        let writer = SummaryWriter::default();
        let state = test_state();

        let content = writer.generate_content_with_landing(
            &TerminationReason::CompletionPromise,
            &state,
            None,
            Some("abc1234: feat(auth): add tokens"),
            None,
        );

        assert!(content.contains("# Loop Summary"));
        assert!(content.contains("**Status:** Completed successfully"));
        assert!(content.contains("**Iterations:** 12"));
        assert!(content.contains("**Est. cost:** $1.50"));
        assert!(content.contains("## Tasks"));
        assert!(content.contains("## Events"));
        assert!(content.contains("## Final Commit"));
        assert!(content.contains("abc1234: feat(auth): add tokens"));
    }

    #[test]
    fn test_write_creates_directory() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nested/dir/summary.md");

        let writer = SummaryWriter::new(&path);
        let state = test_state();

        writer
            .write(&TerminationReason::CompletionPromise, &state, None, None)
            .unwrap();

        assert!(path.exists());
        let content = fs::read_to_string(path).unwrap();
        assert!(content.contains("# Loop Summary"));
    }

    #[test]
    fn test_write_with_landing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("summary.md");

        let writer = SummaryWriter::new(&path);
        let state = test_state();

        let landing = LandingResult {
            committed: true,
            commit_sha: Some("abc1234".to_string()),
            handoff_path: tmp.path().join("handoff.md"),
            open_tasks: vec!["task-1".to_string(), "task-2".to_string()],
            stashes_cleared: 2,
            working_tree_clean: true,
        };

        writer
            .write_with_landing(
                &TerminationReason::CompletionPromise,
                &state,
                None,
                None,
                Some(&landing),
            )
            .unwrap();

        let content = fs::read_to_string(path).unwrap();
        assert!(content.contains("## Landing"));
        assert!(content.contains("**Auto-committed:** Yes (abc1234)"));
        assert!(content.contains("**Handoff:**"));
        assert!(content.contains("**Open tasks:** 2"));
        assert!(content.contains("**Stashes cleared:** 2"));
        assert!(content.contains("**Working tree clean:** Yes"));
    }

    #[test]
    fn test_scratchpad_exists_but_no_tasks() {
        let tmp = TempDir::new().unwrap();
        let scratchpad = tmp.path().join("scratchpad.md");
        fs::write(&scratchpad, "# Notes\n\nSome notes without task list.\n").unwrap();

        let writer = SummaryWriter::default();
        let state = test_state();

        let content = writer.generate_content_with_landing(
            &TerminationReason::CompletionPromise,
            &state,
            Some(&scratchpad),
            None,
            None,
        );

        assert!(content.contains("_Scratchpad found, but no task section extracted._"));
        assert!(!content.contains("_No scratchpad found._"));
    }

    #[test]
    fn test_events_file_missing() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("summary.md");

        // Isolate from any leftover `.ralph/events.jsonl` in the workspace
        // (or its parent directories) by running the assertion with cwd
        // pointed at the empty tempdir. CwdGuard restores the original
        // directory on drop, even when the assertion panics.
        let _cwd = CwdGuard::chdir(tmp.path());

        // Point to a non-existent events file
        let writer = SummaryWriter::new(&path);
        let state = test_state();

        let content = writer.generate_content_with_landing(
            &TerminationReason::CompletionPromise,
            &state,
            None,
            None,
            None,
        );

        assert!(content.contains("_No event history file found._"));
    }

    #[test]
    fn test_events_file_empty() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("summary.md");
        let events_path = tmp.path().join("events.jsonl");
        fs::write(&events_path, "").unwrap();

        let mut writer = SummaryWriter::new(&path);
        writer.events_path = Some(events_path);
        let state = test_state();

        let content = writer.generate_content_with_landing(
            &TerminationReason::CompletionPromise,
            &state,
            None,
            None,
            None,
        );

        assert!(content.contains("_Event history file is empty._"));
    }

    #[test]
    fn test_from_context_resolves_timestamped_events_via_marker() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().to_path_buf();

        // Create a LoopContext (primary)
        let ctx = LoopContext::primary(workspace.clone());

        // Create .ralph directory and write the current-events marker
        fs::create_dir_all(ctx.ralph_dir()).unwrap();
        let timestamped_events = workspace.join(".ralph/events-20250101-120000.jsonl");
        fs::write(
            &timestamped_events,
            r#"{"ts":"2025-01-01T12:00:00Z","topic":"build.task","payload":{}}"#,
        )
        .unwrap();
        fs::write(
            ctx.current_events_marker(),
            ".ralph/events-20250101-120000.jsonl\n",
        )
        .unwrap();

        let writer = SummaryWriter::from_context(&ctx);
        let state = test_state();

        let content = writer.generate_content_with_landing(
            &TerminationReason::CompletionPromise,
            &state,
            None,
            None,
            None,
        );

        // Should find the event via the marker, not report "No event history file found"
        assert!(
            content.contains("1 total events"),
            "Expected to find 1 event from timestamped file, but got:\n{}",
            content
        );
        assert!(
            !content.contains("_No event history file found._"),
            "Should not report missing file when marker points to existing events"
        );
    }

    #[test]
    fn test_from_context_falls_back_when_marker_missing() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path().to_path_buf();

        let ctx = LoopContext::primary(workspace.clone());
        fs::create_dir_all(ctx.ralph_dir()).unwrap();

        // No marker, no default events.jsonl → should report missing
        let writer = SummaryWriter::from_context(&ctx);
        let state = test_state();

        let content = writer.generate_content_with_landing(
            &TerminationReason::CompletionPromise,
            &state,
            None,
            None,
            None,
        );

        assert!(content.contains("_No event history file found._"));
    }

    // ───────────────────────────────────────────────────────────────
    // U8: append_diagnosis_hint
    // ───────────────────────────────────────────────────────────────

    fn write_summary_with_status(status: &str) -> (TempDir, SummaryWriter) {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("summary.md");
        let writer = SummaryWriter::new(&path);
        let state = test_state();
        writer
            .write(
                &TerminationReason::CompletionPromise,
                &state,
                None,
                Some("deadbeef: feat: example"),
            )
            .unwrap();
        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains(status), "summary missing '{status}':\n{body}");
        (tmp, writer)
    }

    #[test]
    fn test_append_diagnosis_hint_none_is_noop() {
        let (tmp, writer) = write_summary_with_status("Completed successfully");
        let path = tmp.path().join("summary.md");
        let before = fs::read_to_string(&path).unwrap();

        // No hint → no-op; existing body is preserved bit-for-bit.
        writer.append_diagnosis_hint(None).unwrap();

        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(before, after);
        assert!(
            !after.contains("## Diagnostics"),
            "summary must not contain a Diagnostics section when hint is None"
        );
    }

    #[test]
    fn test_append_diagnosis_hint_empty_does_not_emit_section() {
        let (tmp, writer) = write_summary_with_status("Completed successfully");
        let path = tmp.path().join("summary.md");
        let before = fs::read_to_string(&path).unwrap();

        // Empty hint (all None / empty vectors) → no-op; the section is
        // only meaningful when at least one field is populated.
        let hint = DiagnosisHint::default();
        assert!(hint.is_empty());
        writer.append_diagnosis_hint(Some(&hint)).unwrap();

        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(before, after);
        assert!(!after.contains("## Diagnostics"));
    }

    #[test]
    fn test_append_diagnosis_hint_adds_session_and_command() {
        let (tmp, writer) = write_summary_with_status("Completed successfully");
        let path = tmp.path().join("summary.md");

        let hint = DiagnosisHint {
            session_relpath: Some(".ralph/diagnostics/2026-06-05T10-20-30".to_string()),
            diagnose_command: Some("ralph diagnose --session latest".to_string()),
            references: Vec::new(),
        };
        writer.append_diagnosis_hint(Some(&hint)).unwrap();

        let body = fs::read_to_string(&path).unwrap();
        assert!(
            body.contains("## Diagnostics"),
            "Diagnostics section missing:\n{body}"
        );
        assert!(
            body.contains("Session: `.ralph/diagnostics/2026-06-05T10-20-30`"),
            "Session line missing:\n{body}"
        );
        assert!(
            body.contains("Run: `ralph diagnose --session latest`"),
            "Command line missing:\n{body}"
        );
        // Section appears AFTER the original summary body.
        let section_idx = body.find("## Diagnostics").unwrap();
        let status_idx = body.find("**Status:**").unwrap();
        let commit_idx = body.find("deadbeef: feat: example").unwrap();
        assert!(
            section_idx > status_idx,
            "Diagnostics section must appear after Status"
        );
        assert!(
            section_idx > commit_idx,
            "Diagnostics section must appear after Final Commit"
        );
    }

    #[test]
    fn test_append_diagnosis_hint_includes_payload_violation_reference() {
        let (tmp, writer) = write_summary_with_status("Completed successfully");
        let path = tmp.path().join("summary.md");

        let hint = DiagnosisHint {
            session_relpath: Some(".ralph/diagnostics/2026-06-05T10-20-30".to_string()),
            diagnose_command: Some("ralph diagnose --session latest".to_string()),
            references: vec![DiagnosisReference {
                label: "Payload contract violation report".to_string(),
                relpath: ".ralph/diagnostics/payload-contract-error-2026-06-05T12-34-56-789Z.json"
                    .to_string(),
            }],
        };
        writer.append_diagnosis_hint(Some(&hint)).unwrap();

        let body = fs::read_to_string(&path).unwrap();
        assert!(body.contains("## Diagnostics"));
        assert!(
            body.contains("Payload contract violation report: `.ralph/diagnostics/payload-contract-error-2026-06-05T12-34-56-789Z.json`"),
            "Violation reference line missing:\n{body}"
        );
    }

    #[test]
    fn test_append_diagnosis_hint_preserves_existing_summary() {
        let (tmp, writer) = write_summary_with_status("Completed successfully");
        let path = tmp.path().join("summary.md");

        // Capture the body BEFORE appending the hint so we can compare.
        let before = fs::read_to_string(&path).unwrap();
        let hint = DiagnosisHint {
            session_relpath: Some(".ralph/diagnostics/2026-06-05T10-20-30".to_string()),
            diagnose_command: None,
            references: Vec::new(),
        };
        writer.append_diagnosis_hint(Some(&hint)).unwrap();
        let after = fs::read_to_string(&path).unwrap();

        // Existing summary content is preserved verbatim; the hint is
        // appended at the very end.
        assert!(
            after.starts_with(&before),
            "append_diagnosis_hint must not mutate the existing body.\nbefore:\n{before}\n\nafter:\n{after}"
        );
        assert!(after.len() > before.len());
        assert!(
            after.ends_with(
                "## Diagnostics\n\n- Session: `.ralph/diagnostics/2026-06-05T10-20-30`\n"
            )
        );
    }

    #[test]
    fn test_append_diagnosis_hint_is_idempotent_when_called_twice() {
        let (tmp, writer) = write_summary_with_status("Completed successfully");
        let path = tmp.path().join("summary.md");

        let hint = DiagnosisHint {
            session_relpath: Some(".ralph/diagnostics/2026-06-05T10-20-30".to_string()),
            diagnose_command: Some("ralph diagnose --session latest".to_string()),
            references: Vec::new(),
        };
        writer.append_diagnosis_hint(Some(&hint)).unwrap();
        let once = fs::read_to_string(&path).unwrap();
        // Calling a second time with the same hint must double the section.
        // The runner is expected to call this exactly once, but the writer
        // contract is "append": we want loud test failures when callers
        // accidentally double-invoke, not silent data loss.
        writer.append_diagnosis_hint(Some(&hint)).unwrap();
        let twice = fs::read_to_string(&path).unwrap();
        assert_eq!(twice.matches("## Diagnostics").count(), 2);
        assert!(twice.len() > once.len());
    }

    #[test]
    fn test_diagnosis_hint_is_empty_predicate() {
        assert!(DiagnosisHint::default().is_empty());
        assert!(
            !DiagnosisHint {
                session_relpath: Some(".ralph/diagnostics/x".to_string()),
                diagnose_command: None,
                references: Vec::new(),
            }
            .is_empty()
        );
        assert!(
            !DiagnosisHint {
                session_relpath: None,
                diagnose_command: Some("ralph diagnose --session latest".to_string()),
                references: Vec::new(),
            }
            .is_empty()
        );
        assert!(
            !DiagnosisHint {
                session_relpath: None,
                diagnose_command: None,
                references: vec![DiagnosisReference {
                    label: "report".to_string(),
                    relpath: ".ralph/diagnostics/report.json".to_string(),
                }],
            }
            .is_empty()
        );
    }
}
