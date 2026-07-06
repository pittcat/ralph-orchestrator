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
//!    is about to write, the gate consumes the ticket (deletes
//!    the file) and the mutation proceeds. If the ticket is
//!    missing, mismatched, or stale, the gate denies with a
//!    stable prefix and a recovery hint.
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

use crate::operation_guard::OperationContext;
use ralph_core::config::TasksConfig;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Path-relative marker so a denied agent knows where to look.
pub const TICKET_REL_PATH: &str = ".ralph/agent/.ralph-task-verify-ticket";

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

/// Combined read + delete: returns the ticket if it exists, and
/// then removes the file in one call so successful apply burns
/// the ticket atomically (modulo the I/O window between
/// `read_ticket` and `consume_ticket` — small, accepted).
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
///   → `Err(denied: stale or mismatched ticket)`.
/// - Agent + gate on + ticket matches → consume the ticket and
///   return `Ok(())`.
pub fn require_ticket(
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
    let record = match read_and_consume_ticket(path)? {
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
        // The mismatched ticket is consumed (read+consume) so the
        // next apply starts clean. This is the same behavior as
        // the match path.
        assert!(!path.exists(), "consumed on mismatch too");
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
    }
}
