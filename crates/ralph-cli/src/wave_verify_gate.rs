//! Precheck→Apply one-shot ticket gate for `ralph wave emit` mutations
//! invoked by agents.
//!
//! Plan `2026-07-22-001-feat-wave-protocol-suite-default-plan` U1:
//! closes the OPAC drift window where an agent could `verify` a
//! payload set, edit one payload, and `emit` a *different* batch
//! that nonetheless passed the existing policy / ACL checks. The
//! contract mirrors `task_verify_gate`:
//!
//! 1. Agent invokes `ralph wave verify <topic> --payloads ...` and
//!    passes all policy / origin / ACL gates. On success the gate
//!    writes a one-shot file at
//!    `<workspace>/.ralph/agent/.ralph-wave-verify-ticket`.
//! 2. The same agent (same `loop_id` + `hat_id`) then invokes
//!    `ralph wave emit <topic> --payloads ...` with the same
//!    payload set. Before the JSONL is touched, the gate reads
//!    and consumes the on-disk ticket. The SHA-256 fingerprint
//!    of `<topic>\n<canonical_payloads>\n<loop_id>\n<hat_id>` must
//!    match; otherwise the gate denies with the stable prefix
//!    `wave_verify_gate denied` so callers (and reviewers) can
//!    grep for the root cause.
//!
//! Human CLI invocations (`is_agent_context == false`) bypass
//! the gate entirely. Operators must not be locked out by a
//! stuck ticket.
//!
//! `consume_ticket` deletes the ticket file as a side effect so
//! the gate is one-shot: a successful emit burns the ticket, a
//! failed emit leaves the ticket in place for the agent to retry
//! with the same payload set.
//!
//! `--unsafe-no-policy-check` is the **policy** bypass only; it
//! does NOT bypass the OPAC ticket gate. If the gate denies, the
//! agent must re-run `ralph wave verify` with the *current*
//! payloads and try again.

use crate::operation_guard::OperationContext;
use anyhow::Context as _;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Path-relative marker so a denied agent knows where to look.
pub const TICKET_REL_PATH: &str = ".ralph/agent/.ralph-wave-verify-ticket";

/// 2026-07-24-003 plan U6: sibling file that records an in-flight
/// claim of the on-disk ticket. The prepare/claim/consume state
/// machine is:
///
/// | state | `.ralph-wave-verify-ticket` | `.ralph-wave-verify-ticket.claim` |
/// |---|---|---|
/// | `prepared` | exists | missing |
/// | `claimed`  | exists | exists |
/// | `consumed` | missing | missing |
///
/// The claim marker carries the unix-second timestamp of the
/// claim so an operator can `cat` the workspace and see when the
/// ticket was taken into Apply.
pub const TICKET_CLAIM_REL_PATH: &str = ".ralph/agent/.ralph-wave-verify-ticket.claim";

/// Stable deny prefix (mirrors `task_verify_gate denied` for
/// grep-ability).
pub const DENY_PREFIX: &str = "wave_verify_gate denied";

