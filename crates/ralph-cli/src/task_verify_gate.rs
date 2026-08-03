//! U7 (2026-07-04-003 plan): two-step verify-then-apply gate for
//! `task add` / `task ensure` mutations invoked by agents.
//!
//! The gate closes the OPAC drift window where an agent could
//! `verify` a payload and then `apply` a *different* payload that
//! happened to also pass the role gate. The contract is:
//!
//! 1. Agent invokes `ralph tools task verify <verb>` → success
//!    path calls `record_ticket(ticket_path, fingerprint, loop, hat)`
//!    which writes a one-shot file at
//!    `<workspace>/.ralph/agent/.ralph-task-verify-ticket`.
//! 2. The same agent (same `loop_id` + `hat_id`) then invokes
//!    `ralph tools task <verb>` → success path calls
//!    `require_ticket` *before* any store mutation. If the
//!    on-disk ticket's fingerprint matches the payload the agent
//!    is about to write, the gate claims the ticket (atomically
//!    renames it to `.ralph-task-verify-ticket.claimed` under an
//!    exclusive `FileLock`) and the mutation proceeds. The
//!    caller MUST then invoke `consume_claimed_ticket` once the
//!    Apply side effect has committed; if Apply fails the caller
//!    MUST invoke `restore_ticket_from_claim` so the next attempt
//!    can re-use the prepared record. If the ticket is missing,
//!    mismatched, or stale, the gate denies with a stable prefix
//!    and a recovery hint, leaving the on-disk ticket
//!    **untouched** so the agent can retry with the correct
//!    payload.
//!
//! The fingerprint is a SHA-256 hex of:
//!   `<verb>\n<canonical_payload>\n<loop_id>\n<hat_id>`
//! so a verify-then-apply with the *same* `AddArgs` produces the
//! same fingerprint; any drift (priority changed, description
//! added, title edited) breaks the match and forces the agent
//! back to `verify`.
//!
//! Human CLI invocations (`is_agent_context == false`) bypass
//! the gate entirely. Operators must not be locked out by a
//! stuck ticket.
//!
//! `consume_ticket` deletes the ticket file as a side effect so
//! the gate is one-shot: a successful apply burns the ticket, a
//! failed apply leaves the ticket in place for the agent to
//! retry with the same payload.
//!
//! U1 (2026-08-03-001-fix-opac-high-confidence-gates-plan): the
//! claim lifecycle is now race-safe. `read_and_consume_ticket`
//! removed the file **before** validating fingerprint/loop/hat,
//! which let a mismatched or wrong-caller consume the prepared
//! record and a concurrent Apply double-mutate. The new flow
//! holds an exclusive `FileLock` across the read, full match,
//! and atomic rename so at most one process can claim a given
//! ticket; mismatches and caller mismatches now leave the
//! prepared record on disk for a correct retry.

use crate::operation_guard::OperationContext;
use ralph_core::config::TasksConfig;
use ralph_core::file_lock::FileLock;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Path-relative marker so a denied agent knows where to look.
pub const TICKET_REL_PATH: &str = ".ralph/agent/.ralph-task-verify-ticket";

/// Suffix used to mark a ticket that has been atomically claimed by
/// the gate but not yet consumed by a successful Apply. The gate
/// renames `<TICKET_REL_PATH>` to `<TICKET_REL_PATH><CLAIM_SUFFIX>`
/// inside an exclusive `FileLock` so a concurrent Apply sees no
/// prepared record on its turn.
pub const CLAIM_SUFFIX: &str = ".claimed";

/// Stable deny prefix (mirrors `hat_command_policy denied` for
/// grep-ability).
pub const DENY_PREFIX: &str = "task_verify_gate denied";

