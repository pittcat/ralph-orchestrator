//! Focused helpers used while finalizing loop termination diagnostics.

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use ralph_core::diagnosis::TerminationHint;
use tracing::debug;

/// Snapshot one diagnostics artifact after all termination-side rows have
/// been flushed. Hashing is streamed so a large agent-output sidecar cannot
/// be loaded wholesale just to finalize the manifest.
pub(super) fn diagnostic_artifact_integrity(
    session_dir: &Path,
    name: &str,
) -> ralph_core::diagnostics::ArtifactIntegrity {
    use sha2::Digest as _;

    let path = session_dir.join(name);
    let metadata = fs::metadata(&path).ok();
    let hash_self_referential_manifest = name == "diagnosis-input.json";
    let mut sha256 = None;
    if !hash_self_referential_manifest
        && metadata.as_ref().is_some_and(std::fs::Metadata::is_file)
        && let Ok(mut file) = File::open(&path)
    {
        let mut hasher = sha2::Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        let mut readable = true;
        loop {
            match file.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => hasher.update(&buffer[..count]),
                Err(_) => {
                    readable = false;
                    break;
                }
            }
        }
        if readable {
            let digest = hasher.finalize();
            sha256 = Some(
                digest
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<String>(),
            );
        }
    }
    let status = match metadata.as_ref() {
        None => ralph_core::diagnostics::ArtifactStatus::Missing,
        Some(metadata) if !metadata.is_file() => ralph_core::diagnostics::ArtifactStatus::Degraded,
        Some(_) if hash_self_referential_manifest || sha256.is_some() => {
            ralph_core::diagnostics::ArtifactStatus::Present
        }
        Some(_) => ralph_core::diagnostics::ArtifactStatus::Degraded,
    };
    ralph_core::diagnostics::ArtifactIntegrity {
        path: name.to_string(),
        status,
        sha256,
        size_bytes: metadata.as_ref().map(std::fs::Metadata::len),
        last_modified: metadata
            .and_then(|value| value.modified().ok())
            .map(|value| chrono::DateTime::<chrono::Utc>::from(value).to_rfc3339()),
    }
}

pub(super) fn collect_idempotent_counts(
    event_loop: &ralph_core::EventLoop,
) -> (
    usize, /* recovery_count */
    usize, /* drift_finding_count */
    usize, /* task_count (informational only) */
) {
    let log_mutex = event_loop.idempotent_log();
    let mut guard = match log_mutex.lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    // `replay` rebuilds the in-memory index from disk; the
    // SC-5 measurement command expects the count to mirror what's persisted.
    let _ = guard.replay();
    let finals = guard.final_records();
    let counts =
        ralph_core::event_loop::idempotent_wiring::DiagnosisSummary::from_final_records(&finals);
    drop(guard);
    (
        counts.recovery_count,
        counts.drift_finding_count,
        counts.task_count,
    )
}

/// Return the execution capabilities that were actually available to this
/// loop. Keep this in one place so the initial bundle identity and its final
/// snapshot cannot disagree about supervisor/wave execution.
pub(super) fn execution_capabilities(config: &ralph_core::RalphConfig) -> Vec<String> {
    let supervisor = config.event_loop.supervisor.enabled;
    let wave = config.hats.values().any(|hat| {
        let extra = hat.extra_instructions.iter().map(String::as_str);
        std::iter::once(hat.instructions.as_str())
            .chain(extra)
            .any(|text| {
                text.contains("ralph wave emit")
                    || text.contains("ralph wave verify")
                    || text.contains("## WAVE CONTEXT")
            })
    });

    let mut capabilities = Vec::with_capacity(2);
    if supervisor {
        capabilities.push("supervisor".to_string());
    }
    if wave {
        capabilities.push("wave".to_string());
    }
    if capabilities.is_empty() {
        capabilities.push("single-chain".to_string());
    }
    capabilities
}

