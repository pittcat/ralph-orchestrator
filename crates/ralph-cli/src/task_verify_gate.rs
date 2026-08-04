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
//!    `<workspace>/.ralph/agent/task-tickets/<verb>__<loop>__<hat>__<intent>.ticket`
//!    (per-operation scoped namespace; U2).
//! 2. The same agent (same `loop_id` + `hat_id`) then invokes
//!    `ralph tools task <verb>` → success path calls
//!    `try_claim_matching_ticket` *before* any store mutation. If
//!    the on-disk ticket's fingerprint matches the payload the
//!    agent is about to write, the gate claims the ticket
//!    (atomically renames it to a sibling `<ticket>.claimed`
//!    marker under an exclusive
//!    `FileLock`) and the mutation proceeds. The caller MUST then
//!    settle the claim once the Apply side effect finishes:
//!    `consume_claimed_ticket` after a successful Apply;
//!    `restore_ticket_from_claim` after a failed Apply so the next
//!    attempt can re-use the prepared record. Burning the ticket
//!    before the mutation commits is forbidden (U1). If the ticket
//!    is missing, mismatched, or stale, the gate denies with a
//!    stable prefix and a recovery hint, leaving the on-disk
//!    ticket **untouched** so the agent can retry with the correct
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
use ralph_core::{ConfirmationState, TaskStore};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Path-relative marker so a denied agent knows where to look.
pub const TICKET_REL_PATH: &str = ".ralph/agent/.ralph-task-verify-ticket";

/// U2 (2026-08-03-001-fix-opac-high-confidence-gates-plan):
/// per-operation-scoped ticket directory. Distinct
/// operation/intent/activation tuples never share a file here.
pub const TICKET_NAMESPACE_DIR: &str = ".ralph/agent/task-tickets";

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
///
/// **U2 (2026-08-03-001-fix-opac-high-confidence-gates-plan):**
/// this helper now resolves a per-operation-scoped ticket path
/// derived from `(verb, canonical_payload, loop_id, hat_id)`.
/// Distinct operation/intent/activation tuples never share a
/// ticket file, so a subsequent verify can never overwrite an
/// unrelated pending operation.
///
/// Legacy behavior (STAB-OPAC-GATES-002, documented to match the
/// code): the old fixed path
/// (`.ralph/agent/.ralph-task-verify-ticket`) is never consulted
/// by the gate — Apply only reads scoped paths. A legacy
/// plaintext ticket left at the old path is therefore ignored,
/// never accepted as authority, and never cleaned up; an Apply
/// attempt is denied with the stable `no verify ticket` re-verify
/// hint. Pinned by the integration test
/// `test_legacy_plaintext_ticket_is_not_trusted`.
#[allow(dead_code)] // legacy back-compat path; real callers use scoped_ticket_path.
pub fn ticket_path(workspace: &Path) -> PathBuf {
    scoped_ticket_path(workspace, "", "", "", "")
}

/// Resolve the per-operation-scoped ticket path for a single
/// `(verb, canonical_payload, loop, hat)` tuple. The legacy
/// fixed-path ticket is replaced by a namespace of
/// `.ralph/agent/task-tickets/<verb>__<loop>__<hat>__<intent>.ticket`
/// so that `verify add` and `verify ensure`, different
/// loop/hat activations, and distinct intent digests never
/// collide on the same file.
pub fn scoped_ticket_path(
    workspace: &Path,
    verb: &str,
    canonical_payload: &str,
    loop_id: &str,
    hat_id: &str,
) -> PathBuf {
    if verb.is_empty()
        && canonical_payload.is_empty()
        && loop_id.is_empty()
        && hat_id.is_empty()
    {
        // Back-compat caller (no scope). Return the legacy path
        // so existing wiring (`record_ticket` without a scope)
        // continues to compile. The runtime check below in
        // `read_ticket` rejects the legacy plaintext shape.
        return workspace.join(TICKET_REL_PATH);
    }
    let intent = short_intent_digest(verb, canonical_payload, loop_id, hat_id);
    workspace.join(TICKET_NAMESPACE_DIR).join(format!(
        "{}__{}__{}__{}.ticket",
        safe_segment(verb),
        safe_segment(loop_id),
        safe_segment(hat_id),
        intent
    ))
}