/// Compute a stable fingerprint for a (verb, payload, loop, hat)
/// tuple.
///
/// The hash is intentionally a SHA-256 hex (64 chars) so a human
/// can paste it into a recovery command if the on-disk ticket is
/// corrupted. The canonical payload is opaque to the gate — the
/// caller is expected to JSON-serialize a `serde`-shaped struct
/// so semantically-equivalent payloads hash the same.
pub fn mutation_fingerprint(
    verb: &str,
    canonical_payload: &str,
    loop_id: &str,
    hat_id: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verb.as_bytes());
    hasher.update(b"\n");
    hasher.update(canonical_payload.as_bytes());
    hasher.update(b"\n");
    hasher.update(loop_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(hat_id.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

/// Resolve the ticket file path for a workspace.
pub fn ticket_path(workspace: &Path) -> PathBuf {
    workspace.join(TICKET_REL_PATH)
}

/// Write a one-shot ticket so the next `require_ticket` for the
/// same (loop, hat) will succeed.
///
/// The ticket format is:
/// ```text
/// <sha256-fingerprint> <loop_id> <hat_id> <unix-timestamp-secs>
/// ```
/// One line, no trailing newline. The trailing timestamp lets us
/// reject tickets older than a configurable max age (caller's
/// responsibility; this module does not age out by default).
pub fn record_ticket(
    path: &Path,
    fingerprint: &str,
    loop_id: &str,
    hat_id: &str,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("{fingerprint} {loop_id} {hat_id} {now}\n");
    std::fs::write(path, line)?;
    Ok(())
}

/// Read the on-disk ticket and return its parsed fields.
///
/// Returns `Ok(None)` when the file does not exist (the common
/// "agent never verified" case). Returns `Err` for any I/O
/// failure or malformed line.
pub fn read_ticket(path: &Path) -> anyhow::Result<Option<TicketRecord>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path)?;
    let line = raw.lines().next().unwrap_or("").trim();
    if line.is_empty() {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let fingerprint = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("task_verify_gate: ticket line missing fingerprint"))?
        .to_string();
    let loop_id = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("task_verify_gate: ticket line missing loop_id"))?
        .to_string();
    let hat_id = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("task_verify_gate: ticket line missing hat_id"))?
        .to_string();
    let ts_secs: u64 = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("task_verify_gate: ticket line missing timestamp"))?
        .parse()
        .unwrap_or(0);
    Ok(Some(TicketRecord {
        fingerprint,
        loop_id,
        hat_id,
        timestamp_secs: ts_secs,
    }))
}

/// On-disk ticket payload, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRecord {
    pub fingerprint: String,
    pub loop_id: String,
    pub hat_id: String,
    pub timestamp_secs: u64,
}

/// Consume a ticket by deleting the file. The caller has
/// confirmed the fingerprint matches the pending mutation; the
/// ticket is now burned so a retry needs a fresh `verify`.
pub fn consume_ticket(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}

/// Resolve the on-disk claim marker path. When the gate atomically
/// claims a prepared ticket it renames the prepared file to
/// `<TICKET_REL_PATH><CLAIM_SUFFIX>` so a concurrent caller
/// observes no prepared record on its turn.
///
/// `ticket_path` is the prepared-ticket path (the same value the
/// gate saw). The marker is its sibling: `<dir>/<name><CLAIM_SUFFIX>`.
pub fn claim_marker_path(ticket_path: &Path) -> PathBuf {
    ticket_path.with_file_name(format!(
        "{}{}",
        ticket_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or(TICKET_REL_PATH),
        CLAIM_SUFFIX,
    ))
}

/// Atomically rename a freshly-claimed ticket to the claim marker
/// so a concurrent Apply observes no prepared record. Returns
/// the marker path on success.
fn rename_to_claim_marker(src: &Path) -> anyhow::Result<PathBuf> {
    let marker = claim_marker_path(src);
    std::fs::rename(src, &marker).map_err(|e| {
        anyhow::anyhow!(
            "{DENY_PREFIX}: claim marker rename failed ({} → {}): {}",
            src.display(),
            marker.display(),
            e
        )
    })?;
    Ok(marker)
}