/// Refresh the session pointer on loop termination so diagnosis resolves the
/// final worktree session after its live loop record disappears.
pub(super) fn finalize_session_pointer(
    diagnostics: &ralph_core::diagnostics::DiagnosticsCollector,
    ctx: Option<&ralph_core::LoopContext>,
) {
    let Some(ctx) = ctx else {
        return;
    };
    if ctx.is_primary() || !diagnostics.is_enabled() {
        return;
    }
    match diagnostics.write_session_pointer(ctx.repo_root(), ctx.workspace()) {
        Ok(true) => {
            debug!(
                target: "ralph_cli::loop_runner",
                main_repo = %ctx.repo_root().display(),
                "refreshed session pointer on loop termination",
            );
        }
        Ok(false) => {}
        Err(err) => {
            tracing::warn!(
                target: "ralph_cli::loop_runner",
                main_repo = %ctx.repo_root().display(),
                error = %err,
                "failed to refresh session pointer on loop termination; \
                 ralph diagnose may not find this worktree session after the loop ends",
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Termination-side builders / finalizer (moved out of `inner.rs`).
//
// Step 4 (2026-09-03-0959 DAG 接线): `inner.rs` already sat above the
// 5000-line single-file budget, so the DAG observability seam could only
// land if an equivalent block moved out first. These three helpers are a
// pure, behaviour-preserving move — no signature, error-string, or timeout
// changes. `inner.rs` keeps `pub(crate) use` re-exports of the two
// `*_termination_diagnostics` builders so the existing
// `loop_runner::build_termination_diagnostics` paths (tests + mod.rs
// re-exports) keep resolving unchanged.
// ---------------------------------------------------------------------------

pub(crate) fn build_termination_diagnostics(
    event_loop: &ralph_core::EventLoop,
    payload_violation_report_relpath: Option<&str>,
) -> Option<(
    ralph_core::DiagnosisHint,
    ralph_core::diagnostics::DiagnosisSummary,
)> {
    // `session_id()` is implemented as `session_dir().file_name()`,
    // so when `session_dir()` is `Some(_)` the id is also `Some(_)`
    // (the directory is created with a timestamped name like
    // `2026-06-17T22-21-30`, always valid UTF-8 ASCII). The
    // `?` above short-circuits the disabled-collector case; the
    // `expect` here only triggers on a malformed file system,
    // which the surrounding summary path would then also fail
    // to read.
    let session_dir = event_loop.diagnostics().session_dir()?;
    let _ = session_dir; // SC-5: counts come from IdempotentLog, not legacy journals
    let session_id = event_loop
        .diagnostics()
        .session_id()
        .expect("session_id must be Some when session_dir is Some");

    // Workspace-relative path so the hint survives a worktree
    // checkout. The session directory always lives at
    // `<workspace>/.ralph/diagnostics/<session_id>`, matching
    // [`ralph_core::LoopContext::diagnostics_dir`].
    let session_relpath = Some(format!(".ralph/diagnostics/{session_id}"));
    let diagnose_command = Some("ralph diagnose --session latest".to_string());

    let mut references = Vec::new();
    if let Some(relpath) = payload_violation_report_relpath {
        references.push(ralph_core::DiagnosisReference {
            label: "Payload contract violation report".to_string(),
            relpath: relpath.to_string(),
        });
    }

    let hint = ralph_core::DiagnosisHint {
        session_relpath,
        diagnose_command,
        references,
    };

    let state = event_loop.state();
    let now = chrono::Utc::now();

    // SC-5: derive counts from the loop-scoped idempotent
    // log rather than scanning legacy `recovery.jsonl`. The
    // journal paths below still point at the legacy journals
    // because `ralph diagnose` and operator greps read those
    // files; the COUNT field is now sourced from the
    // authoritative IdempotentLog.
    let (recovery_count, drift_finding_count, _task_count) = collect_idempotent_counts(event_loop);

    let mut notes = Vec::new();
    notes.push(format!(
        "recovery count source: IdempotentLog.final_records() (U8 / SC-5); {} final records on disk",
        recovery_count
    ));
    notes.push(format!(
        "drift count source: IdempotentLog.final_records() (U8 / SC-5); {} final findings on disk",
        drift_finding_count
    ));
    notes.push(
        "recovery journal path: .ralph/diagnostics/<session>/recovery.jsonl (legacy, still readable by `ralph diagnose`)"
            .to_string(),
    );

    let summary = ralph_core::diagnostics::DiagnosisSummary {
        schema_version: ralph_core::diagnostics::DiagnosisSummary::SCHEMA_VERSION,
        session_id: session_id.clone(),
        generated_at: now,
        loop_started_at: None,
        loop_terminated_at: Some(now),
        total_iterations: Some(state.iteration),
        termination_reason: None,
        recovery_journal_path: Some(format!(".ralph/diagnostics/{session_id}/recovery.jsonl")),
        drift_journal_path: Some(format!(".ralph/diagnostics/{session_id}/drift.jsonl")),
        orchestration_log_path: Some(format!(
            ".ralph/diagnostics/{session_id}/orchestration.jsonl"
        )),
        errors_log_path: Some(format!(".ralph/diagnostics/{session_id}/errors.jsonl")),
        recovery_count: recovery_count as u32,
        drift_finding_count: drift_finding_count as u32,
        notes,
    };

    Some((hint, summary))
}

/// U8: write the diagnosis summary seed and append the
/// operator-facing `## Diagnostics` hint to `summary.md`.
///
/// Skipped silently when:
/// - the diagnostics collector has no session directory (i.e.
///   diagnostics disabled for this run), AND
/// - the caller did not provide a payload contract violation
///   reference.
///
/// In that combined case the runner must not invent an empty hint
/// section: it would expose invalid paths and contradict the
/// "no hint when diagnostics are off" contract.
pub(crate) fn write_termination_diagnostics(
    event_loop: &ralph_core::EventLoop,
    summary_writer: &ralph_core::SummaryWriter,
    payload_violation_report_relpath: Option<&str>,
) {
    let Some((hint, summary)) =
        build_termination_diagnostics(event_loop, payload_violation_report_relpath)
    else {
        return;
    };

    if let Err(e) = summary_writer.append_diagnosis_hint(Some(&hint)) {
        tracing::warn!(
            target: "ralph_cli::loop_runner",
            error = %e,
            "Failed to append diagnosis hint section to summary.md"
        );
    }

    event_loop
        .diagnostics()
        .write_diagnosis_summary_seed(&summary);
}

/// U6/U8: post-termination hook that appends a `## Recovery Diagnosis`
/// section to the summary when the recovery responder produced a
/// Final hint, then (U8) writes the operator-facing
/// `## Diagnostics` hint and the `diagnosis-summary.json` seed. The
/// responder hint is taken (one-shot) so the next run does not see
/// a stale signal. Called from each `return Ok(reason)` site in
/// [`run_loop_impl`].
///
/// This is a free function so we can call it from the loop body
/// without threading the hint through the `handle_termination`
/// closure. The hint-taking is intentionally idempotent within a
/// single loop run: once consumed, subsequent `take_termination_hint`
/// calls return `None` until the next `record_finding` writes a new
/// hint.
///
/// `payload_violation_report_relpath` is the workspace-relative
/// path of the root-level `payload-contract-error-*.json` report
/// (U4 / U6 hard gate), or `None` for the normal-termination
/// path. Only the U4 payload contract violation path passes
/// `Some(_)`; every other caller passes `None`. The flag is plumbed
/// through this helper rather than `handle_termination` so the
pub(crate) fn finalize_recovery_diagnosis(
    event_loop: &mut ralph_core::EventLoop,
    ctx: &Option<ralph_core::LoopContext>,
    payload_violation_report_relpath: Option<&str>,
) {
    let summary_writer = if let Some(c) = ctx {
        ralph_core::SummaryWriter::from_context(c)
    } else {
        ralph_core::SummaryWriter::default()
    };

    // U6: drain the responder's hint and append the existing
    // `## Recovery Diagnosis` section. The hint may be `None` on
    // non-final terminations; the section is then skipped, but the
    // U8 step below still runs as long as diagnostics are enabled.
    if let Some(hint) = event_loop.recovery_responder_mut().take_termination_hint()
        && let Err(e) = summary_writer.append_recovery_section(&hint)
    {
        tracing::warn!(
            target: "ralph_cli::loop_runner",
            error = %e,
            "Failed to append recovery diagnosis section to summary.md"
        );
    }

    // U8: append the operator-facing `## Diagnostics` hint and
    // write the `diagnosis-summary.json` seed.
    write_termination_diagnostics(
        event_loop,
        &summary_writer,
        payload_violation_report_relpath,
    );

    // U4: persist active hat activations so `ralph diagnose` can
    // render the `## Active Hat Activations` section.
    let activations = event_loop.hat_lifecycle_tracker().active_activations();
    event_loop
        .diagnostics()
        .write_active_activations(&activations);

    // Plan 2026-08-12-001 D11/D15: append the termination and final
    // feedback rows before taking the artifact snapshot. Otherwise the
    // manifest would report a size/hash from just before those rows and
    // falsely claim the sidecars were finalized.
    if event_loop.diagnostics().session_dir().is_some() {
        event_loop.diagnostics().log_runtime_trace(
            ralph_core::diagnostics::RuntimeTraceEntry::new(
                event_loop.state().iteration as u64,
                0,
                ralph_core::diagnostics::RuntimeTracePhase::Termination,
            )
            .with_status("terminated")
            .with_kind("loop_termination"),
        );
        for finding in event_loop.recovery_responder().pending_findings() {
            let feedback_id = if finding.diagnosis_id.is_empty() {
                finding.retry_key.clone()
            } else {
                finding.diagnosis_id.clone()
            };
            event_loop.diagnostics().log_feedback(
                ralph_core::diagnostics::FeedbackEntry::new(
                    finding.iteration.unwrap_or(event_loop.state().iteration) as u64,
                    feedback_id,
                    finding.retry_key.clone(),
                    ralph_core::diagnostics::FeedbackPhase::Final,
                )
                .with_outcome(format!("{:?}", finding.outcome))
                .with_status("terminated")
                .with_source_ref("loop_runner/termination"),
            );
        }

        let Some(session_dir) = event_loop.diagnostics().session_dir() else {
            unreachable!("session directory was checked above");
        };
        let artifacts = [
            "diagnosis-input.json",
            "runtime-trace.jsonl",
            "feedback.jsonl",
            "recovery.jsonl",
            "drift.jsonl",
            "diagnosis-summary.json",
        ]
        .into_iter()
        .map(|name| diagnostic_artifact_integrity(session_dir, name))
        .collect();
        event_loop
            .diagnostics()
            .finalize_input_bundle(artifacts, execution_capabilities(event_loop.config()));
    }

    // D1 (2026-06-16, plan 002 Unit 5): refresh the session pointer on
    // every termination path so `ralph diagnose --session latest` finds
    // the **final** session after the loop ends. The startup path
    // (run_loop_impl, before handle_termination) writes the pointer once,
    // but if the loop completes or is terminated after writing recovery
    // envelopes, the pointer needs to point at the same session the
    // envelopes live in. Best-effort: a write failure is logged but does
    // not block the loop's normal return. The pointer file path is
    // last-write-wins when concurrent worktrees race; this is documented
    // as the expected behavior in the runtime-diagnosis guide.
    finalize_session_pointer(event_loop.diagnostics(), ctx.as_ref());

    // Suppress the unused-import lint when the function is the only
    // user of `TerminationHint`. The type is re-exported in case the
    // diagnostic report pipeline (U7) wants to introspect the hint
    // structure directly.
    let _ = std::marker::PhantomData::<TerminationHint>;
}