/// Compute a stable fingerprint for a (topic, payload-list, loop, hat)
/// tuple.
///
/// The canonical payload format joins payloads with `\u{1F}` (Unit
/// Separator) so a verify-then-apply with the *same* payload set
/// produces the same fingerprint; any drift (payload edited, payload
/// added, payload reordered) breaks the match and forces the agent
/// back to `verify`. The hash is intentionally a SHA-256 hex (64
/// chars) so a human can paste it into a recovery command if the
/// on-disk ticket is corrupted.
pub fn emission_fingerprint(
    topic: &str,
    canonical_payloads: &str,
    loop_id: &str,
    hat_id: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(topic.as_bytes());
    hasher.update(b"\n");
    hasher.update(canonical_payloads.as_bytes());
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

/// Render a payload list to its canonical form for fingerprinting.
///
/// Joins payloads with `\u{1F}` (Unit Separator) — the same
/// delimiter `wave::compute_payload_digest` uses — so verify and
/// emit hash the *exact* same byte string regardless of how the
/// caller combined the payload array.
pub fn canonical_payload_form(payloads: &[String]) -> String {
    let mut joined = String::new();
    for (i, p) in payloads.iter().enumerate() {
        if i > 0 {
            joined.push('\u{1F}');
        }
        joined.push_str(p);
    }
    joined
}

/// Resolve the ticket file path for a workspace.
pub fn ticket_path(workspace: &Path) -> PathBuf {
    workspace.join(TICKET_REL_PATH)
}

/// 2026-07-24-003 plan U6: resolve the claim marker path for a
/// workspace.
pub fn claim_marker_path(workspace: &Path) -> PathBuf {
    workspace.join(TICKET_CLAIM_REL_PATH)
}

/// Write a one-shot ticket so the next `require_ticket` for the
/// same (loop, hat) will succeed.
///
/// The ticket format is:
/// ```text
/// <sha256-fingerprint>\u{1F}<topic>\u{1F}<loop_id>\u{1F}<hat_id>\u{1F}<unix-timestamp-secs>
/// ```
/// One line, no trailing newline. Fields are separated by `\u{1F}`
/// (Unit Separator) so an empty `loop_id` (e.g. when the marker is
/// missing and no env override is set) cannot collapse into the
/// adjacent field under `split_whitespace`. The trailing timestamp
/// lets us reject tickets older than a configurable max age
/// (caller's responsibility; this module does not age out by
/// default).
pub fn record_ticket(
    path: &Path,
    fingerprint: &str,
    topic: &str,
    loop_id: &str,
    hat_id: &str,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create parent dir for {}", path.display()))?;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let line = format!("{fingerprint}\u{1F}{topic}\u{1F}{loop_id}\u{1F}{hat_id}\u{1F}{now}\n");
    std::fs::write(path, line).with_context(|| format!("write ticket to {}", path.display()))?;
    Ok(())
}

/// Read the on-disk ticket and return its parsed fields.
///
/// Returns `Ok(None)` when the file does not exist (the common
/// "agent never verified" case). Returns `Err` for any I/O
/// failure or malformed line.
///
/// Fields are separated by `\u{1F}` (Unit Separator) so an empty
/// `loop_id` cannot collapse under `split_whitespace`; see
/// `record_ticket` for the layout.
pub fn read_ticket(path: &Path) -> anyhow::Result<Option<TicketRecord>> {
    if !path.exists() {
        return Ok(None);
    }
    let raw =
        std::fs::read_to_string(path).with_context(|| format!("read ticket {}", path.display()))?;
    let line = raw.lines().next().unwrap_or("").trim_end();
    if line.is_empty() {
        return Ok(None);
    }
    let mut parts = line.split('\u{1F}');
    let fingerprint = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("wave_verify_gate: ticket line missing fingerprint"))?
        .to_string();
    let topic = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("wave_verify_gate: ticket line missing topic"))?
        .to_string();
    let loop_id = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("wave_verify_gate: ticket line missing loop_id"))?
        .to_string();
    let hat_id = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("wave_verify_gate: ticket line missing hat_id"))?
        .to_string();
    let ts_secs: u64 = parts
        .next()
        .ok_or_else(|| anyhow::anyhow!("wave_verify_gate: ticket line missing timestamp"))?
        .parse()
        .unwrap_or(0);
    Ok(Some(TicketRecord {
        fingerprint,
        topic,
        loop_id,
        hat_id,
        timestamp_secs: ts_secs,
    }))
}

/// On-disk ticket payload, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TicketRecord {
    pub fingerprint: String,
    pub topic: String,
    pub loop_id: String,
    pub hat_id: String,
    pub timestamp_secs: u64,
}

/// Consume a ticket by deleting the file. The caller has confirmed
/// the fingerprint matches the pending emit; the ticket is now
/// burned so a retry needs a fresh `verify`.
///
/// 2026-07-24-003 plan U6: production callers should now use
/// [`consume_claimed_ticket`] which removes both the ticket and
/// the claim marker (the proper `prepared → claimed → consumed`
/// finalisation). `consume_ticket` is retained as a no-claim
/// delete helper used by tests / one-shot scripts.
pub fn consume_ticket(path: &Path) -> anyhow::Result<()> {
    if path.exists() {
        std::fs::remove_file(path).with_context(|| format!("delete ticket {}", path.display()))?;
    }
    Ok(())
}