/// Consume a previously-claimed ticket. The caller has finished
/// the Apply side effect (e.g. committed to the task store); the
/// claim marker is now removed so a follow-up Apply requires a
/// fresh verify. Idempotent: deleting a missing marker is Ok.
///
/// `ticket_path` is the prepared-ticket path (the same value the
/// gate saw). The marker path is derived from it so callers do
/// not need to know the workspace root.
pub fn consume_claimed_ticket(ticket_path: &Path) -> anyhow::Result<()> {
    let marker = claim_marker_path(ticket_path);
    if marker.exists() {
        std::fs::remove_file(&marker)?;
    }
    Ok(())
}

/// Restore a previously-claimed ticket after the Apply side
/// effect failed. The claim marker is renamed back to the
/// prepared ticket path so a subsequent Apply (after the agent
/// fixes the cause of the failure) can re-claim without a fresh
/// verify. Returns `Ok(())` if no claim marker exists — that is
/// a no-op (the caller is restoring after a successful Apply
/// which already consumed the marker, or after a gate deny).
///
/// `ticket_path` is the prepared-ticket path. The marker path is
/// derived from it so callers do not need to know the workspace
/// root.
#[allow(dead_code)]
pub fn restore_ticket_from_claim(ticket_path: &Path) -> anyhow::Result<()> {
    let marker = claim_marker_path(ticket_path);
    if !marker.exists() {
        return Ok(());
    }
    // If the prepared slot is already populated (very unusual),
    // surface the conflict instead of silently overwriting.
    if ticket_path.exists() {
        return Err(anyhow::anyhow!(
            "{DENY_PREFIX}: cannot restore claimed ticket — prepared slot already populated at {}",
            ticket_path.display()
        ));
    }
    std::fs::rename(&marker, ticket_path).map_err(|e| {
        anyhow::anyhow!(
            "{DENY_PREFIX}: claim restore rename failed ({} → {}): {}",
            marker.display(),
            ticket_path.display(),
            e
        )
    })?;
    Ok(())
}

/// Combined read + delete: returns the ticket if it exists, and
/// then removes the file in one call so successful apply burns
/// the ticket atomically.
///
/// **U1:** This helper predates the atomic-claim lifecycle. The
/// new flow goes through [`try_claim_matching_ticket`] +
/// [`consume_claimed_ticket`] / [`restore_ticket_from_claim`].
/// `read_and_consume_ticket` remains as a diagnostic helper so
/// non-`apply` tooling can read-and-clear a stale record without
/// going through the lock; the gate itself no longer uses it.
#[allow(dead_code)]
pub fn read_and_consume_ticket(path: &Path) -> anyhow::Result<Option<TicketRecord>> {
    let record = read_ticket(path)?;
    if record.is_some() {
        consume_ticket(path)?;
    }
    Ok(record)
}

/// Decide whether the gate should enforce for this caller.
///
/// Returns `true` when the caller is an agent AND
/// `config.require_verify_for_cli_mutate` is set AND
/// `config.allow_unsafe_task_mutate` is NOT set. Humans always
/// bypass (false). Agents with the unsafe escape hatch bypass
/// (false). Agents with the gate off bypass (false).
pub fn gate_is_active(ctx: &OperationContext, config: &TasksConfig) -> bool {
    if !ctx.is_agent_context {
        return false;
    }
    if !config.require_verify_for_cli_mutate {
        return false;
    }
    if config.allow_unsafe_task_mutate {
        return false;
    }
    true
}