/// 16-hex prefix of the full fingerprint. The full 64-hex SHA-256
/// would exceed most filesystem path limits (e.g. ext4 = 255 bytes)
/// when combined with verb/loop/hat segments; 16 hex chars give
/// 64 bits of collision space which is plenty for per-workspace
/// per-operation ticket disambiguation while keeping the file name
/// short.
fn short_intent_digest(verb: &str, canonical_payload: &str, loop_id: &str, hat_id: &str) -> String {
    let full = mutation_fingerprint(verb, canonical_payload, loop_id, hat_id);
    full.chars().take(16).collect()
}

/// Replace filesystem-unsafe characters in a path segment with
/// `_`. The segments are bounded to 64 chars so the total ticket
/// file name stays within filesystem limits even with hostile
/// loop/hat identifiers (e.g. long paths, colons, slashes).
fn safe_segment(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .take(64)
        .collect();
    if cleaned.is_empty() {
        "anon".to_string()
    } else {
        cleaned
    }
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

/// Unit 1 (task confirmation): deny a protected mutation while the
/// caller's scope still carries a pending confirmation.
///
/// Runs before the ticket claim so the prepared record survives the
/// denial untouched; once `task confirm` consumes the pending record,
/// the same ticket can be re-claimed without a fresh `verify`. The
/// three gate bypasses (human CLI, gate off, unsafe hatch) skip the
/// precheck entirely, matching [`gate_is_active`].
pub fn pending_confirmation_precheck(
    store: &TaskStore,
    config: &TasksConfig,
    ctx: &OperationContext,
    verb: &str,
) -> anyhow::Result<()> {
    if !gate_is_active(ctx, config) {
        return Ok(());
    }
    let loop_id = ctx.current_loop_id.as_deref().unwrap_or("");
    let hat_id = ctx.current_hat_id.as_deref().unwrap_or("");
    for task in store.all() {
        if let Some(cfm) = task.confirmation.as_ref()
            && cfm.state == ConfirmationState::Pending
            && cfm.loop_id == loop_id
            && cfm.hat_id == hat_id
        {
            anyhow::bail!(
                "{DENY_PREFIX} '{verb}': confirmation_required — task '{task_id}' \
                 (loop '{loop_id}', hat '{hat_id}') still carries a pending confirmation \
                 (reference '{reference}'). Consume it first with \
                 `ralph tools task confirm {task_id} --reference {reference} --digest <digest>` \
                 from the same loop/hat (the digest is the confirmation.digest field printed by \
                 the Apply that recorded it; if that Apply output is no longer in the current \
                 context, run `ralph tools task show {task_id} --format json` and read \
                 `confirmation.reference` / `confirmation.digest`), then retry this mutation. \
                 The prepared verify ticket is preserved, so the same payload does not need a \
                 fresh `ralph tools task verify {verb}`.",
                task_id = task.id,
                reference = cfm.reference,
            );
        }
    }
    Ok(())
}

/// U1 (2026-08-03-001-fix-opac-high-confidence-gates-plan):
/// race-safe gate claim — the single gate entry point.
///
/// Holds an exclusive `FileLock` across the read, the full
/// fingerprint/loop/hat validation, and an atomic
/// `std::fs::rename` of the prepared ticket to its claim marker.
/// Returns `Ok(())` after a successful claim; on mismatch,
/// missing record, or format error, returns `Err(...)` and the
/// prepared ticket is **left untouched** on disk so a corrected
/// Apply can re-claim without re-running `verify`.
///
/// U1 contract (plan §1 "目标行为"): only a successful Apply
/// consumes the ticket. Callers MUST settle the claim after the
/// Apply side effect: [`consume_claimed_ticket`] on success or
/// [`restore_ticket_from_claim`] on failure, so a failed Apply
/// leaves the prepared record available for retry. Burning the
/// ticket before the mutation commits is forbidden.
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
    fn test_claim_agent_no_record_denied() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", true);
        let cfg = default_config(true);
        let fp = mutation_fingerprint("add", "{}", "loop-1", "executor");
        let err = try_claim_matching_ticket(&path, &cfg, &ctx, "add", &fp)
            .expect_err("missing ticket must deny");
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
    fn test_claim_human_bypass() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", false);
        let cfg = default_config(true);
        let fp = mutation_fingerprint("add", "{}", "loop-1", "executor");
        // Human CLI with no ticket on disk → still Ok.
        try_claim_matching_ticket(&path, &cfg, &ctx, "add", &fp).expect("human must bypass");
    }

    #[test]
    fn test_claim_gate_off_bypasses() {
        // config gate OFF + agent ctx → still Ok without ticket.
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", true);
        let cfg = default_config(false);
        let fp = mutation_fingerprint("add", "{}", "loop-1", "executor");
        try_claim_matching_ticket(&path, &cfg, &ctx, "add", &fp)
            .expect("gate off must bypass for agent");
    }

    #[test]
    fn test_claim_unsafe_escape_hatch_bypasses() {
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
        try_claim_matching_ticket(&path, &cfg, &ctx, "add", &fp)
            .expect("unsafe escape must bypass");
    }

    #[test]
    fn test_claim_match_claims_marker_and_settle_consumes() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", true);
        let cfg = default_config(true);
        let fp = mutation_fingerprint("add", r#"{"title":"t"}"#, "loop-1", "executor");
        record_ticket(&path, &fp, "loop-1", "executor").expect("record");
        try_claim_matching_ticket(&path, &cfg, &ctx, "add", &fp)
            .expect("matching ticket must claim");
        // Claim moved the prepared record to the marker; nothing
        // is burned until the Apply side effect settles.
        assert!(!path.exists(), "claim must move the prepared record");
        let marker = claim_marker_path(&path);
        assert!(marker.exists(), "claim marker must be present");
        consume_claimed_ticket(&path).expect("settle after successful apply");
        assert!(!marker.exists(), "consume must remove the claim marker");
    }

    #[test]
    fn test_claim_fingerprint_mismatch_denied() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", true);
        let cfg = default_config(true);
        let on_disk_fp = mutation_fingerprint("add", r#"{"title":"t1"}"#, "loop-1", "executor");
        let pending_fp = mutation_fingerprint("add", r#"{"title":"t2"}"#, "loop-1", "executor");
        record_ticket(&path, &on_disk_fp, "loop-1", "executor").expect("record");
        let err = try_claim_matching_ticket(&path, &cfg, &ctx, "add", &pending_fp)
            .expect_err("mismatch must deny");
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
    fn test_claim_loop_hat_mismatch_denied() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "worker", true);
        let cfg = default_config(true);
        let fp = mutation_fingerprint("add", "{}", "loop-1", "executor");
        record_ticket(&path, &fp, "loop-1", "executor").expect("record");
        let err = try_claim_matching_ticket(&path, &cfg, &ctx, "add", &fp)
            .expect_err("hat mismatch must deny");
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
            try_claim_matching_ticket(&path_a, &cfg_a, &ctx_a, "add", &fp_a)
        });
        let handle_b = thread::spawn(move || {
            barrier_b.wait();
            try_claim_matching_ticket(&path_b, &cfg_b, &ctx_b, "add", &fp_b)
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

    // ─────────────────────────────────────────────────────────────────
    // U2 (2026-08-03-001-fix-opac-high-confidence-gates-plan):
    // per-operation/intent/activation ticket namespace.
    // ─────────────────────────────────────────────────────────────────

    fn scoped_for(
        ws: &tempfile::TempDir,
        verb: &str,
        payload: &str,
        loop_id: &str,
        hat: &str,
    ) -> PathBuf {
        scoped_ticket_path(ws.path(), verb, payload, loop_id, hat)
    }

    /// U2: `verify add` and `verify ensure` for distinct intents
    /// must live in different files and both remain applicable.
    #[test]
    fn test_u2_add_and_ensure_tickets_coexist() {
        let ws = temp_workspace();
        let ctx = make_ctx("loop-1", "executor", true);
        let cfg = default_config(true);

        let add_fp = mutation_fingerprint("add", r#"{"title":"a"}"#, "loop-1", "executor");
        let ensure_fp =
            mutation_fingerprint("ensure", r#"{"title":"b","key":"k"}"#, "loop-1", "executor");
        let add_path = scoped_for(&ws, "add", r#"{"title":"a"}"#, "loop-1", "executor");
        let ensure_path =
            scoped_for(&ws, "ensure", r#"{"title":"b","key":"k"}"#, "loop-1", "executor");

        record_ticket(&add_path, &add_fp, "loop-1", "executor").expect("record add");
        record_ticket(&ensure_path, &ensure_fp, "loop-1", "executor")
            .expect("record ensure");

        // Both records exist independently.
        assert!(add_path.exists(), "add ticket must be on disk");
        assert!(ensure_path.exists(), "ensure ticket must be on disk");

        // Each gate claim matches only its own file; settle
        // consumes the claimed ticket after the Apply succeeds.
        try_claim_matching_ticket(&add_path, &cfg, &ctx, "add", &add_fp).expect("add apply claims");
        consume_claimed_ticket(&add_path).expect("add apply settles");
        assert!(!add_path.exists(), "add apply consumes its own ticket");
        assert!(
            ensure_path.exists(),
            "add apply must not touch the ensure ticket"
        );

        try_claim_matching_ticket(&ensure_path, &cfg, &ctx, "ensure", &ensure_fp)
            .expect("ensure apply claims");
        consume_claimed_ticket(&ensure_path).expect("ensure apply settles");
        assert!(
            !ensure_path.exists(),
            "ensure apply consumes its own ticket"
        );
    }

    /// U2: different loop/hat activations cannot consume each
    /// other's tickets.
    #[test]
    fn test_u2_different_activation_tickets_isolated() {
        let ws = temp_workspace();
        let cfg = default_config(true);

        let payload = r#"{"title":"t"}"#;
        let fp_loop_a =
            mutation_fingerprint("add", payload, "loop-a", "executor");
        let fp_loop_b =
            mutation_fingerprint("add", payload, "loop-b", "executor");
        let path_loop_a = scoped_for(&ws, "add", payload, "loop-a", "executor");
        let path_loop_b = scoped_for(&ws, "add", payload, "loop-b", "executor");

        record_ticket(&path_loop_a, &fp_loop_a, "loop-a", "executor").expect("record a");
        record_ticket(&path_loop_b, &fp_loop_b, "loop-b", "executor").expect("record b");

        let ctx_a = make_ctx("loop-a", "executor", true);
        try_claim_matching_ticket(&path_loop_a, &cfg, &ctx_a, "add", &fp_loop_a)
            .expect("loop-a apply must claim");
        consume_claimed_ticket(&path_loop_a).expect("loop-a apply settles");

        // loop-b's ticket must survive loop-a's apply.
        assert!(
            path_loop_b.exists(),
            "different-loop ticket must not be consumed by loop-a"
        );

        let ctx_b = make_ctx("loop-b", "executor", true);
        try_claim_matching_ticket(&path_loop_b, &cfg, &ctx_b, "add", &fp_loop_b)
            .expect("loop-b apply must claim");
        consume_claimed_ticket(&path_loop_b).expect("loop-b apply settles");
        assert!(
            !path_loop_b.exists(),
            "loop-b apply consumes only its own ticket"
        );
    }

    /// U2: a later verify for a distinct intent must not
    /// invalidate an unrelated pending operation.
    #[test]
    fn test_u2_later_verify_does_not_invalidate_prior_intent() {
        let ws = temp_workspace();
        let ctx = make_ctx("loop-1", "executor", true);
        let cfg = default_config(true);

        // 1. Verify intent A — record ticket.
        let payload_a = r#"{"title":"A"}"#;
        let fp_a = mutation_fingerprint("add", payload_a, "loop-1", "executor");
        let path_a = scoped_for(&ws, "add", payload_a, "loop-1", "executor");
        record_ticket(&path_a, &fp_a, "loop-1", "executor").expect("record A");

        // 2. Verify intent B — record ticket at a different file.
        let payload_b = r#"{"title":"B"}"#;
        let fp_b = mutation_fingerprint("add", payload_b, "loop-1", "executor");
        let path_b = scoped_for(&ws, "add", payload_b, "loop-1", "executor");
        record_ticket(&path_b, &fp_b, "loop-1", "executor").expect("record B");

        // Both files exist simultaneously.
        assert!(path_a.exists(), "intent A must still be applicable");
        assert!(path_b.exists(), "intent B must be applicable");

        // Either intent can be applied at most once.
        try_claim_matching_ticket(&path_a, &cfg, &ctx, "add", &fp_a).expect("A apply claims");
        consume_claimed_ticket(&path_a).expect("A apply settles");
        assert!(!path_a.exists(), "A apply consumes intent A ticket");
        assert!(path_b.exists(), "intent B must survive A's apply");
        try_claim_matching_ticket(&path_b, &cfg, &ctx, "add", &fp_b).expect("B apply claims");
        consume_claimed_ticket(&path_b).expect("B apply settles");
        assert!(!path_b.exists(), "B apply consumes intent B ticket");
    }

    /// U2: distinct payloads produce distinct ticket files (the
    /// intent digest segment disambiguates).
    #[test]
    fn test_u2_distinct_intents_produce_distinct_files() {
        let ws = temp_workspace();
        let payload_a = r#"{"title":"A"}"#;
        let payload_b = r#"{"title":"B"}"#;
        let path_a = scoped_for(&ws, "add", payload_a, "loop-1", "executor");
        let path_b = scoped_for(&ws, "add", payload_b, "loop-1", "executor");
        assert_ne!(path_a, path_b, "distinct intents must produce distinct files");
    }

    // ── Unit 1 (task confirmation): pending gate precheck ───────────

    fn task_with_confirmation(loop_id: &str, hat_id: &str, confirmed: bool) -> ralph_core::Task {
        let mut task = ralph_core::Task::new("protected target".to_string(), 2);
        task.loop_id = Some(loop_id.to_string());
        let mut cfm = ralph_core::TaskConfirmation::new_pending(
            "digest-1".to_string(),
            loop_id.to_string(),
            hat_id.to_string(),
        );
        if confirmed {
            cfm.mark_confirmed();
        }
        task.confirmation = Some(Box::new(cfm));
        task
    }

    fn store_with_task(tmp: &TempDir, task: ralph_core::Task) -> ralph_core::TaskStore {
        let path = tmp.path().join("tasks.jsonl");
        let mut store = ralph_core::TaskStore::load(&path).expect("load store");
        store.add(task);
        store
    }

    #[test]
    fn test_pending_confirmation_precheck_denies_same_scope() {
        let tmp = temp_workspace();
        let store = store_with_task(&tmp, task_with_confirmation("loop-a", "coordinator", false));
        let ctx = make_ctx("loop-a", "coordinator", true);
        let err = pending_confirmation_precheck(&store, &default_config(true), &ctx, "add")
            .expect_err("same-scope pending confirmation must deny");
        let msg = err.to_string();
        assert!(msg.contains(DENY_PREFIX), "stable gate prefix: {msg}");
        assert!(msg.contains("confirmation_required"), "stable token: {msg}");
        assert!(
            msg.contains("ralph tools task confirm"),
            "recovery hint must name the confirm command: {msg}"
        );
    }

    #[test]
    fn test_pending_confirmation_precheck_human_bypass() {
        let tmp = temp_workspace();
        let store = store_with_task(&tmp, task_with_confirmation("loop-a", "coordinator", false));
        let ctx = make_ctx("loop-a", "coordinator", false);
        pending_confirmation_precheck(&store, &default_config(true), &ctx, "add")
            .expect("human CLI bypasses the pending gate");
    }

    #[test]
    fn test_pending_confirmation_precheck_gate_off_bypass() {
        let tmp = temp_workspace();
        let store = store_with_task(&tmp, task_with_confirmation("loop-a", "coordinator", false));
        let ctx = make_ctx("loop-a", "coordinator", true);
        pending_confirmation_precheck(&store, &default_config(false), &ctx, "add")
            .expect("gate-off bypasses the pending gate");
    }

    #[test]
    fn test_pending_confirmation_precheck_unsafe_hatch_bypass() {
        let tmp = temp_workspace();
        let store = store_with_task(&tmp, task_with_confirmation("loop-a", "coordinator", false));
        let ctx = make_ctx("loop-a", "coordinator", true);
        let mut config = default_config(true);
        config.allow_unsafe_task_mutate = true;
        pending_confirmation_precheck(&store, &config, &ctx, "add")
            .expect("unsafe escape hatch bypasses the pending gate");
    }

    #[test]
    fn test_pending_confirmation_precheck_allows_confirmed_row() {
        let tmp = temp_workspace();
        let store = store_with_task(&tmp, task_with_confirmation("loop-a", "coordinator", true));
        let ctx = make_ctx("loop-a", "coordinator", true);
        pending_confirmation_precheck(&store, &default_config(true), &ctx, "add")
            .expect("confirmed rows must not block the next mutation");
    }

    #[test]
    fn test_pending_confirmation_precheck_allows_other_scope() {
        let tmp = temp_workspace();
        let store = store_with_task(&tmp, task_with_confirmation("loop-b", "executor", false));
        let ctx = make_ctx("loop-a", "coordinator", true);
        pending_confirmation_precheck(&store, &default_config(true), &ctx, "ensure")
            .expect("pending confirmations from another loop/hat must not block");
    }
}