/// 2026-07-24-003 plan U6: claim marker is removed (Apply failed
/// before completion; the ticket must be returned to `prepared`
/// so the next attempt can claim it again).
///
/// This is the roll-back side of `claim_ticket`: the underlying
/// ticket file is untouched. After this call the workspace is in
/// the original `prepared` state.
pub fn restore_ticket(workspace: &Path) -> anyhow::Result<()> {
    let claim = claim_marker_path(workspace);
    if claim.exists() {
        std::fs::remove_file(&claim)
            .with_context(|| format!("delete claim marker {}", claim.display()))?;
    }
    Ok(())
}

/// 2026-07-24-003 plan U6: finalise a successful Apply by
/// removing both the ticket and the claim marker. Idempotent —
/// either file already missing is a no-op.
///
/// `cleanup_failed` is set to `true` if either remove_file
/// returned an I/O error; the caller decides whether to surface
/// `applied_cleanup_pending` to the agent.
///
/// The claim marker is always removed when present so a retry
/// sees a clean workspace; the underlying ticket is what we
/// surface as cleanup-pending when the delete I/O errors.
pub fn consume_claimed_ticket(workspace: &Path) -> anyhow::Result<bool> {
    let claim = claim_marker_path(workspace);
    let ticket = ticket_path(workspace);
    // Always drop the claim marker first so retries are
    // unblocked even when the ticket delete errors.
    if claim.exists() {
        if let Err(err) = std::fs::remove_file(&claim) {
            eprintln!(
                "warning: failed to delete claim marker at {}: {}",
                claim.display(),
                err
            );
        }
    }
    let mut cleanup_failed = false;
    if ticket.exists() {
        if let Err(err) = std::fs::remove_file(&ticket) {
            eprintln!(
                "warning: failed to delete ticket at {}: {}",
                ticket.display(),
                err
            );
            cleanup_failed = true;
        }
    }
    Ok(cleanup_failed)
}

/// Combined read + delete: returns the ticket if it exists, and
/// then removes the file in one call so a successful emit burns
/// the ticket atomically (modulo the small I/O window between
/// `read_ticket` and `consume_ticket` — accepted).
///
/// 2026-07-24-003 plan U6: deprecated for production paths; kept
/// as a public API for callers that want the legacy one-shot
/// consume (unit tests + operator one-liners).
#[allow(dead_code)]
pub fn read_and_consume_ticket(path: &Path) -> anyhow::Result<Option<TicketRecord>> {
    let record = read_ticket(path)?;
    if record.is_some() {
        consume_ticket(path)?;
    }
    Ok(record)
}