/// The full gate check. Returns `Ok(())` when the mutation may
/// proceed; `Err` with a stable, machine-grepable deny prefix
/// when the gate denies.
///
/// Behavior:
/// - Human CLI (`!ctx.is_agent_context`) → `Ok(())` always.
/// - Agent + config gate off → `Ok(())` always.
/// - Agent + config gate on + `allow_unsafe_task_mutate` →
///   `Ok(())` (escape hatch).
/// - Agent + gate on + no ticket on disk → `Err(denied: missing ticket)`.
/// - Agent + gate on + ticket with wrong (loop|hat|fingerprint)
///   → `Err(denied: stale or mismatched ticket)`; the prepared
///   record is **left on disk** so a corrected Apply can
///   re-claim without a fresh verify.
/// - Agent + gate on + ticket matches → atomically rename the
///   ticket to the claim marker under an exclusive `FileLock`
///   (concurrent Apply observes no prepared record) and return
///   `Ok(())`. The caller MUST invoke
///   [`consume_claimed_ticket`] on successful Apply or
///   [`restore_ticket_from_claim`] on Apply failure.
pub fn require_ticket(
    path: &Path,
    config: &TasksConfig,
    ctx: &OperationContext,
    verb: &str,
    fingerprint: &str,
) -> anyhow::Result<()> {
    try_claim_matching_ticket(path, config, ctx, verb, fingerprint)?;
    // Match: ticket has been atomically renamed to the claim
    // marker. Burn it now so the existing one-shot contract holds
    // for callers that don't drive the new restore/restoration
    // helpers directly.
    consume_claimed_ticket(path)
}

