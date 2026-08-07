//! D5: startup-timeout helpers for `agent_doc_sync::sync_all`.
//!
//! `sync_all` runs blocking I/O before backend spawn. A stuck lock,
//! slow disk, or NFS round-trip can otherwise hang the outer loop.
//! We run the sync on a worker thread and `recv_timeout` it.
//!
//! Also exposes `adapter_timeout_duration` (a tiny adapter-timeout
//! helper shared between the runner entry and inner impl).

use std::path::Path;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

pub(crate) fn adapter_timeout_duration(timeout_secs: u64) -> Option<Duration> {
    (timeout_secs > 0).then(|| Duration::from_secs(timeout_secs))
}

/// Outcome of running `sync_all` with a startup timeout.
#[derive(Debug)]
pub(super) enum SyncRunError {
    /// `sync_all` returned an error (lock contention, I/O, etc.).
    Sync(ralph_core::agent_doc_sync::SyncError),
    /// The sync did not finish within the configured timeout.
    Timeout { secs: u64 },
}

/// Run `sync_all` on a worker thread and bound it with `timeout_secs`.
///
/// `timeout_secs == 0` disables the timeout (legacy behaviour): the
/// call blocks on the worker thread indefinitely.
///
/// The worker thread is intentionally **not** joined on timeout —
/// the thread will eventually finish (or stay parked on a held file
/// lock) and exit; leaking it is preferable to blocking the loop.
pub(super) fn run_sync_with_timeout(
    workspace_root: &Path,
    sync_config: &ralph_core::agent_doc_sync::SyncConfig<'_>,
    timeout_secs: u64,
) -> Result<ralph_core::agent_doc_sync::SyncReport, SyncRunError> {
    use std::path::PathBuf;

    if timeout_secs == 0 {
        // No timeout: run inline so we surface real errors.
        return ralph_core::agent_doc_sync::sync_all(workspace_root, sync_config)
            .map_err(SyncRunError::Sync);
    }

    let (tx, rx) = mpsc::channel::<
        Result<ralph_core::agent_doc_sync::SyncReport, ralph_core::agent_doc_sync::SyncError>,
    >();
    let root: PathBuf = workspace_root.to_path_buf();

    // Reconstruct a short-lived `SyncConfig` whose lifetimes are tied
    // to the worker thread. `target_files` is a `&'static` slice of
    // string literals; `blocks_vec` is an owned `Vec` moved into the
    // closure.
    let target_files: &'static [&'static str] = &["CLAUDE.md", "AGENTS.md"];
    let blocks_vec: Vec<ralph_core::agent_doc_sync::BlockSpec> = sync_config.blocks.to_vec();
    let on_error = sync_config.on_error;
    let session_dir_owned: Option<PathBuf> = sync_config.session_dir.map(|p| p.to_path_buf());

    let handle = thread::Builder::new()
        .name("ralph-agent-doc-sync".to_string())
        .spawn(move || {
            let cfg = ralph_core::agent_doc_sync::SyncConfig {
                skip: false,
                on_error,
                target_files,
                blocks: &blocks_vec,
                session_dir: session_dir_owned.as_deref(),
            };
            let result = ralph_core::agent_doc_sync::sync_all(&root, &cfg);
            // Ignore send failure: receiver may have timed out.
            let _ = tx.send(result);
        })
        .expect("failed to spawn agent_doc_sync worker thread");

    match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
        Ok(Ok(report)) => Ok(report),
        Ok(Err(e)) => Err(SyncRunError::Sync(e)),
        Err(mpsc::RecvTimeoutError::Timeout) => {
            tracing::warn!(
                target: "ralph_cli::loop_runner",
                timeout_secs,
                "agent_doc_sync: worker thread did not return in time; detaching"
            );
            // Detach: the thread will eventually finish (or hang on a
            // held lock); joining it would defeat the timeout.
            let _ = handle;
            Err(SyncRunError::Timeout { secs: timeout_secs })
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            // Worker panicked before sending. Treat as a sync error
            // so callers can decide (Strict → exit, Warn → continue).
            Err(SyncRunError::Sync(
                ralph_core::agent_doc_sync::SyncError::VerifyFailed {
                    path: String::from("<agent_doc_sync>"),
                    detail: format!(
                        "worker thread disconnected before sending (likely panicked) within {timeout_secs}s"
                    ),
                },
            ))
        }
    }
}

/// Append a `startup_timeout` recovery envelope so operators can see
/// the timeout in `ralph diagnose --source agent_doc_sync`. When
/// `session_dir` is `None`, this is a no-op (sync ran without
/// diagnostics enabled).
pub(super) fn write_startup_timeout_envelope(
    session_dir: Option<&Path>,
    timeout_secs: u64,
    on_error: ralph_core::OnErrorPolicy,
) {
    use ralph_core::diagnosis::{
        DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope,
        RecoveryJournalEntry,
    };
    let Some(session_dir) = session_dir else {
        return;
    };
    let severity = match on_error {
        ralph_core::OnErrorPolicy::Strict => DiagnosisSeverity::Error,
        ralph_core::OnErrorPolicy::Warn => DiagnosisSeverity::Warning,
    };
    let envelope = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::AgentDocSync)
        .severity(severity)
        .iteration(0)
        .reason_code("startup_timeout")
        .message(format!(
            "agent_doc_sync exceeded {timeout_secs}s startup timeout"
        ))
        .outcome(DiagnosisOutcome::Escalated)
        .build();
    let entry = RecoveryJournalEntry::from_envelope(envelope, vec![]);
    // Best-effort: a write failure here must not crash the loop.
    if let Ok(line) = serde_json::to_string(&entry) {
        let path = session_dir.join("recovery.jsonl");
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            use std::io::Write;
            let _ = writeln!(f, "{line}");
        }
    }
}
