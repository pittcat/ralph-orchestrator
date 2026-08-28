//! Focused helpers used while finalizing loop termination diagnostics.

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

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