/// U1 (2026-08-03-001-fix-opac-high-confidence-gates-plan):
/// race-safe variant of the gate check.
///
/// Holds an exclusive `FileLock` across the read, the full
/// fingerprint/loop/hat validation, and an atomic
/// `std::fs::rename` of the prepared ticket to its claim marker.
/// Returns `Ok(())` after a successful claim; on mismatch,
/// missing record, or format error, returns `Err(...)` and the
/// prepared ticket is **left untouched** on disk so a corrected
/// Apply can re-claim without re-running `verify`.
///
/// Callers that want the legacy "consume on accept" behavior
/// should keep using [`require_ticket`]. Callers that need to
/// defer consume until after the Apply side effect commits
/// should call `try_claim_matching_ticket` and then either
/// [`consume_claimed_ticket`] (success) or
/// [`restore_ticket_from_claim`] (failure).
pub fn try_claim_matching_ticket(
    path: &Path,
    config: &TasksConfig,
    ctx: &OperationContext,
    verb: &str,
    fingerprint: &str,
) -> anyhow::Result<()> {
    if !gate_is_active(ctx, config) {
        return Ok(());
    }
    let loop_id = ctx.current_loop_id.as_deref().unwrap_or("");
    let hat_id = ctx.current_hat_id.as_deref().unwrap_or("");

    // Hold the lock across read+validate+rename. Two concurrent
    // Apply processes serialize here; the first to acquire the
    // lock finds the prepared ticket, validates it, and renames
    // it to the claim marker. The second acquires the lock after
    // the first releases it and finds no prepared record.
    let _lock_guard = FileLock::new(path).and_then(|l| l.exclusive()).map_err(|e| {
        anyhow::anyhow!(
            "{DENY_PREFIX} '{verb}': failed to acquire gate lock: {err}",
            verb = verb,
            err = e,
        )
    })?;

    let record = match read_ticket(path)? {
        Some(r) => r,
        None => {
            return Err(anyhow::anyhow!(
                "{DENY_PREFIX} '{verb}': no verify ticket at {} — \
                 run `ralph tools task verify {verb} <args...>` first to record a matching ticket, \
                 then re-invoke the real mutation with the same payload. \
                 Hat: '{hat_id}' Loop: '{loop_id}'.",
                path.display()
            ));
        }
    };
    if record.fingerprint != fingerprint {
        // Do NOT remove the prepared record. The agent's
        // mismatch payload can be fixed and re-applied against the
        // same ticket; burning it on mismatch would force a fresh
        // verify with no security benefit.
        return Err(anyhow::anyhow!(
            "{DENY_PREFIX} '{verb}': ticket fingerprint mismatch (on-disk={on_disk} pending={pending}) — \
             the payload you are about to write differs from the one you verified. \
             Re-run `ralph tools task verify {verb}` with the *current* args and try again. \
             Hat: '{hat_id}' Loop: '{loop_id}'.",
            on_disk = record.fingerprint,
            pending = fingerprint
        ));
    }
    if record.loop_id != loop_id || record.hat_id != hat_id {
        return Err(anyhow::anyhow!(
            "{DENY_PREFIX} '{verb}': ticket (loop, hat) = ({rec_loop}, {rec_hat}) \
             but caller is ({loop_id}, {hat_id}) — tickets are bound to the verifying hat. \
             Re-run verify from this hat. \
             Hat: '{hat_id}' Loop: '{loop_id}'.",
            rec_loop = record.loop_id,
            rec_hat = record.hat_id
        ));
    }
    // Atomically rename the prepared record to the claim marker
    // so a concurrent caller observes no prepared file on its
    // turn. rename(2) is atomic on POSIX for same-filesystem
    // moves; the FileLock above provides the same-filesystem
    // serialization and the cross-process witness.
    rename_to_claim_marker(path)?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod task_verify_gate_tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_workspace() -> TempDir {
        TempDir::new().expect("temp dir")
    }

    fn make_ctx(loop_id: &str, hat_id: &str, is_agent: bool) -> OperationContext {
        OperationContext {
            workspace_root: PathBuf::from("/tmp"),
            current_loop_id: Some(loop_id.to_string()),
            current_hat_id: Some(hat_id.to_string()),
            is_agent_context: is_agent,
        }
    }

    fn default_config(gate_on: bool) -> TasksConfig {
        TasksConfig {
            enabled: true,
            coordinator_hats: Vec::new(),
            require_verify_for_cli_mutate: gate_on,
            allow_unsafe_task_mutate: false,
        }
    }

    #[test]
    fn test_fingerprint_stable_for_same_payload() {
        let a = mutation_fingerprint("add", r#"{"title":"t"}"#, "loop-1", "executor");
        let b = mutation_fingerprint("add", r#"{"title":"t"}"#, "loop-1", "executor");
        assert_eq!(a, b, "same inputs must hash equal");
        assert_eq!(a.len(), 64, "SHA-256 hex must be 64 chars");
    }

    #[test]
    fn test_fingerprint_differs_for_different_title() {
        let a = mutation_fingerprint("add", r#"{"title":"t1"}"#, "loop-1", "executor");
        let b = mutation_fingerprint("add", r#"{"title":"t2"}"#, "loop-1", "executor");
        assert_ne!(a, b, "different payloads must hash different");
    }

    #[test]
    fn test_record_then_consume_ok() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        record_ticket(&path, "fp-1", "loop-1", "executor").expect("record");
        let record = read_ticket(&path).expect("read").expect("present");
        assert_eq!(record.fingerprint, "fp-1");
        assert_eq!(record.loop_id, "loop-1");
        assert_eq!(record.hat_id, "executor");
        consume_ticket(&path).expect("consume");
        assert!(!path.exists(), "consume must delete the file");
    }

    #[test]
    fn test_consume_without_record_err_on_read() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        // No record written — read returns Ok(None).
        let record = read_ticket(&path).expect("read missing");
        assert!(record.is_none(), "missing file must yield None");
        // consume on a missing file is a no-op (Ok).
        consume_ticket(&path).expect("consume missing is ok");
    }

    #[test]
    fn test_consume_twice_second_is_noop() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        record_ticket(&path, "fp", "l", "h").expect("record");
        consume_ticket(&path).expect("first consume");
        // Second consume: file gone, should be Ok.
        consume_ticket(&path).expect("second consume is noop");
    }

    #[test]
    fn test_require_ticket_agent_no_record_denied() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", true);
        let cfg = default_config(true);
        let fp = mutation_fingerprint("add", "{}", "loop-1", "executor");
        let err =
            require_ticket(&path, &cfg, &ctx, "add", &fp).expect_err("missing ticket must deny");
        let msg = err.to_string();
        assert!(
            msg.starts_with(DENY_PREFIX),
            "must carry stable prefix: {msg}"
        );
        assert!(
            msg.contains("no verify ticket"),
            "must explain root cause: {msg}"
        );
        assert!(
            msg.contains("ralph tools task verify add"),
            "must include recovery: {msg}"
        );
        // The file must not be created by the failed gate.
        assert!(!path.exists(), "deny must not create a ticket");
    }

    #[test]
    fn test_require_ticket_human_bypass() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", false);
        let cfg = default_config(true);
        let fp = mutation_fingerprint("add", "{}", "loop-1", "executor");
        // Human CLI with no ticket on disk → still Ok.
        require_ticket(&path, &cfg, &ctx, "add", &fp).expect("human must bypass");
    }

    #[test]
    fn test_require_ticket_agent_or_config_strict() {
        // config gate OFF + agent ctx → still Ok without ticket.
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", true);
        let cfg = default_config(false);
        let fp = mutation_fingerprint("add", "{}", "loop-1", "executor");
        require_ticket(&path, &cfg, &ctx, "add", &fp).expect("gate off must bypass for agent");
    }

    #[test]
    fn test_require_ticket_unsafe_escape_hatch_bypasses() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", true);
        let cfg = TasksConfig {
            enabled: true,
            coordinator_hats: Vec::new(),
            require_verify_for_cli_mutate: true,
            allow_unsafe_task_mutate: true,
        };
        let fp = mutation_fingerprint("add", "{}", "loop-1", "executor");
        // unsafe escape hatch: agent with no ticket is allowed.
        require_ticket(&path, &cfg, &ctx, "add", &fp).expect("unsafe escape must bypass");
    }

    #[test]
    fn test_require_ticket_match_consumes_and_allows() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", true);
        let cfg = default_config(true);
        let fp = mutation_fingerprint("add", r#"{"title":"t"}"#, "loop-1", "executor");
        record_ticket(&path, &fp, "loop-1", "executor").expect("record");
        require_ticket(&path, &cfg, &ctx, "add", &fp).expect("matching ticket must allow");
        // Ticket is consumed.
        assert!(!path.exists(), "matching apply must consume the ticket");
    }

    #[test]
    fn test_require_ticket_fingerprint_mismatch_denied() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", true);
        let cfg = default_config(true);
        let on_disk_fp = mutation_fingerprint("add", r#"{"title":"t1"}"#, "loop-1", "executor");
        let pending_fp = mutation_fingerprint("add", r#"{"title":"t2"}"#, "loop-1", "executor");
        record_ticket(&path, &on_disk_fp, "loop-1", "executor").expect("record");
        let err =
            require_ticket(&path, &cfg, &ctx, "add", &pending_fp).expect_err("mismatch must deny");
        let msg = err.to_string();
        assert!(msg.contains("fingerprint mismatch"), "must explain: {msg}");
        // U1: mismatch preserves the prepared record. A corrected
        // Apply against the same ticket must succeed without a
        // fresh verify.
        assert!(
            path.exists(),
            "mismatch must leave prepared record on disk for retry"
        );
    }

    #[test]
    fn test_require_ticket_loop_hat_mismatch_denied() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "worker", true);
        let cfg = default_config(true);
        let fp = mutation_fingerprint("add", "{}", "loop-1", "executor");
        record_ticket(&path, &fp, "loop-1", "executor").expect("record");
        let err =
            require_ticket(&path, &cfg, &ctx, "add", &fp).expect_err("hat mismatch must deny");
        let msg = err.to_string();
        assert!(
            msg.contains("ticket (loop, hat)"),
            "must explain hat binding: {msg}"
        );
        // U1: caller mismatch preserves the prepared record.
        assert!(
            path.exists(),
            "caller mismatch must leave prepared record on disk"
        );
    }

    /// U1: two concurrent Apply with the same fingerprint must
    /// produce exactly one winner. The loser receives
    /// `task_verify_gate denied` and the task store observes at
    /// most one Apply (verified via external witness below).
    #[test]
    fn test_concurrent_apply_exactly_one_winner() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", true);
        let cfg = default_config(true);
        let fp = mutation_fingerprint("add", r#"{"title":"t"}"#, "loop-1", "executor");
        record_ticket(&path, &fp, "loop-1", "executor").expect("record");

        let barrier = Arc::new(Barrier::new(2));
        let path_a = path.clone();
        let path_b = path.clone();
        let cfg_a = cfg.clone();
        let cfg_b = cfg.clone();
        let ctx_a = ctx.clone();
        let ctx_b = ctx.clone();
        let barrier_a = barrier.clone();
        let barrier_b = barrier.clone();

        let fp_a = fp.clone();
        let fp_b = fp.clone();
        let handle_a = thread::spawn(move || {
            barrier_a.wait();
            require_ticket(&path_a, &cfg_a, &ctx_a, "add", &fp_a)
        });
        let handle_b = thread::spawn(move || {
            barrier_b.wait();
            require_ticket(&path_b, &cfg_b, &ctx_b, "add", &fp_b)
        });
        let result_a = handle_a.join().expect("thread a");
        let result_b = handle_b.join().expect("thread b");

        let oks = [&result_a, &result_b]
            .iter()
            .filter(|r| r.is_ok())
            .count();
        let denials = [&result_a, &result_b]
            .iter()
            .filter(|r| {
                r.as_ref()
                    .err()
                    .map(|e| e.to_string().contains(DENY_PREFIX))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(oks, 1, "exactly one Apply must win: a={:?} b={:?}", result_a, result_b);
        assert_eq!(denials, 1, "exactly one Apply must be denied with stable prefix");
    }

    /// U1: when the explicit two-step claim lifecycle is used
    /// (claim → Apply → consume OR restore), an Apply failure
    /// leaves the prepared record available for retry, and a
    /// successful Apply burns the record.
    #[test]
    fn test_apply_failure_restores_prepared_ticket() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", true);
        let cfg = default_config(true);
        let fp = mutation_fingerprint("add", r#"{"title":"t"}"#, "loop-1", "executor");
        record_ticket(&path, &fp, "loop-1", "executor").expect("record");

        // 1. Claim — ticket moves to the claim marker.
        try_claim_matching_ticket(&path, &cfg, &ctx, "add", &fp).expect("claim");
        assert!(!path.exists(), "claim must rename prepared to marker");
        let marker = path.with_file_name(format!(
            "{}{}",
            path.file_name().and_then(|s| s.to_str()).unwrap(),
            CLAIM_SUFFIX
        ));
        assert!(marker.exists(), "claim marker must be present");

        // 2. Apply fails — caller invokes restore.
        restore_ticket_from_claim(&path).expect("restore");
        assert!(path.exists(), "restore must put prepared record back");
        assert!(!marker.exists(), "restore must remove the claim marker");
        let record = read_ticket(&path).expect("read").expect("present");
        assert_eq!(record.fingerprint, fp, "restored record must be intact");

        // 3. A corrected Apply can re-claim the restored ticket
        //    without a fresh verify.
        try_claim_matching_ticket(&path, &cfg, &ctx, "add", &fp).expect("re-claim");
        consume_claimed_ticket(&path).expect("consume after successful apply");
        assert!(!path.exists(), "consume must remove the prepared record");
        assert!(
            !marker.exists(),
            "consume must remove the claim marker"
        );
    }

    /// U1: legacy `require_ticket` continues to consume on
    /// success so existing callers (which don't drive the
    /// restore/restoration helpers directly) keep their
    /// one-shot contract.
    #[test]
    fn test_legacy_require_ticket_consumes_on_match() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", true);
        let cfg = default_config(true);
        let fp = mutation_fingerprint("add", r#"{"title":"t"}"#, "loop-1", "executor");
        record_ticket(&path, &fp, "loop-1", "executor").expect("record");
        require_ticket(&path, &cfg, &ctx, "add", &fp).expect("match must allow");
        assert!(
            !path.exists(),
            "legacy require_ticket must consume the prepared record on match"
        );
        let marker = path.with_file_name(format!(
            "{}{}",
            path.file_name().and_then(|s| s.to_str()).unwrap(),
            CLAIM_SUFFIX
        ));
        assert!(
            !marker.exists(),
            "legacy require_ticket must also clean up the claim marker"
        );
    }
}