/// The full gate check. Returns `Ok(())` when the emit may
/// proceed; `Err` with a stable, machine-grepable deny prefix
/// when the gate denies.
///
/// 2026-07-24-003 plan U6 behaviour change: a successful claim
/// writes the claim marker (not the legacy delete). The Apply
/// step is responsible for either `consume_claimed_ticket` (on
/// success) or `restore_ticket` (on failure). This closes the
/// drift window where Apply failed mid-flight but the ticket was
/// already gone — the agent no longer needs to re-run
/// `ralph wave verify` after a recoverable failure.
///
/// Behaviour:
/// - Human CLI (`!ctx.is_agent_context`) → `Ok(())` always.
/// - Agent + no ticket on disk → `Err(denied: missing ticket)`.
/// - Agent + ticket with wrong (loop|hat|topic|fingerprint) →
///   `Err(denied: stale or mismatched ticket)`. The ticket file
///   is NOT touched — a clean retry with the right payloads can
///   still succeed.
/// - Agent + ticket already claimed → `Err(denied: ticket
///   already claimed)`. The marker carries the in-flight
///   timestamp so an operator can spot stuck claims.
/// - Agent + ticket matches → write the claim marker and return
///   `Ok(())`.
pub fn require_ticket(
    workspace: &Path,
    ctx: &OperationContext,
    topic: &str,
    fingerprint: &str,
) -> anyhow::Result<()> {
    // Human CLI bypasses OPAC ticket gate — operators must not be
    // locked out by a stuck ticket (mirrors task_verify_gate).
    if !ctx.is_agent_context {
        return Ok(());
    }
    let loop_id = ctx.current_loop_id.as_deref().unwrap_or("");
    let hat_id = ctx.current_hat_id.as_deref().unwrap_or("");
    let ticket = ticket_path(workspace);
    let claim = claim_marker_path(workspace);

    if claim.exists() {
        // Orphan claim (ticket already gone): clear so the agent can
        // re-verify instead of being permanently locked out.
        if !ticket.exists() {
            let _ = std::fs::remove_file(&claim);
        } else {
            return Err(anyhow::anyhow!(
                "{DENY_PREFIX} '{topic}': ticket already claimed — \
                 another Apply is in flight or a previous attempt left a \
                 claim marker. If Store already applied this wave, retry \
                 after `ralph wave inspect` confirms; otherwise wait or \
                 re-verify. Hat: '{hat_id}' Loop: '{loop_id}'."
            ));
        }
    }

    let record = match read_ticket(&ticket)? {
        Some(r) => r,
        None => {
            return Err(anyhow::anyhow!(
                "{DENY_PREFIX} '{topic}': no verify ticket — \
                 run `ralph wave verify {topic} --payloads ...` first to record a matching ticket, \
                 then re-invoke `ralph wave emit` with the same payloads. \
                 Hat: '{hat_id}' Loop: '{loop_id}'."
            ));
        }
    };
    if record.fingerprint != fingerprint {
        return Err(anyhow::anyhow!(
            "{DENY_PREFIX} '{topic}': ticket fingerprint mismatch (on-disk={on_disk} pending={pending}) — \
             the payloads you are about to emit differ from the ones you verified. \
             Re-run `ralph wave verify {topic}` with the *current* payloads and try again. \
             Hat: '{hat_id}' Loop: '{loop_id}'.",
            on_disk = record.fingerprint,
            pending = fingerprint
        ));
    }
    if record.topic != topic {
        return Err(anyhow::anyhow!(
            "{DENY_PREFIX} '{topic}': ticket was recorded for topic '{rec_topic}' \
             but emit targets '{topic}' — tickets are bound to a single topic. \
             Re-run verify with the intended topic. \
             Hat: '{hat_id}' Loop: '{loop_id}'.",
            rec_topic = record.topic,
        ));
    }
    if record.loop_id != loop_id || record.hat_id != hat_id {
        return Err(anyhow::anyhow!(
            "{DENY_PREFIX} '{topic}': ticket (loop, hat) = ({rec_loop}, {rec_hat}) \
             but caller is ({loop_id}, {hat_id}) — tickets are bound to the verifying hat. \
             Re-run verify from this hat. \
             Hat: '{hat_id}' Loop: '{loop_id}'.",
            rec_loop = record.loop_id,
            rec_hat = record.hat_id
        ));
    }
    // Successful claim — `create_new` so concurrent emits cannot both
    // hold the claim (closes TOCTOU vs plain write).
    if let Some(parent) = claim.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!("create parent dir for claim marker (hat={hat_id} loop={loop_id})")
        })?;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&claim)
    {
        Ok(mut f) => {
            use std::io::Write;
            f.write_all(format!("{now}\n").as_bytes())
                .with_context(|| "write claim marker contents")?;
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(anyhow::anyhow!(
                "{DENY_PREFIX} '{topic}': ticket already claimed — \
                 another Apply is in flight. Hat: '{hat_id}' Loop: '{loop_id}'."
            ));
        }
        Err(err) => {
            return Err(anyhow::anyhow!("write claim marker failed: {err}"));
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod wave_verify_gate_tests {
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

    #[test]
    fn test_fingerprint_stable_for_same_payloads() {
        let canonical =
            canonical_payload_form(&[r#"{"dim":"a"}"#.to_string(), r#"{"dim":"b"}"#.to_string()]);
        let a = emission_fingerprint("review.wave.ready", &canonical, "loop-1", "executor");
        let b = emission_fingerprint("review.wave.ready", &canonical, "loop-1", "executor");
        assert_eq!(a, b, "same inputs must hash equal");
        assert_eq!(a.len(), 64, "SHA-256 hex must be 64 chars");
    }

    #[test]
    fn test_fingerprint_differs_for_different_payload() {
        let canonical_a = canonical_payload_form(&[r#"{"dim":"a"}"#.to_string()]);
        let canonical_b = canonical_payload_form(&[r#"{"dim":"b"}"#.to_string()]);
        let a = emission_fingerprint("review.wave.ready", &canonical_a, "loop-1", "executor");
        let b = emission_fingerprint("review.wave.ready", &canonical_b, "loop-1", "executor");
        assert_ne!(a, b, "different payloads must hash different");
    }

    #[test]
    fn test_canonical_payload_form_unit_separator() {
        let joined =
            canonical_payload_form(&[r#"{"a":"1"}"#.to_string(), r#"{"a":"2"}"#.to_string()]);
        // Verify the separator character is present and the order matches.
        assert_eq!(joined, "{\"a\":\"1\"}\u{1F}{\"a\":\"2\"}");
    }

    #[test]
    fn test_record_then_consume_ok() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        record_ticket(&path, "fp-1", "review.wave.ready", "loop-1", "executor").expect("record");
        let record = read_ticket(&path).expect("read").expect("present");
        assert_eq!(record.fingerprint, "fp-1");
        assert_eq!(record.topic, "review.wave.ready");
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
        record_ticket(&path, "fp", "t", "l", "h").expect("record");
        consume_ticket(&path).expect("first consume");
        consume_ticket(&path).expect("second consume is noop");
    }

    #[test]
    fn test_require_ticket_agent_no_record_denied() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", true);
        let canonical = canonical_payload_form(&[r#"{"dim":"x"}"#.to_string()]);
        let fp = emission_fingerprint("review.wave.ready", &canonical, "loop-1", "executor");
        let err = require_ticket(ws.path(), &ctx, "review.wave.ready", &fp)
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
            msg.contains("ralph wave verify review.wave.ready"),
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
        let canonical = canonical_payload_form(&[r#"{"dim":"x"}"#.to_string()]);
        let fp = emission_fingerprint("review.wave.ready", &canonical, "loop-1", "executor");
        // Human CLI with no ticket on disk → still Ok.
        require_ticket(ws.path(), &ctx, "review.wave.ready", &fp).expect("human must bypass");
    }

    #[test]
    fn test_require_ticket_match_claims_and_allows() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let claim = claim_marker_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", true);
        let canonical = canonical_payload_form(&[r#"{"dim":"x"}"#.to_string()]);
        let fp = emission_fingerprint("review.wave.ready", &canonical, "loop-1", "executor");
        record_ticket(&path, &fp, "review.wave.ready", "loop-1", "executor").expect("record");
        require_ticket(ws.path(), &ctx, "review.wave.ready", &fp)
            .expect("matching ticket must allow");
        // U6: ticket is NOT consumed yet; the claim marker is
        // written instead. The Apply step is responsible for
        // `consume_claimed_ticket` (success) or `restore_ticket`
        // (failure).
        assert!(path.exists(), "matching emit must NOT delete the ticket");
        assert!(claim.exists(), "matching emit must write the claim marker");
    }

    #[test]
    fn test_require_ticket_fingerprint_mismatch_keeps_ticket() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", true);
        let canonical_a = canonical_payload_form(&[r#"{"dim":"a"}"#.to_string()]);
        let canonical_b = canonical_payload_form(&[r#"{"dim":"b"}"#.to_string()]);
        let on_disk_fp =
            emission_fingerprint("review.wave.ready", &canonical_a, "loop-1", "executor");
        let pending_fp =
            emission_fingerprint("review.wave.ready", &canonical_b, "loop-1", "executor");
        record_ticket(
            &path,
            &on_disk_fp,
            "review.wave.ready",
            "loop-1",
            "executor",
        )
        .expect("record");
        let err = require_ticket(ws.path(), &ctx, "review.wave.ready", &pending_fp)
            .expect_err("mismatch must deny");
        let msg = err.to_string();
        assert!(msg.contains("fingerprint mismatch"), "must explain: {msg}");
        // U6: mismatch does NOT consume the ticket — the agent
        // can re-verify with the matching payload and retry
        // without re-running `ralph wave verify` against a
        // ticket the gate already deleted.
        assert!(path.exists(), "U6 mismatch must NOT consume the ticket");
    }

    #[test]
    fn test_require_ticket_topic_mismatch_denied() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", true);
        let canonical = canonical_payload_form(&[r#"{"dim":"x"}"#.to_string()]);
        let fp = emission_fingerprint("review.wave.ready", &canonical, "loop-1", "executor");
        record_ticket(&path, &fp, "review.wave.ready", "loop-1", "executor").expect("record");
        let err = require_ticket(ws.path(), &ctx, "review.different", &fp)
            .expect_err("topic mismatch must deny");
        let msg = err.to_string();
        assert!(
            msg.contains("review.wave.ready"),
            "must echo recorded topic: {msg}"
        );
    }

    #[test]
    fn test_require_ticket_loop_hat_mismatch_denied() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "worker", true);
        let canonical = canonical_payload_form(&[r#"{"dim":"x"}"#.to_string()]);
        let fp = emission_fingerprint("review.wave.ready", &canonical, "loop-1", "executor");
        record_ticket(&path, &fp, "review.wave.ready", "loop-1", "executor").expect("record");
        let err = require_ticket(ws.path(), &ctx, "review.wave.ready", &fp)
            .expect_err("hat mismatch must deny");
        let msg = err.to_string();
        assert!(
            msg.contains("ticket (loop, hat)"),
            "must explain hat binding: {msg}"
        );
    }

    #[test]
    fn test_require_ticket_double_claim_rejected() {
        // U6: a second agent that tries to claim the same
        // ticket sees the in-flight claim marker and is denied
        // with the stable prefix.
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let ctx = make_ctx("loop-1", "executor", true);
        let canonical = canonical_payload_form(&[r#"{"dim":"x"}"#.to_string()]);
        let fp = emission_fingerprint("review.wave.ready", &canonical, "loop-1", "executor");
        record_ticket(&path, &fp, "review.wave.ready", "loop-1", "executor").expect("record");
        require_ticket(ws.path(), &ctx, "review.wave.ready", &fp)
            .expect("first claim must succeed");
        let err = require_ticket(ws.path(), &ctx, "review.wave.ready", &fp)
            .expect_err("second claim must deny");
        let msg = err.to_string();
        assert!(
            msg.contains("already claimed"),
            "must explain the claim collision: {msg}"
        );
    }

    #[test]
    fn test_consume_claimed_ticket_removes_both() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let claim = claim_marker_path(ws.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "stub\n").unwrap();
        std::fs::write(&claim, "stub\n").unwrap();
        let cleanup_failed = consume_claimed_ticket(ws.path()).expect("consume must succeed");
        assert!(!cleanup_failed, "no cleanup failure");
        assert!(!path.exists(), "ticket must be removed");
        assert!(!claim.exists(), "claim marker must be removed");
    }

    #[test]
    fn test_restore_ticket_keeps_underlying_ticket() {
        let ws = temp_workspace();
        let path = ticket_path(ws.path());
        let claim = claim_marker_path(ws.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "stub\n").unwrap();
        std::fs::write(&claim, "stub\n").unwrap();
        restore_ticket(ws.path()).expect("restore must succeed");
        assert!(path.exists(), "underlying ticket must survive restore");
        assert!(!claim.exists(), "claim marker must be removed");
    }
}
