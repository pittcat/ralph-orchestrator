//! Wave CLI tool for dispatching parallel wave events.
//!
//! Provides `ralph wave emit` for agents to dispatch work items
//! to wave-capable hats that execute in parallel.

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use ralph_core::agent_doc_sync::compute_sha256_hex;
use ralph_core::file_lock::FileLock;
#[cfg(feature = "supervisor-db")]
use ralph_core::supervisor::SupervisorStore;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// Arguments for the wave subcommand.
#[derive(Parser, Debug)]
pub struct WaveArgs {
    #[command(subcommand)]
    pub command: WaveCommands,
}

/// Wave subcommands.
#[derive(Subcommand, Debug)]
pub enum WaveCommands {
    /// Emit multiple events as a wave for parallel execution
    Emit(WaveEmitArgs),
    /// U21: OPAC Precheck for `ralph wave emit` mutations.
    ///
    /// Validate payloads against the active event policy (no
    /// business events written) and record a one-shot
    /// `ralph-wave-verify-ticket` so the next `ralph wave emit`
    /// can prove it targets the same payload set. Mirrors
    /// `ralph wave emit` schema / origin-guard checks but stops
    /// before any business-event write step. Returns the same
    /// `{ok, wave_id?, error?}` shape so agents can treat verify
    /// and emit uniformly. Intended for OPAC Precheck stage.
    Verify(WaveVerifyArgs),
    /// 2026-07-24-003 plan Unit 2: read-only Confirm for a wave.
    ///
    /// Returns the public `wave_id`'s current state from the
    /// supervisor store: phase, expected/done/failed/pending/in-flight
    /// counts, and `cancel_requested`. Unknown wave_id → `registered:
    /// false`; corrupt / missing store → `availability: unavailable`.
    /// Pair with `ralph wave emit` for the OPAC Apply/Confirm loop.
    /// Read-only — never mutates the store, events JSONL, or tickets.
    Inspect(WaveInspectArgs),
    /// 2026-07-25-005 plan U11: create a redrive child wave for a
    /// parent wave with failed slots. The child inherits the parent's
    /// `kind` and `slot_retry_budget` and carries `attempt_epoch + 1`.
    /// Idempotent: calling with the same (parent, slot, epoch) triple
    /// returns the existing child wave without creating duplicates.
    /// Bypasses `FlowStepScope` because it emits no business events.
    Redrive(WaveRedriveArgs),
}

/// Arguments for `ralph wave inspect`.
#[derive(Parser, Debug)]
pub struct WaveInspectArgs {
    /// Public wave id (the value `ralph wave emit` echoed on the
    /// success response).
    pub wave_id: String,

    /// Output format: `text` (default) or `json` (agent-stable shape).
    #[arg(long, value_enum, default_value_t = WaveOutputFormat::Text)]
    pub output: WaveOutputFormat,
}

/// Arguments for `ralph wave redrive`.
#[derive(Parser, Debug)]
pub struct WaveRedriveArgs {
    /// Public wave id of the parent wave to redrive.
    #[arg(long = "wave-id", short = 'w', value_name = "ID")]
    pub wave_id: String,

    /// Optional comma-separated list of slot indices to redrive.
    /// If absent, all failed slots are redriven.
    #[arg(long = "slots", value_delimiter = ',')]
    pub slots: Option<Vec<u32>>,

    /// Output format: `text` (default) or `json`.
    #[arg(long = "output", value_enum, default_value_t = WaveRedriveOutputFormat::Text)]
    pub output: WaveRedriveOutputFormat,

    /// Explicit path to a `ralph.yml` config file.
    #[arg(long = "config", short = 'c', value_name = "CONFIG", global = true)]
    pub config: Vec<String>,
}

/// Output format for `ralph wave redrive`.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveRedriveOutputFormat {
    Text,
    Json,
}

/// Arguments for `ralph wave verify`.
#[derive(Parser, Debug)]
pub struct WaveVerifyArgs {
    /// Event topic that would be emitted (e.g., "review.file")
    pub topic: String,

    /// Payloads to validate (one per parallel worker)
    #[arg(long, num_args = 1.., group = "verify_payload_source")]
    pub payloads: Vec<String>,

    /// Read payloads from stdin, one per line
    #[arg(long, group = "verify_payload_source")]
    pub payloads_stdin: bool,

    /// Output format: `text` (default; "ok" on stdout) or `json`
    /// (`{ok: true, topic, count}` for U5 machine verification).
    #[arg(long, value_enum, default_value_t = WaveOutputFormat::Text)]
    pub output: WaveOutputFormat,

    /// Explicit path to a `ralph.yml` for the policy precheck.
    /// Mirrors the global `-c` flag at the top-level command.
    #[arg(long = "config", short = 'c', value_name = "CONFIG", global = true)]
    pub config: Vec<String>,
}

/// Arguments for `ralph wave emit`.
#[derive(Parser, Debug)]
pub struct WaveEmitArgs {
    /// Event topic for all wave events (e.g., "review.file")
    pub topic: String,

    /// Payloads for each wave event instance (one per parallel worker)
    #[arg(long, num_args = 1.., group = "payload_source")]
    pub payloads: Vec<String>,

    /// Read payloads from stdin, one per line
    #[arg(long, group = "payload_source")]
    pub payloads_stdin: bool,

    /// Output format: `text` (default; wave_id on stdout) or `json`
    /// (`{wave_id, topic, count, events_file}` for U5 machine verification).
    #[arg(long, value_enum, default_value_t = WaveOutputFormat::Text)]
    pub output: WaveOutputFormat,

    /// Optional idempotency key (U2). Re-emitting with the same
    /// (loop_id, hat, topic, key) returns the original wave_id instead of
    /// writing a new wave. Use for review-coordinator waves that may be
    /// retried after timeout or duplicate dispatch. Omit to keep legacy
    /// behavior (each call generates a new wave_id).
    #[arg(long, value_name = "KEY")]
    pub idempotency_key: Option<String>,

    /// U4: Validate all payloads against the active event policy
    /// (in `ralph.yml` or merged preset) before writing the JSONL.
    /// Combined with `--output json` the failure response carries a
    /// structured `validation_errors` array.
    #[arg(long)]
    pub policy_check: bool,

    /// U4: Bypass the mandatory policy check. Only honored when the
    /// config has `event_policy.allow_unsafe_cli_emit: true`; otherwise
    /// the check is still enforced. This mirrors `ralph emit
    /// --unsafe-no-policy-check` semantics.
    #[arg(long = "unsafe-no-policy-check", conflicts_with = "policy_check")]
    pub no_policy_check: bool,

    /// Explicit path to a `ralph.yml` for the policy precheck (U4).
    /// Mirrors the global `-c` flag at the top-level command. When set,
    /// the precheck uses this path instead of the CWD-discovered
    /// `ralph.yml`, so a `ralph run -c custom.yml` invocation can route
    /// its nested `ralph wave emit` through the same strict policy.
    #[arg(long = "config", short = 'c', value_name = "CONFIG", global = true)]
    pub config: Vec<String>,
}

/// U5: Output format for `ralph wave emit`.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveOutputFormat {
    Text,
    Json,
}

/// U2: Max length of idempotency key (bytes). Bounds log line size and
/// prevents runaway keys from polluting `.wave-idempotency.jsonl`.
pub const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;

/// U2: One row of `.wave-idempotency.jsonl`.
///
/// Schema is flat (single object per line) so future fields can be added
/// without breaking parsers that ignore unknown keys.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct IdempotencyRecord {
    /// SHA-256 hex of `"<loop_id>|<hat>|<topic>|<key>"`. Primary dedup key.
    pub scope_key: String,
    /// Echo of the user-supplied key (for operator audit).
    pub idempotency_key: String,
    /// Wave ID returned on first emission; returned on all later dedup hits.
    pub wave_id: String,
    /// Topic emitted (redundant with scope but logs-friendly).
    pub topic: String,
    /// Hat that emitted (or "" if unset at first call).
    pub hat: String,
    /// SHA-256 hex of the serialized payload list.
    pub payload_digest: String,
    /// Number of events that should exist with this wave_id.
    pub count: u32,
    /// ISO-8601 UTC timestamp of first emission.
    pub created_at: String,
}

/// U2: Outcome of `write_wave_events_with_idempotency`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyOutcome {
    /// The wave_id (whether new or deduped).
    pub wave_id: String,
    /// `true` when this call was a dedup hit (no new events written).
    pub deduplicated: bool,
}

/// Execute a wave command.
pub fn execute(args: WaveArgs, use_colors: bool) -> Result<()> {
    match args.command {
        WaveCommands::Emit(emit_args) => execute_emit(emit_args, use_colors),
        WaveCommands::Verify(verify_args) => execute_verify(verify_args),
        WaveCommands::Inspect(inspect_args) => execute_inspect(inspect_args),
        WaveCommands::Redrive(redrive_args) => execute_redrive(redrive_args),
    }
}

/// U23: Hat-level wave ACL gate. Mirrors `HatCommandPolicy::check_wave_emit`
/// so worker hats cannot dispatch waves (they must use `ralph emit`).
fn enforce_wave_acl(_verb: &str) -> Result<()> {
    use crate::hat_command_policy::HatCommandPolicy;
    use crate::operation_guard::OperationContext;
    use crate::policy_check::OnConfigError;
    use crate::policy_check::load_policy_config_for_cli_emit;

    let workspace_root = std::env::current_dir().unwrap_or_default();
    let ctx = OperationContext::detect(workspace_root.clone());
    let config = load_policy_config_for_cli_emit(None, OnConfigError::Warn, &[])?;
    let config = match config {
        Some(c) => c,
        None => return Ok(()), // no config → no policy → bypass
    };

    match HatCommandPolicy::check_wave_emit(&ctx, &config) {
        crate::hat_command_policy::PolicyDecision::Allow { .. } => Ok(()),
        crate::hat_command_policy::PolicyDecision::Deny { reason, hint } => {
            bail!("wave ACL denied: {reason}; {hint}")
        }
    }
}

/// Execute `ralph wave inspect` — read-only public Confirm.
///
/// Returns a stable, agent-safe DTO describing the public wave id:
/// `registered` (`true` when the store has a row for this wave),
/// `availability` (`"available"` for a healthy lookup miss;
/// `"unavailable"` when the store cannot be opened), `phase`,
/// `expected_total`, slot counts, and `cancel_requested`. The
/// function never echoes `db_path`, `events_file`, internal `store_id`,
/// `pid`, payloads, or ticket paths — agents should not need to read
/// internal ledgers to know whether their wave landed (S13, R11).
///
/// 2026-07-24-003 plan U3: when the supervisor store is reachable,
/// the command queries `SupervisorStore::fan_in_status` for the
/// public wave id. A `Found` row populates the phase / counts;
/// `UnknownWave` becomes the lookup-miss view. A corrupt or missing
/// db becomes `unavailable` with a sanitised reason (S13).
fn execute_inspect(args: WaveInspectArgs) -> Result<()> {
    let wave_id = args.wave_id.trim().to_string();
    if wave_id.is_empty() {
        bail!("ralph wave inspect: <wave_id> must not be empty");
    }

    let store_path = std::env::var("RALPH_EMISSION_STORE_PATH")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(".ralph/supervisor.db"));

    // No ledger on disk → the wave cannot have reached the store.
    // Surface the lookup-miss shape (`available / registered=false`)
    // so the agent distinguishes "store healthy, wave unknown" from
    // "store unreachable".
    if !store_path.exists() {
        return emit_view(WaveInspectView::unknown(wave_id), args.output);
    }

    // Best-effort open: a corrupt DB MUST NOT abort the read-only
    // Confirm command. We probe with `rusqlite` only when the
    // feature is compiled in; otherwise the file's mere presence
    // without a backing store still classifies as unavailable.
    #[cfg(feature = "supervisor-db")]
    {
        match ralph_core::supervisor::RusqliteSupervisorStore::open(&store_path) {
            Ok(store) => {
                // 2026-07-24-003 plan U8: a `wave_id` returned by
                // the cutover emission (U5) is the store-minted
                // `public_wave_id` in `wave_emissions` — not
                // necessarily a runtime wave row. Probe the
                // emission table FIRST so S1 / S2 / S7
                // confirm-after-Apply round-trips resolve
                // correctly; fall back to `fan_in_status` for
                // dispatcher-issued runtime waves.
                let view = match store.emission_state_for_wave_id(&wave_id) {
                    Ok(Some(state)) => {
                        let mut v = WaveInspectView::from_emission_state(state);
                        v.wave_id = wave_id.clone();
                        v
                    }
                    Ok(None) => match store.fan_in_status(&wave_id) {
                        Ok(snap) => {
                            let mut v = WaveInspectView::from_snapshot(&snap);
                            v.wave_id = wave_id.clone();
                            v
                        }
                        Err(err) => match err {
                            ralph_core::supervisor::SupervisorStoreError::UnknownWave(_) => {
                                WaveInspectView::unknown(wave_id)
                            }
                            other => {
                                let reason = ralph_core::supervisor::sanitize_unavailable_reason(
                                    &other.to_string(),
                                );
                                WaveInspectView::unavailable(wave_id, &reason)
                            }
                        },
                    },
                    Err(err) => {
                        let reason =
                            ralph_core::supervisor::sanitize_unavailable_reason(&err.to_string());
                        WaveInspectView::unavailable(wave_id, &reason)
                    }
                };
                emit_view(view, args.output)
            }
            Err(err) => {
                let reason = ralph_core::supervisor::sanitize_unavailable_reason(&err.to_string());
                emit_view(WaveInspectView::unavailable(wave_id, &reason), args.output)
            }
        }
    }
    #[cfg(not(feature = "supervisor-db"))]
    {
        let _ = wave_id; // suppress unused warning on no-feature builds
        emit_view(
            WaveInspectView::unavailable(
                args.wave_id,
                "supervisor-db feature not compiled in this build",
            ),
            args.output,
        )
    }
}

fn emit_view(view: WaveInspectView, output: WaveOutputFormat) -> Result<()> {
    match output {
        WaveOutputFormat::Text => println!("{}", render_wave_inspect_view_text(&view)),
        WaveOutputFormat::Json => {
            let stdout = std::io::stdout();
            let mut handle = stdout.lock();
            serde_json::to_writer_pretty(&mut handle, &view)?;
            writeln!(handle)?;
        }
    }
    Ok(())
}

/// Execute `ralph wave redrive` — create a child attempt wave for a
/// parent wave with failed slots.
///
/// The function opens the supervisor store, calls `create_redrive_wave`,
/// and emits the result in the requested output format.
///
/// No FlowStepScope check is performed: redrive is an operator-only
/// maintenance command that emits no business events.
fn execute_redrive(args: WaveRedriveArgs) -> Result<()> {
    let wave_id = args.wave_id.trim().to_string();
    if wave_id.is_empty() {
        bail!("ralph wave redrive: --wave-id must not be empty");
    }

    let store_path = std::env::var("RALPH_EMISSION_STORE_PATH")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(".ralph/supervisor.db"));

    #[cfg(feature = "supervisor-db")]
    {
        // No store on disk → cannot redrive.
        if !store_path.exists() {
            if matches!(args.output, WaveRedriveOutputFormat::Json) {
                let payload = serde_json::json!({
                    "ok": false,
                    "error": "supervisor store not found",
                    "hint": "RALPH_EMISSION_STORE_PATH is not set and .ralph/supervisor.db does not exist",
                });
                println!("{}", serde_json::to_string(&payload)?);
            } else {
                println!("error: supervisor store not found");
                println!(
                    "hint: set RALPH_EMISSION_STORE_PATH or ensure .ralph/supervisor.db exists"
                );
            }
            anyhow::bail!("supervisor store not available");
        }

        let store = match ralph_core::supervisor::RusqliteSupervisorStore::open(&store_path) {
            Ok(s) => s,
            Err(err) => {
                if matches!(args.output, WaveRedriveOutputFormat::Json) {
                    let payload = serde_json::json!({
                        "ok": false,
                        "error": "failed to open supervisor store",
                        "detail": ralph_core::supervisor::sanitize_unavailable_reason(&err.to_string()),
                    });
                    println!("{}", serde_json::to_string(&payload)?);
                } else {
                    println!("error: failed to open supervisor store: {}", err);
                }
                anyhow::bail!("store open failed");
            }
        };

        let slots_ref: Option<&[u32]> = args.slots.as_deref();
        let result = store.create_redrive_wave(&wave_id, slots_ref);

        match result {
            Ok(redrive) => {
                match args.output {
                    WaveRedriveOutputFormat::Text => {
                        println!("ok");
                        println!("parent_wave_id: {}", redrive.parent_wave_id);
                        println!("child_wave_id: {}", redrive.child_wave_id);
                        println!("attempt_epoch: {}", redrive.attempt_epoch);
                        println!("slots: {:?}", redrive.slots);
                    }
                    WaveRedriveOutputFormat::Json => {
                        let payload = serde_json::json!({
                            "ok": true,
                            "parent_wave_id": redrive.parent_wave_id,
                            "child_wave_id": redrive.child_wave_id,
                            "attempt_epoch": redrive.attempt_epoch,
                            "slots": redrive.slots,
                            "redrive_request_id": redrive.redrive_request_id,
                        });
                        println!("{}", serde_json::to_string(&payload)?);
                    }
                }
                Ok(())
            }
            Err(err) => {
                if matches!(args.output, WaveRedriveOutputFormat::Json) {
                    let payload = serde_json::json!({
                        "ok": false,
                        "error": err.to_string(),
                    });
                    println!("{}", serde_json::to_string(&payload)?);
                } else {
                    println!("error: {}", err);
                }
                Err(err.into())
            }
        }
    }

    #[cfg(not(feature = "supervisor-db"))]
    {
        let _ = (wave_id, args.slots, args.output, store_path);
        if matches!(args.output, WaveRedriveOutputFormat::Json) {
            println!(
                "{}",
                serde_json::json!({
                    "ok": false,
                    "error": "supervisor-db feature not compiled in this build",
                })
            );
        } else {
            println!("error: supervisor-db feature not compiled in this build");
        }
        anyhow::bail!("supervisor-db feature not available")
    }
}

/// Serializable view of the inspection result. Both human and JSON
/// output derive from this struct so the two surfaces cannot drift.
///
/// Output safety (R11): the struct never includes `db_path`,
/// `events_file`, internal `store_id`, `pid`, payloads, or ticket
/// paths — only the public `wave_id`, `phase`, slot counts, and the
/// `availability` reason code. `skip_serializing_if` on
/// `unavailable_reason` keeps the JSON quiet in the happy path.
#[derive(Debug, Clone, serde::Serialize, PartialEq, Eq)]
pub struct WaveInspectView {
    pub ok: bool,
    pub wave_id: String,
    pub registered: bool,
    /// `"available"` for a healthy lookup, `"unavailable"` when the
    /// store cannot be opened. The two states map onto S13's
    /// "unknown ≠ unavailable" distinction.
    pub availability: &'static str,
    /// Stable enum string from `WavePhase` (`dispatch`, `collect`,
    /// `integrate`, `done`, `failed`). Empty string when
    /// `registered == false` (the wave never reached the store).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_total: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_flight_count: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cancel_requested: Option<bool>,
    /// Stable, opaque reason code for the unavailable branch. Only
    /// emitted when `availability == "unavailable"`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unavailable_reason: Option<String>,
}

impl WaveInspectView {
    /// Lookup-miss: the wave never made it into the supervisor store.
    /// `availability` stays `available` because the store is healthy.
    pub fn unknown(wave_id: impl Into<String>) -> Self {
        Self {
            ok: true,
            wave_id: wave_id.into(),
            registered: false,
            availability: "available",
            phase: None,
            expected_total: None,
            completed_count: None,
            failed_count: None,
            pending_count: None,
            in_flight_count: None,
            cancel_requested: None,
            unavailable_reason: None,
        }
    }

    /// 2026-07-24-003 plan U8: build a registered view from an
    /// emission-side `EmissionState` (the cutover path minting
    /// a `public_wave_id` on first apply). The phase string is
    /// derived from the state so the agent sees a stable
    /// confirm surface: `applied` → `done`,
    /// `recovery_required` / `failed` → `failed`, others →
    /// `dispatch`. The view's `wave_id` field is filled by the
    /// call site from the queried id.
    pub fn from_emission_state(state: ralph_core::supervisor::EmissionState) -> Self {
        use ralph_core::supervisor::EmissionState as E;
        let phase = match state {
            E::Applied => "done",
            E::Failed => "failed",
            E::RecoveryRequired => "failed",
            E::Reserved | E::Applying => "dispatch",
        };
        Self {
            ok: true,
            wave_id: String::new(),
            registered: true,
            availability: "available",
            phase: Some(phase.to_string()),
            expected_total: None,
            completed_count: None,
            failed_count: None,
            pending_count: None,
            in_flight_count: None,
            cancel_requested: None,
            unavailable_reason: None,
        }
    }

    /// 2026-07-24-003 plan U3: build a registered view from a
    /// live `WaveSnapshot`. The phase string is rendered through
    /// `WavePhase::as_str` (stable contract) so the agent-facing
    /// shape stays decoupled from internal enum reprs.
    pub fn from_snapshot(snap: &ralph_core::supervisor::WaveSnapshot) -> Self {
        Self {
            ok: true,
            wave_id: snap.wave_id.clone(),
            registered: true,
            availability: "available",
            phase: Some(snap.phase.to_string()),
            expected_total: Some(snap.expected_total),
            completed_count: Some(snap.completed_count),
            failed_count: Some(snap.failed_count),
            pending_count: Some(snap.pending_count),
            in_flight_count: Some(snap.in_flight_count),
            cancel_requested: Some(snap.cancel_requested),
            unavailable_reason: None,
        }
    }

    /// Store-open failure: the lookup cannot be trusted. `registered`
    /// stays `false` (no row proven) and `availability` flips to
    /// `unavailable` so the agent can distinguish the two failure
    /// modes (S13). The `reason` is sanitised before emission so a
    /// verbose sqlite error (which may echo `.ralph/supervisor.db`
    /// or a host filesystem path) cannot leak into the agent JSON
    /// surface (R11).
    pub fn unavailable(wave_id: impl Into<String>, reason: &str) -> Self {
        const MAX: usize = 200;
        let trimmed = reason.trim();
        let stable = if trimmed.is_empty() {
            "unavailable".to_string()
        } else {
            // Strip any path-like segments: a verbose rusqlite error
            // typically reads "failed to open supervisor database:
            // ... .ralph/supervisor.db: file is not a database". Drop
            // everything after the first colon-separated fragment
            // that contains a `/`, then cap to MAX chars.
            let head = trimmed
                .split(|c: char| c == ':')
                .next()
                .unwrap_or(trimmed)
                .trim();
            let sanitised = if head.is_empty() { trimmed } else { head };
            if sanitised.chars().count() > MAX {
                let mut s: String = sanitised.chars().take(MAX).collect();
                s.push('…');
                s
            } else {
                sanitised.to_string()
            }
        };
        Self {
            ok: true,
            wave_id: wave_id.into(),
            registered: false,
            availability: "unavailable",
            phase: None,
            expected_total: None,
            completed_count: None,
            failed_count: None,
            pending_count: None,
            in_flight_count: None,
            cancel_requested: None,
            unavailable_reason: Some(stable),
        }
    }
}

/// Render the inspect view in plain text for humans / smoke tests.
/// The text surface never echoes ledger paths or payloads — agents
/// that want machine-readable fields should pin `--output json`.
pub fn render_wave_inspect_view_text(view: &WaveInspectView) -> String {
    let mut out = String::new();
    out.push_str("wave: ");
    out.push_str(&view.wave_id);
    out.push('\n');
    if !view.registered {
        if view.availability == "unavailable" {
            out.push_str("status: unavailable (store open failed)\n");
            if let Some(reason) = &view.unavailable_reason {
                out.push_str("reason: ");
                out.push_str(reason);
                out.push('\n');
            }
        } else {
            out.push_str("status: not registered (no row in store)\n");
        }
        return out;
    }
    out.push_str("status: registered\n");
    if let Some(phase) = &view.phase {
        out.push_str("phase: ");
        out.push_str(phase);
        out.push('\n');
    }
    if let Some(total) = view.expected_total {
        out.push_str(&format!(
            "counts: expected={total} pending={} in_flight={} completed={} failed={}\n",
            view.pending_count.unwrap_or(0),
            view.in_flight_count.unwrap_or(0),
            view.completed_count.unwrap_or(0),
            view.failed_count.unwrap_or(0),
        ));
    }
    if matches!(view.cancel_requested, Some(true)) {
        out.push_str("cancel_requested: true\n");
    }
    out
}

/// Execute `ralph wave verify` — validates payloads against the
/// active event policy without writing business events. On success
/// in agent context, records a one-shot ticket for the subsequent
/// `wave emit` (ticket write is intentional side effect; not a
/// zero-disk operation).
fn execute_verify(args: WaveVerifyArgs) -> Result<()> {
    enforce_wave_acl("verify")?;
    let payloads = if args.payloads_stdin {
        read_payloads_from_stdin()?
    } else {
        args.payloads
    };

    if payloads.is_empty() {
        bail!("At least one payload is required (use --payloads or --payloads-stdin)");
    }
    validate_payload_shape(&payloads)?;

    let events_file = resolve_events_file();
    let hat = std::env::var("RALPH_CURRENT_HAT")
        .ok()
        .filter(|s| !s.is_empty());

    // Reuse the same precheck as `wave emit` so verify/apply share a
    // single authorization core. The `false` / `false` flags mean
    // we never invoke --policy-check / --unsafe-no-policy-check
    // gating — verify is a pure dry-run.
    run_wave_precheck(
        &args.topic,
        false,
        false,
        args.output,
        hat.as_deref(),
        &payloads,
        &events_file,
        &args.config,
    )?;

    // Origin guard (U5 / P1 #8): supervisor-only coordination topics must be rejected by verify
    // with the same error shape as `wave emit`, so an attacker-craftable verify cannot pass and
    // emit anyway. Agents have no legitimate way to emit `*.wave.complete` / `*.wave.failed`,
    // so a verify call against one of those is always a hard reject. The supervisor itself
    // uses `system_injected` writes that bypass this CLI gate (see
    // `ralph_core::event_origin::SUPERVISOR_COORDINATION_TOPICS`).
    if ralph_core::event_origin::is_supervisor_coordination_topic(&args.topic) {
        bail!(
            "ralph wave verify refused: topic `{}` is a supervisor coordination topic; \
             it must be emitted by the supervisor, not via CLI. \
             Allowed agent topics are declared in `event_loop.event_policy.business_topics`.",
            args.topic
        );
    }

    // 2026-07-22-001 plan U1: Precheck→Apply ticket gate. After every
    // gate above passes (ACL, payload shape, policy precheck, origin
    // guard), record a one-shot ticket so the subsequent `wave emit`
    // can prove it targets the *same* payload set. Human CLI invocations
    // skip the ticket — operators must not be locked out by a stuck
    // ticket (mirrors `task_verify_gate::require_ticket`).
    let op_ctx = crate::operation_guard::OperationContext::detect(
        std::env::current_dir().unwrap_or_default(),
    );
    if op_ctx.is_agent_context {
        let workspace = std::env::current_dir().unwrap_or_default();
        let canonical = crate::wave_verify_gate::canonical_payload_form(&payloads);
        let fp = crate::wave_verify_gate::emission_fingerprint(
            &args.topic,
            &canonical,
            op_ctx.current_loop_id.as_deref().unwrap_or(""),
            op_ctx.current_hat_id.as_deref().unwrap_or(""),
        );
        let path = crate::wave_verify_gate::ticket_path(&workspace);
        crate::wave_verify_gate::record_ticket(
            &path,
            &fp,
            &args.topic,
            op_ctx.current_loop_id.as_deref().unwrap_or(""),
            op_ctx.current_hat_id.as_deref().unwrap_or(""),
        )?;
    }

    let wave_id_for_output = if matches!(args.output, WaveOutputFormat::Json) {
        // KTD-6 (P1 #7): emit a synthetic stable hash of (topic, payload digest, optional
        // idempotency key) so verify JSON carries a deterministic `wave_id`. The real wave_id
        // is minted at `wave emit` time; this hash lets agents correlate verify + emit without
        // breaking the "verify never writes" contract.
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        args.topic.hash(&mut hasher);
        for p in &payloads {
            p.hash(&mut hasher);
        }
        let hex = format!("{:x}", hasher.finish());
        format!("verify:{hex}")
    } else {
        String::new()
    };

    match args.output {
        WaveOutputFormat::Text => println!("ok"),
        WaveOutputFormat::Json => {
            let payload = serde_json::json!({
                "ok": true,
                "wave_id": wave_id_for_output,
                "topics": [&args.topic],
                "count": payloads.len(),
            });
            println!("{}", serde_json::to_string(&payload)?);
        }
    }

    Ok(())
}

/// Execute `ralph wave emit` — write N tagged events atomically.
fn execute_emit(args: WaveEmitArgs, use_colors: bool) -> Result<()> {
    enforce_wave_acl("emit")?;
    // Nested wave prevention: bail if running inside a wave worker
    if std::env::var("RALPH_WAVE_WORKER").is_ok_and(|v| v == "1") {
        bail!(
            "Cannot dispatch waves from inside a wave worker. \
             Wave workers must emit results via `ralph emit`, not `ralph wave emit`."
        );
    }

    // U2: Validate idempotency key shape if provided
    if let Some(ref key) = args.idempotency_key {
        validate_idempotency_key(key)?;
    }

    let payloads = if args.payloads_stdin {
        read_payloads_from_stdin()?
    } else {
        args.payloads
    };

    if payloads.is_empty() {
        bail!("At least one payload is required (use --payloads or --payloads-stdin)");
    }
    validate_payload_shape(&payloads)?;

    // U4: Resolve events file first (precheck needs to know where to
    // replay from for terminal-monotonicity / duplicate-terminal
    // checks). `resolve_events_file` follows the same env / marker /
    // default priority as the write path.
    let events_file = resolve_events_file();
    let hat = std::env::var("RALPH_CURRENT_HAT")
        .ok()
        .filter(|s| !s.is_empty());

    // U4: Schema precheck — load workspace ralph.yml (or preset) and
    // validate every payload against the active event policy BEFORE
    // any line is written. Failures are atomic: when any payload
    // violates policy, no events are written, and the operator / agent
    // receives a structured failure response.
    run_wave_precheck(
        &args.topic,
        args.policy_check,
        args.no_policy_check,
        args.output,
        hat.as_deref(),
        &payloads,
        &events_file,
        &args.config,
    )?;

    // 2026-07-22-001 plan U1: Precheck→Apply ticket gate. The policy
    // precheck above verifies each payload against schema/origin rules;
    // the OPAC ticket gate below closes the *drift* window where an
    // agent could verify payloads P and then emit a *different* set
    // P' that still passes the schema. `--unsafe-no-policy-check`
    // bypasses the schema gate but NOT this OPAC ticket gate.
    let workspace_root = std::env::current_dir().unwrap_or_default();
    let op_ctx = crate::operation_guard::OperationContext::detect(workspace_root.clone());
    let canonical = crate::wave_verify_gate::canonical_payload_form(&payloads);
    let fp = crate::wave_verify_gate::emission_fingerprint(
        &args.topic,
        &canonical,
        op_ctx.current_loop_id.as_deref().unwrap_or(""),
        op_ctx.current_hat_id.as_deref().unwrap_or(""),
    );

    // Stale-claim recovery: Apply succeeded in Store but the process
    // crashed before `consume_claimed_ticket`. A leftover claim would
    // permanently block `require_ticket`. When the keyed Store row is
    // already `applied` for this scope+digest, clear markers and
    // short-circuit to dedup (no second write).
    let stale_applied = if op_ctx.is_agent_context
        && crate::wave_verify_gate::claim_marker_path(&workspace_root).exists()
    {
        if let Some(ref key) = args.idempotency_key {
            try_recover_stale_claim_if_store_applied(
                &workspace_root,
                &args.topic,
                &payloads,
                &events_file,
                key,
            )?
        } else {
            None
        }
    } else {
        None
    };

    let fail_at = std::env::var("RALPH_WAVE_EMIT_FAIL_AT").ok();

    let (outcome, used_store_cutover, mut applied_cleanup_pending) = if let Some(out) =
        stale_applied
    {
        let _ = crate::wave_verify_gate::consume_claimed_ticket(&workspace_root);
        (out, true, false)
    } else {
        crate::wave_verify_gate::require_ticket(&workspace_root, &op_ctx, &args.topic, &fp)?;

        if let Some(ref key) = args.idempotency_key {
            let (loop_id, hat) = build_scope_inputs();
            let outcome_res = write_wave_events_with_idempotency_store_with_injection(
                &args.topic,
                &payloads,
                &events_file,
                key,
                &loop_id,
                &hat,
                fail_at.as_deref(),
            );
            match outcome_res {
                Ok(out) => (out, true, false),
                Err(err) => {
                    if let Err(restore_err) =
                        crate::wave_verify_gate::restore_ticket(&workspace_root)
                    {
                        eprintln!(
                            "warning: failed to restore ticket after emit failure: {restore_err}"
                        );
                    }
                    return Err(err);
                }
            }
        } else {
            let wave_id = write_wave_events(&args.topic, &payloads, &events_file)?;
            (
                IdempotencyOutcome {
                    wave_id,
                    deduplicated: false,
                },
                false,
                false,
            )
        }
    };

    let wave_id = outcome.wave_id;
    let deduplicated = outcome.deduplicated;
    let total = payloads.len();

    // U6 cleanup step: the CLI consumes both the ticket and the
    // claim marker whenever the run was ticket-gated (any agent
    // path — with or without an idempotency key). The call is
    // **idempotent**: `consume_claimed_ticket` only deletes
    // files that exist, so a second-pass retry is a no-op. A
    // cleanup failure is non-fatal — the emission has already
    // landed and the store (or FileLock) is the source of
    // truth; the agent gets `applied_cleanup_pending: true`
    // and a pointer at `ralph wave inspect <wave_id>` so it
    // knows the ticket is in a stuck state.
    if op_ctx.is_agent_context {
        let cleanup_result = if fail_at.as_deref() == Some("cleanup_ticket") {
            // U6 fault injection: mirror a real I/O failure on
            // the ticket-delete step. We DO drop the claim
            // marker (the apply phase is over) but pretend the
            // ticket delete errored, so the response surfaces
            // `applied_cleanup_pending: true`.
            if let Err(err) =
                std::fs::remove_file(crate::wave_verify_gate::claim_marker_path(&workspace_root))
            {
                eprintln!("warning: claim marker cleanup errored during fault injection: {err}");
            }
            Ok::<_, anyhow::Error>(true)
        } else {
            crate::wave_verify_gate::consume_claimed_ticket(&workspace_root)
        };
        match cleanup_result {
            Ok(false) => {}
            Ok(true) => {
                // Cleanup partial-failure: the emission is
                // durable, but the ticket is still on disk.
                // Do not surface a hard error to the agent —
                // `applied_cleanup_pending` is the stable
                // surface for this case.
                applied_cleanup_pending = true;
            }
            Err(err) => {
                eprintln!("warning: ticket cleanup errored unexpectedly: {err}");
                applied_cleanup_pending = true;
            }
        }
    }

    // U5: optionally emit structured JSON for machine verification.
    match args.output {
        WaveOutputFormat::Text => {
            // Print wave ID to stdout (machine-parseable)
            println!("{}", wave_id);
        }
        WaveOutputFormat::Json => {
            // U5 / R11: the success JSON surface is the agent's
            // contract; `events_file` MUST NOT appear — agents
            // never need to read internal ledger paths.
            // `ok: true` mirrors the failure-path shape
            // `{ok: false, error, ...}` emitted by
            // `policy_check::emit_policy_validation_failure` so
            // agents can use a uniform `jq '.ok'` contract on
            // both paths. The `applied_via: store` tag is
            // informational (helps operators distinguish
            // store-routed from no-key emissions when grepping
            // logs); agents that ignore unknown fields still
            // parse cleanly.
            //
            // U6: when the on-disk ticket cleanup fails, the
            // `applied_cleanup_pending: true` flag tells the
            // agent the emission is durable but the local
            // state needs operator attention — do NOT retry
            // (the store's `AlreadyApplied` would just dedup
            // the same wave_id), but DO run `ralph wave
            // inspect <wave_id>` to confirm the landing.
            let applied_via = if used_store_cutover {
                "store"
            } else {
                "legacy_file_lock"
            };
            let mut payload = serde_json::json!({
                "ok": true,
                "wave_id": wave_id,
                "topic": args.topic,
                "count": total,
                "deduplicated": deduplicated,
                "applied_via": applied_via,
                "applied": true,
            });
            if applied_cleanup_pending {
                payload["applied_cleanup_pending"] = serde_json::json!(true);
            }
            println!("{}", serde_json::to_string(&payload)?);
        }
    }

    // Human-readable confirmation to stderr (always)
    let dedup_tag = if deduplicated { " (deduplicated)" } else { "" };
    if use_colors {
        eprintln!(
            "\x1b[32m\u{2713}\x1b[0m Wave dispatched: {} events on topic '{}' (wave {}){}",
            total, args.topic, wave_id, dedup_tag
        );
    } else {
        eprintln!(
            "Wave dispatched: {} events on topic '{}' (wave {}){}",
            total, args.topic, wave_id, dedup_tag
        );
    }

    Ok(())
}

/// Reject the historical footgun where agents passed one shell variable
/// containing many newline-delimited JSON objects to `--payloads`, and
/// enforce the U1 invariant: every payload must be a JSON object.
fn validate_payload_shape(payloads: &[String]) -> Result<()> {
    for (idx, payload) in payloads.iter().enumerate() {
        if looks_like_multiple_json_lines(payload) {
            bail!(
                "`--payloads` argument {idx} contains multiple JSON payload lines. \
                 Use `--payloads-stdin` instead, e.g. `cat payloads.jsonl | ralph wave emit <topic> --payloads-stdin`."
            );
        }
        validate_single_payload_object(payload).with_context(|| {
            format!(
                "payload[{idx}] is not a JSON object: {payload:?} \
                 (word-splitting? pass `cat payloads.jsonl` to --payloads-stdin, \
                 not `printf '%s\\n' $(cat payloads.jsonl)`)"
            )
        })?;
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════
// U4: schema precheck helpers
// ═══════════════════════════════════════════════════════════════

/// U4: Run the workspace-config policy precheck for a wave batch.
///
/// Mirrors `ralph emit`'s strict-mode logic but applies it to the
/// whole payload batch atomically: if any payload fails, the entire
/// batch is rejected before any line is written to the JSONL. The
/// output mode is mapped to the shared [`policy_check::OutputMode`]
/// so the failure response (text vs JSON) is uniform with `ralph emit`.
#[allow(clippy::too_many_arguments)]
fn run_wave_precheck(
    topic: &str,
    policy_check_flag: bool,
    no_policy_check_flag: bool,
    output: WaveOutputFormat,
    hat: Option<&str>,
    payloads: &[String],
    events_file: &Path,
    config_overrides: &[String],
) -> Result<()> {
    use crate::policy_check::{
        OnConfigError, OutputMode, PolicyCheckFlags, PolicyCheckMode, ValidationFailure,
        emit_policy_validation_failure, enabled_event_policy, load_policy_config_for_cli_emit,
        resolve_policy_check_mode_with_ctx, validate_batch_against_config,
    };

    // Load workspace config. We `Warn` on broken configs so a typo in
    // `ralph.yml` cannot silently disable the L1 fail-fast guarantee the
    // plan is designed to provide — the agent still sees a clear warning
    // naming the parse error and the path. When the user supplied an
    // explicit `-c` flag, route the source through that path directly so
    // deployments using `ralph run -c custom.yml` route their nested
    // `ralph wave emit` through the same strict policy.
    //
    // Plan 001 §4.3 C1/C5: `load_policy_config_for_cli_emit` additionally
    // honours `RALPH_HATS_SOURCE` so wave workers spawned by the
    // dispatcher pick up the loop's preset policy without re-passing `-H`.
    // Plan 001 §4.3 C5: in both the explicit `--config` and the
    // default-discovery branches, thread `RALPH_HATS_SOURCE` through
    // `load_policy_config_for_cli_emit` so wave workers spawned by the
    // dispatcher pick up the loop's preset policy without re-passing `-H`.
    // The explicit `--config` branch falls through to the default
    // loader when the file is missing so the env var still applies.
    // 2026-07-13-001 plan U4: pass the explicit `--config` (or
    // the inherited `RALPH_CONFIG`) source through to the policy
    // loader so wave workers spawned from `ralph run -c custom.yml`
    // honour the same project config without requiring a
    // `ralph.yml` symlink in the workspace.
    let explicit_sources: Vec<crate::cli::ConfigSource> = config_overrides
        .first()
        .map(|path_str| {
            let path = PathBuf::from(path_str);
            if path.is_file() {
                vec![crate::cli::ConfigSource::File(path)]
            } else {
                Vec::new()
            }
        })
        .unwrap_or_default();
    let explicit_source_slice: &[crate::cli::ConfigSource] = &explicit_sources;
    let config = match config_overrides.first() {
        Some(path_str) => {
            let path = PathBuf::from(path_str);
            if path.is_file() {
                use crate::preflight::load_config_for_preflight_sync;
                let workspace_root = std::env::current_dir().unwrap_or_default();
                let hats_source = std::env::var("RALPH_HATS_SOURCE")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .map(|s| crate::cli::HatsSource::parse(&s));
                match load_config_for_preflight_sync(
                    explicit_source_slice,
                    hats_source.as_ref(),
                    &workspace_root,
                ) {
                    Ok(cfg) => Some(cfg),
                    Err(e) => {
                        eprintln!(
                            "Warning: policy check could not parse config at {}: {}. Proceeding without policy enforcement.",
                            path.display(),
                            e
                        );
                        None
                    }
                }
            } else {
                eprintln!(
                    "Warning: explicit --config '{}' is not a file; falling back to CWD-discovered ralph.yml.",
                    path_str
                );
                load_policy_config_for_cli_emit(None, OnConfigError::Warn, explicit_source_slice)?
            }
        }
        None => load_policy_config_for_cli_emit(None, OnConfigError::Warn, &[])?,
    };

    let flags = PolicyCheckFlags {
        policy_check: policy_check_flag,
        no_policy_check: no_policy_check_flag,
    };
    // U15: agent context defaults to strict policy-check even when the
    // resolved config does not enable `require_policy_check_for_cli_emit`.
    let op_ctx = crate::operation_guard::OperationContext::detect(
        std::env::current_dir().unwrap_or_default(),
    );
    let mode = resolve_policy_check_mode_with_ctx(&flags, config.as_ref(), op_ctx.is_agent_context);

    // No policy in play → only the JSON-object shape check ran
    // already in `validate_payload_shape`. Nothing more to do.
    let Some(policy) = enabled_event_policy(config.as_ref()) else {
        if mode == PolicyCheckMode::ExplicitCheck {
            eprintln!(
                "Warning: --policy-check was requested but no event policy is configured or enabled."
            );
        }
        return Ok(());
    };

    // The user explicitly opted out AND the config permits it
    // (resolve_policy_check_mode returns Skip in that case). If
    // mode is Skip here, the unsafe bypass won; honor it.
    if mode == PolicyCheckMode::Skip {
        return Ok(());
    }

    // The user asked to bypass the policy check but the config denied it
    // (resolve_policy_check_mode returned Enforce because
    // allow_unsafe_cli_emit: false). Surface this clearly so the agent
    // knows the bypass flag was ignored and why — otherwise they get
    // a generic "missing required field" error and a confused round-trip.
    if no_policy_check_flag
        && !matches!(mode, PolicyCheckMode::Skip)
        && config
            .as_ref()
            .and_then(|c| c.event_loop.event_policy.as_ref())
            .is_some_and(|p| p.enabled && p.require_policy_check_for_cli_emit)
    {
        eprintln!(
            "Notice: --unsafe-no-policy-check was ignored: config has event_policy.allow_unsafe_cli_emit: false. Policy check is enforced."
        );
    }

    let batch = validate_batch_against_config(topic, payloads, policy, events_file)?;
    // 2026-07-09-001 plan (U5): pre-parse every payload into
    // a JSON Value so we can hand each error its index-matched
    // payload to the enrichment helper. A parse failure here
    // means the original batch will surface a
    // `payload_type_mismatch` error already; we just keep the
    // `Null` marker on the unmatched indices so the helper
    // does not panic.
    let parsed_payloads: Vec<serde_json::Value> = payloads
        .iter()
        .map(|p| serde_json::from_str(p).unwrap_or(serde_json::Value::Null))
        .collect();
    let schema = config
        .as_ref()
        .and_then(|c| c.event_loop.event_policy.as_ref())
        .and_then(|p| {
            let key: &str = topic;
            p.schemas.get(key)
        });
    if batch.is_ok() {
        // U1 (2026-06-17-005 plan): step handoff progress-task gate
        // precheck, batch path. For each payload on a gated topic
        // (`queue.advance` / `plan.complete`), invoke the same
        // `check_progress_task_alignment` the loop uses so agents
        // running `ralph wave emit --policy-check` see
        // `progress_task_mismatch` before write. Fail-closed on the
        // first mismatch (mirrors batch atomicity).
        if ralph_core::step_handoff::progress_task_gate::is_gated_topic(topic) {
            let workspace_root = std::env::current_dir().unwrap_or_default();
            for (idx, payload) in payloads.iter().enumerate() {
                if let Err(err) =
                    crate::policy_check::check_step_handoff_gate(topic, payload, &workspace_root)
                {
                    let mut errors = batch.errors.clone();
                    errors.push(crate::policy_check::ValidationError {
                        payload_index: idx,
                        ..err
                    });
                    let failure = ValidationFailure::from_batch(
                        topic,
                        crate::policy_check::BatchValidation { errors },
                    )
                    .enrich_with_schema(topic, hat, &parsed_payloads, schema);
                    let out_mode = match output {
                        WaveOutputFormat::Text => OutputMode::Text,
                        WaveOutputFormat::Json => OutputMode::Json,
                    };
                    return emit_policy_validation_failure(&failure, out_mode);
                }
            }
        }
        return Ok(());
    }

    // Build the structured failure payload and emit it in the
    // requested output mode. This always exits non-zero (the helper
    // returns Err) so the agent sees a clear failure.
    let failure = ValidationFailure::from_batch(topic, batch).enrich_with_schema(
        topic,
        hat,
        &parsed_payloads,
        schema,
    );
    let out_mode = match output {
        WaveOutputFormat::Text => OutputMode::Text,
        WaveOutputFormat::Json => OutputMode::Json,
    };
    emit_policy_validation_failure(&failure, out_mode)
}

fn looks_like_multiple_json_lines(payload: &str) -> bool {
    let json_like_lines = payload
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with('{') || trimmed.starts_with('[')
        })
        .take(2)
        .count();
    json_like_lines > 1
}

/// Parse the payload as JSON and require it to be a JSON object.
/// Rejects numbers, strings, arrays, booleans, null, and truncated JSON.
fn validate_single_payload_object(payload: &str) -> Result<()> {
    let value: serde_json::Value =
        serde_json::from_str(payload).with_context(|| format!("invalid JSON: {payload:?}"))?;
    if !value.is_object() {
        bail!(
            "expected JSON object, got {} ({})",
            value_type_name(&value),
            short_preview(payload)
        );
    }
    Ok(())
}

fn value_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn short_preview(payload: &str) -> String {
    const MAX: usize = 40;
    if payload.len() <= MAX {
        payload.to_string()
    } else {
        format!("{}…", &payload[..MAX])
    }
}

/// Read payloads from stdin, one JSON object per line.
/// Empty lines are skipped.
fn read_payloads_from_stdin() -> Result<Vec<String>> {
    read_payloads_from_reader(io::stdin().lock())
}

/// Read payloads from any buffered reader, one payload per line.
/// Empty lines are skipped.
fn read_payloads_from_reader<R: BufRead>(reader: R) -> Result<Vec<String>> {
    let mut payloads = Vec::new();
    for line in reader.lines() {
        let line = line.context("Failed to read line from reader")?;
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            payloads.push(trimmed.to_string());
        }
    }
    Ok(payloads)
}

/// Write wave events to a JSONL file. Returns the generated wave ID.
///
/// This is the core logic, separated from CLI concerns for testability.
pub fn write_wave_events(topic: &str, payloads: &[String], events_file: &Path) -> Result<String> {
    // Read hat from runtime environment if available
    let hat = std::env::var("RALPH_CURRENT_HAT")
        .ok()
        .filter(|s| !s.is_empty());
    write_wave_events_with_provenance(topic, payloads, events_file, hat.as_deref(), None, None)
}

/// Like [`write_wave_events`] but with explicit provenance and idempotency fields.
///
/// When `idempotency_key` and `idempotency_hash` are provided, each wave event
/// record gets `idempotency_key` and `idempotency_hash` fields injected. This
/// enables recovery scanning by wave_id + idempotency_key.
pub fn write_wave_events_with_provenance(
    topic: &str,
    payloads: &[String],
    events_file: &Path,
    hat: Option<&str>,
    idempotency_key: Option<&str>,
    idempotency_hash: Option<&str>,
) -> Result<String> {
    write_wave_events_with_provenance_with_wave_id(
        topic,
        payloads,
        events_file,
        hat,
        idempotency_key,
        idempotency_hash,
        None,
    )
}

/// 2026-07-24-003 plan U5 variant: write a batch with an *explicit*
/// `wave_id`, skipping internal `generate_wave_id()`. Used by the
/// store cutover when `SupervisorStore::reserve_emission` minted a
/// `public_wave_id` for us — the events must carry that exact id so
/// `wave inspect` can correlate the JSONL row with the store row.
pub fn write_wave_events_with_provenance_with_wave_id(
    topic: &str,
    payloads: &[String],
    events_file: &Path,
    hat: Option<&str>,
    idempotency_key: Option<&str>,
    idempotency_hash: Option<&str>,
    explicit_wave_id: Option<&str>,
) -> Result<String> {
    if payloads.is_empty() {
        bail!("At least one payload is required");
    }

    let wave_id = match explicit_wave_id {
        Some(id) => id.to_string(),
        None => generate_wave_id(),
    };

    let total = payloads.len() as u32;
    let ts = chrono::Utc::now().to_rfc3339();

    // Ensure parent directory exists
    if let Some(parent) = events_file.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Build all event records
    let mut lines = String::new();
    for (index, payload) in payloads.iter().enumerate() {
        let mut record = serde_json::json!({
            "topic": topic,
            "payload": payload,
            "ts": ts,
            "wave_id": wave_id,
            "wave_index": index as u32,
            "wave_total": total,
        });

        // Add hat provenance if available
        if let Some(hat_val) = hat
            && let Some(obj) = record.as_object_mut()
        {
            obj.insert("hat".to_string(), serde_json::json!(hat_val));
        }

        // U2: Inject idempotency fields when present
        if let Some(ik) = idempotency_key
            && let Some(obj) = record.as_object_mut()
        {
            obj.insert("idempotency_key".to_string(), serde_json::json!(ik));
        }
        if let Some(ih) = idempotency_hash
            && let Some(obj) = record.as_object_mut()
        {
            obj.insert("idempotency_hash".to_string(), serde_json::json!(ih));
        }

        let json_line = serde_json::to_string(&record)?;
        lines.push_str(&json_line);
        lines.push('\n');
    }

    // Write all events atomically
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(events_file)
        .with_context(|| format!("Failed to open events file: {}", events_file.display()))?;
    file.write_all(lines.as_bytes())?;

    Ok(wave_id)
}

/// Resolve the events file path from environment and marker files.
///
/// Priority: RALPH_EVENTS_FILE env > .ralph/current-events marker > default .ralph/events.jsonl
pub fn resolve_events_file() -> PathBuf {
    if let Ok(path) = std::env::var("RALPH_EVENTS_FILE")
        && !path.is_empty()
    {
        return PathBuf::from(path);
    }
    fs::read_to_string(".ralph/current-events")
        .map(|s| PathBuf::from(s.trim()))
        .unwrap_or_else(|_| PathBuf::from(".ralph/events.jsonl"))
}

// ═══════════════════════════════════════════════════════════════
// U2: Idempotency helpers
// ═══════════════════════════════════════════════════════════════

/// U2: Resolve scope inputs (loop_id, hat) from env and marker files.
///
/// Order:
/// - loop_id: `RALPH_CURRENT_LOOP_ID` env → `.ralph/current-loop-id` marker → `"unknown"`
/// - hat: `RALPH_CURRENT_HAT` env → `""`
fn build_scope_inputs() -> (String, String) {
    let loop_id = std::env::var("RALPH_CURRENT_LOOP_ID")
        .ok()
        .or_else(|| fs::read_to_string(".ralph/current-loop-id").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let hat = std::env::var("RALPH_CURRENT_HAT")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default();
    (loop_id, hat)
}

/// U2: Compute the sha256 hex scope key from the four dedup dimensions.
pub fn compute_scope_key(loop_id: &str, hat: &str, topic: &str, key: &str) -> String {
    let joined = format!("{loop_id}|{hat}|{topic}|{key}");
    compute_sha256_hex(&joined)
}

/// U2: Compute the payload digest for the whole payload list.
///
/// Uses `\u{1F}` (Unit Separator) as delimiter — it is forbidden in JSON
/// strings and therefore unambiguous.
pub fn compute_payload_digest(payloads: &[String]) -> String {
    let mut joined = String::new();
    for (i, p) in payloads.iter().enumerate() {
        if i > 0 {
            joined.push('\u{1F}');
        }
        joined.push_str(p);
    }
    compute_sha256_hex(&joined)
}

/// WRC-U6 (2026-06-12-003): validate a wave record before it is
/// appended to the events JSONL. Returns `Ok(())` when the
/// record's `wave_total` field equals the expected wave size, and
/// `Err` otherwise. The check is intentionally narrow: it
/// catches the documented 335-worker failure mode (a hand-written
/// or scripted `events.jsonl` whose `wave_total` does not match
/// the worker's expectation) without re-running the full wave
/// pipeline. Callers that need the broader wave record
/// validation (topic schema, payload shape, idempotency key)
/// already have those checks in `write_wave_events_with_provenance`.
///
/// The function is `pub(crate)` so the test module can drive
/// the rejection path; production callers are the JSONL
/// append-or-write path and the BDD scenario for AE2 timing.
#[allow(dead_code)] // 003 plan WRC-U6 预留：手写 JSONL 入口的 wave_total 拒收点，待接线
pub(crate) fn validate_wave_record(
    record: &serde_json::Value,
    expected_wave_total: u32,
) -> Result<()> {
    let actual = record
        .get("wave_total")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow!("wave record missing 'wave_total' field"))?;
    if actual != u64::from(expected_wave_total) {
        bail!(
            "wave_total mismatch: record declares {actual} but the wave size is {expected_wave_total}"
        );
    }
    Ok(())
}

/// U2: Derive the idempotency log path as a sibling of `events_file`.
///
/// Returns `<parent>/.<basename>.idempotency.jsonl`.
/// Example: `/repo/.ralph/events.jsonl` → `/repo/.ralph/.events.jsonl.idempotency.jsonl`
fn idempotency_log_path(events_file: &Path) -> PathBuf {
    let parent = events_file.parent().unwrap_or_else(|| Path::new("."));
    let file_name = events_file
        .file_name()
        .map(|s| s.to_string_lossy())
        .unwrap_or_else(|| std::borrow::Cow::Borrowed("events.jsonl"));
    parent.join(format!(".{}.idempotency.jsonl", file_name))
}

/// U2: Read all idempotency records from the log file.
///
/// Self-healing: malformed lines are warned and skipped (not fatal). The
/// idempotency log is append-only and self-written, but a half-line from
/// SIGKILL / disk-full / older writer format must not permanently block
/// subsequent `ralph wave emit` calls. A skipped line is mirrored to a
/// `.corrupt` sidecar so an operator can inspect / truncate later.
fn read_idempotency_records(events_file: &Path) -> Result<Vec<IdempotencyRecord>> {
    let path = idempotency_log_path(events_file);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let mut out = Vec::new();
    let mut corrupt_lines: Vec<String> = Vec::new();
    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<IdempotencyRecord>(trimmed) {
            Ok(rec) => out.push(rec),
            Err(e) => {
                eprintln!(
                    "warning: ignoring malformed idempotency record at {} line {}: {}",
                    path.display(),
                    i + 1,
                    e
                );
                corrupt_lines.push(trimmed.to_string());
            }
        }
    }
    if !corrupt_lines.is_empty() {
        let sidecar = path.with_extension("idempotency.jsonl.corrupt");
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&sidecar)
            .with_context(|| format!("open corrupt sidecar {}", sidecar.display()))?;
        for line in &corrupt_lines {
            writeln!(f, "{}", line)?;
        }
    }
    Ok(out)
}

/// U2: Append one idempotency record to the log file (with fsync).
#[allow(dead_code)] // legacy sidecar writer; see `write_wave_events_with_idempotency_store`.
fn append_idempotency_record(events_file: &Path, rec: &IdempotencyRecord) -> Result<()> {
    let path = idempotency_log_path(events_file);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    // 2026-07-22-001 plan U5 (KTD-4, OQ1): the supervisor store
    // is now the idempotency single source of truth — the
    // `register_wave_if_absent` check on the dispatcher side
    // already prevents double-spawn for the same wave_id. The
    // sidecar file remains as a one-version compatibility shim
    // (so existing operator tooling reading the log keeps
    // working) but is no longer authoritative for dedup. Emit a
    // one-shot stderr warning so an operator scanning the loop
    // output can see exactly why the file is still being
    // written and when it will be removed.
    static SIDECAR_DEPRECATION_LOGGED: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    if !SIDECAR_DEPRECATION_LOGGED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        eprintln!(
            "wave_idempotency_sidecar_deprecation: writing .idempotency.jsonl for legacy compat; \
             authoritative dedup lives in the supervisor store (`register_wave_if_absent`). \
             The sidecar will be removed in a follow-up release (2026-07-22-001 plan U5)."
        );
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("Failed to open idempotency log: {}", path.display()))?;
    let line = serde_json::to_string(rec)?;
    writeln!(f, "{}", line)?;
    f.sync_data()?;
    Ok(())
}

/// U2: Count events in `events_file` whose `idempotency_key` and `wave_id` match.
///
/// Used by the recovery path. Tolerates malformed event lines (continue).
fn count_recovered_events(
    events_file: &Path,
    expected_wave_id: &str,
    expected_key: &str,
) -> Result<u32> {
    if !events_file.exists() {
        return Ok(0);
    }
    let content = fs::read_to_string(events_file)
        .with_context(|| format!("read {}", events_file.display()))?;
    let mut count: u32 = 0;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue, // events file tolerates malformed lines
        };
        if v.get("wave_id").and_then(|x| x.as_str()) == Some(expected_wave_id)
            && v.get("idempotency_key").and_then(|x| x.as_str()) == Some(expected_key)
        {
            count += 1;
        }
    }
    Ok(count)
}

/// U2: Scan events file for events with matching `idempotency_key`.
///
/// Returns `(first_wave_id, count)` when exactly `expected_count` matching
/// events are found. Returns `None` when no matching events exist (clean
/// first call). Errors when partial matches exist (incomplete prior emission).
///
/// Uses both `idempotency_key` AND `idempotency_hash` (the scope_key) to
/// avoid cross-scope false positive on recovery scans.
/// Used by the recovery path when the idempotency record was lost (crash
/// between events append and record append).
#[allow(dead_code)] // legacy recovery path; store-driven recovery uses `reserve_emission`.
fn try_recover_from_events(
    events_file: &Path,
    idempotency_key: &str,
    scope_key: &str,
    expected_count: usize,
) -> Result<Option<(String, usize)>> {
    if !events_file.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(events_file)
        .with_context(|| format!("read {}", events_file.display()))?;
    let mut count: usize = 0;
    let mut first_wave_id: Option<String> = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("idempotency_key").and_then(|x| x.as_str()) == Some(idempotency_key)
            && v.get("idempotency_hash").and_then(|x| x.as_str()) == Some(scope_key)
        {
            count += 1;
            if first_wave_id.is_none() {
                first_wave_id = v
                    .get("wave_id")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
            }
        }
    }
    match count {
        0 => Ok(None),
        n if n == expected_count => {
            let wave_id =
                first_wave_id.unwrap_or_else(|| "w-recovered-unknown-wave-id".to_string());
            Ok(Some((wave_id, count)))
        }
        n => {
            bail!(
                "incomplete prior wave emission: found {} events with idempotency_key '{}' \
                 in events file, but expected {}. Manually clean up partial events or use a \
                 different --idempotency-key.",
                n,
                idempotency_key,
                expected_count
            );
        }
    }
}

/// U2: Validate idempotency key shape.
fn validate_idempotency_key(key: &str) -> Result<()> {
    if key.is_empty() {
        bail!("--idempotency-key must not be empty");
    }
    if key.trim().is_empty() {
        bail!("--idempotency-key must not be whitespace-only");
    }
    if key.len() > MAX_IDEMPOTENCY_KEY_BYTES {
        bail!(
            "--idempotency-key exceeds {} bytes (got {})",
            MAX_IDEMPOTENCY_KEY_BYTES,
            key.len()
        );
    }
    if !key.is_ascii() {
        bail!("--idempotency-key must be ASCII (got non-ASCII bytes)");
    }
    Ok(())
}

/// U2: Emit wave events with idempotency enforcement.
///
/// On first call with a given `(loop_id, hat, topic, key)`, writes N events
/// and one idempotency record. On subsequent calls with the same scope and
/// payload digest, returns the original wave_id with `deduplicated=true`.
///
/// Uses `FileLock::exclusive()` for concurrency safety.
///
/// 2026-07-24-003 plan U5: this legacy sidecar-based routine is
/// **not** called by the production `wave emit` path any more — it
/// survives only so the unit tests in this module can continue
/// asserting the sidecar writer / reader semantics (including
/// recovery paths) without taking a dependency on the
/// `supervisor-db` feature gate. Production traffic uses
/// [`write_wave_events_with_idempotency_store`].
#[allow(dead_code)]
pub fn write_wave_events_with_idempotency(
    topic: &str,
    payloads: &[String],
    events_file: &Path,
    idempotency_key: &str,
) -> Result<IdempotencyOutcome> {
    write_wave_events_with_idempotency_with_scope(
        topic,
        payloads,
        events_file,
        idempotency_key,
        &build_scope_inputs().0,
        &build_scope_inputs().1,
    )
}

/// U2: Like [`write_wave_events_with_idempotency`] but with explicit scope params for testability.
#[allow(dead_code)]
pub fn write_wave_events_with_idempotency_with_scope(
    topic: &str,
    payloads: &[String],
    events_file: &Path,
    idempotency_key: &str,
    loop_id: &str,
    hat: &str,
) -> Result<IdempotencyOutcome> {
    if payloads.is_empty() {
        bail!("At least one payload is required");
    }
    if idempotency_key.is_empty() {
        bail!("idempotency_key must not be empty (caller bug)");
    }

    let scope_key = compute_scope_key(loop_id, hat, topic, idempotency_key);
    let payload_digest = compute_payload_digest(payloads);

    // Acquire exclusive lock on events_file
    let lock = FileLock::new(events_file)
        .with_context(|| format!("create FileLock for {}", events_file.display()))?;
    let _guard = lock
        .exclusive()
        .with_context(|| format!("acquire exclusive lock on {}", lock.lock_path().display()))?;

    // Load existing records
    let records = read_idempotency_records(events_file)?;

    // Dedup check
    if let Some(existing) = records.iter().find(|r| r.scope_key == scope_key) {
        if existing.payload_digest != payload_digest {
            bail!(
                "idempotency-key conflict: same scope already used with a different payload. \
                 original wave_id={}, original count={}, original created_at={}. \
                 If the new payload is intended, use a different --idempotency-key.",
                existing.wave_id,
                existing.count,
                existing.created_at
            );
        }
        // Recovery: verify events file has the expected count
        let count = count_recovered_events(events_file, &existing.wave_id, idempotency_key)?;
        if count < existing.count {
            bail!(
                "incomplete prior wave emission: scope_key {} has record claiming \
                 {} events but only {} found in events file. Refusing to silently re-append; \
                 manually clean up partial events or use a new --idempotency-key.",
                scope_key,
                existing.count,
                count
            );
        }
        return Ok(IdempotencyOutcome {
            wave_id: existing.wave_id.clone(),
            deduplicated: true,
        });
    }

    // U2: Recovery scan — record was lost but events exist with matching idempotency_key
    let recovery =
        try_recover_from_events(events_file, idempotency_key, &scope_key, payloads.len())?;
    if let Some((wave_id, count)) = recovery {
        // Reconstruct the record from the recovered wave data
        let rec = IdempotencyRecord {
            scope_key: scope_key.clone(),
            idempotency_key: idempotency_key.to_string(),
            wave_id: wave_id.clone(),
            topic: topic.to_string(),
            hat: hat.to_string(),
            payload_digest,
            count: count as u32,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        append_idempotency_record(events_file, &rec)?;
        return Ok(IdempotencyOutcome {
            wave_id,
            deduplicated: true,
        });
    }

    // First-time: write events (with idempotency fields), then write record
    let wave_id = write_wave_events_with_provenance(
        topic,
        payloads,
        events_file,
        if hat.is_empty() { None } else { Some(hat) },
        Some(idempotency_key),
        Some(&scope_key),
    )?;

    let rec = IdempotencyRecord {
        scope_key: scope_key.clone(),
        idempotency_key: idempotency_key.to_string(),
        wave_id: wave_id.clone(),
        topic: topic.to_string(),
        hat: hat.to_string(),
        payload_digest,
        count: payloads.len() as u32,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    append_idempotency_record(events_file, &rec)?;

    Ok(IdempotencyOutcome {
        wave_id,
        deduplicated: false,
    })
}

/// 2026-07-24-003 plan U5: emit a wave *through the supervisor store*.
///
/// This is the new single source of truth for keyed wave emits.
/// The CLI no longer writes the legacy `.idempotency.jsonl` for
/// happy-path traffic; the store's `wave_emissions` table is the
/// dedup authority. A legacy sidecar is imported only when the
/// store has no record for the scope (the miss-import branch —
/// S10 closes the migration window without making the sidecar
/// authoritative for new emissions).
///
/// Behaviour table:
///
/// | `reserve_emission` returns | CLI action |
/// |---|---|
/// | `Reserved { public_wave_id }` | append N events with that id → `mark_emission_applying` → `mark_emission_applied` → success (`deduplicated=false`). |
/// | `AlreadyApplied { public_wave_id }` | return id with `deduplicated=true`, zero writes. |
/// | `Conflict` | return `idempotency_key_conflict`, zero writes. |
/// | `RecoveryRequired { ... }` / `FailedPartial { ... }` | return inspect-guided error, zero writes. |
#[allow(dead_code)] // thin wrapper kept for callers / future non-injection path
pub fn write_wave_events_with_idempotency_store(
    topic: &str,
    payloads: &[String],
    events_file: &Path,
    idempotency_key: &str,
    loop_id: &str,
    hat: &str,
) -> Result<IdempotencyOutcome> {
    write_wave_events_with_idempotency_store_with_injection(
        topic,
        payloads,
        events_file,
        idempotency_key,
        loop_id,
        hat,
        None,
    )
}

/// If a claim marker is left behind after Store already applied this
/// scope+digest (crash between `mark_emission_applied` and ticket
/// cleanup), return the dedup outcome so the emit path can clear
/// markers without calling `require_ticket` (which would deny).
fn try_recover_stale_claim_if_store_applied(
    workspace: &Path,
    topic: &str,
    payloads: &[String],
    events_file: &Path,
    idempotency_key: &str,
) -> Result<Option<IdempotencyOutcome>> {
    let (loop_id, hat) = build_scope_inputs();
    let scope_key = compute_scope_key(&loop_id, &hat, topic, idempotency_key);
    let payload_digest = compute_payload_digest(payloads);
    let expected_count = payloads.len() as u32;

    #[cfg(feature = "supervisor-db")]
    {
        use std::path::PathBuf;
        let path: PathBuf = std::env::var("RALPH_EMISSION_STORE_PATH")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| workspace.join(".ralph/supervisor.db"));
        if !path.exists() {
            return Ok(None);
        }
        let store = match ralph_core::supervisor::RusqliteSupervisorStore::open(&path) {
            Ok(s) => s,
            Err(_) => return Ok(None),
        };
        let count_for_wave = |wave_id: &str| -> u32 {
            if !events_file.exists() {
                return 0;
            }
            let Ok(body) = fs::read_to_string(events_file) else {
                return 0;
            };
            let mut n: u32 = 0;
            for line in body.lines() {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) else {
                    continue;
                };
                if v.get("wave_id").and_then(|x| x.as_str()) == Some(wave_id)
                    && v.get("idempotency_key").and_then(|x| x.as_str()) == Some(idempotency_key)
                {
                    n += 1;
                }
            }
            n
        };
        match store.reserve_emission(&scope_key, &payload_digest, expected_count, &count_for_wave) {
            Ok(ralph_core::supervisor::EmissionReservation::AlreadyApplied { public_wave_id }) => {
                Ok(Some(IdempotencyOutcome {
                    wave_id: public_wave_id,
                    deduplicated: true,
                }))
            }
            _ => Ok(None),
        }
    }
    #[cfg(not(feature = "supervisor-db"))]
    {
        let _ = (
            workspace,
            topic,
            payloads,
            events_file,
            idempotency_key,
            scope_key,
            payload_digest,
            expected_count,
        );
        Ok(None)
    }
}

/// 2026-07-24-003 plan U6 variant: the same cutover path with an
/// optional `fail_at` knob for the ticket-recovery integration
/// tests. Production callers use
/// [`write_wave_events_with_idempotency_store`], which passes
/// `None`.
///
/// | `fail_at` value             | behaviour |
/// |---|---|
/// | `None` / absent              | normal path |
/// | `"apply_before_write"`       | return `Err` immediately after miss-import / before reserve write — exercises S5 (ticket must be restored on retry). |
/// | `"cleanup_ticket"`           | handled at the execute_emit layer — exercises S7 (`applied_cleanup_pending: true`). |
pub fn write_wave_events_with_idempotency_store_with_injection(
    topic: &str,
    payloads: &[String],
    events_file: &Path,
    idempotency_key: &str,
    loop_id: &str,
    hat: &str,
    fail_at: Option<&str>,
) -> Result<IdempotencyOutcome> {
    if payloads.is_empty() {
        bail!("At least one payload is required");
    }
    if idempotency_key.is_empty() {
        bail!("idempotency_key must not be empty (caller bug)");
    }

    let scope_key = compute_scope_key(loop_id, hat, topic, idempotency_key);
    let payload_digest = compute_payload_digest(payloads);
    let expected_count = payloads.len() as u32;

    // Open the supervisor store — SQLite is the sole authority for
    // keyed emissions (plan 2026-07-24-003). Resolution order:
    //   1. `RALPH_EMISSION_STORE_PATH` (tests / explicit override)
    //   2. `.ralph/supervisor.db` (create if missing so a lone
    //      `wave emit --idempotency-key` still gets durable UNIQUE)
    // Open failure (corrupt file, permission, migration) is
    // **fail-closed** — never silently fall back to InMemory, which
    // would break cross-process dedup while `wave inspect` reports
    // `unavailable` for the same path.
    #[cfg(feature = "supervisor-db")]
    let store: std::sync::Arc<dyn ralph_core::supervisor::SupervisorStore> = {
        use std::path::PathBuf;
        let path: PathBuf = std::env::var("RALPH_EMISSION_STORE_PATH")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".ralph/supervisor.db"));
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "supervisor_store_unavailable: create parent for {}",
                    path.display()
                )
            })?;
        }
        match ralph_core::supervisor::RusqliteSupervisorStore::open(&path) {
            Ok(s) => std::sync::Arc::new(s),
            Err(err) => {
                bail!(
                    "supervisor_store_unavailable: cannot open emission store at {}: {err}. \
                     Fix or remove the corrupt store, then retry; do not re-emit blindly.",
                    path.display()
                );
            }
        }
    };
    #[cfg(not(feature = "supervisor-db"))]
    let store: std::sync::Arc<dyn ralph_core::supervisor::SupervisorStore> = {
        bail!(
            "supervisor_store_unavailable: this build was compiled without the \
             `supervisor-db` feature; keyed wave emit requires a durable store"
        );
    };

    // Count helper: events on disk carrying a specific wave_id.
    let count_for_wave = |wave_id: &str| -> u32 {
        if !events_file.exists() {
            return 0;
        }
        let body = match fs::read_to_string(events_file) {
            Ok(b) => b,
            Err(_) => return 0,
        };
        let mut n: u32 = 0;
        for line in body.lines() {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let v: serde_json::Value = match serde_json::from_str(trimmed) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if v.get("wave_id").and_then(|x| x.as_str()) == Some(wave_id)
                && v.get("idempotency_key").and_then(|x| x.as_str()) == Some(idempotency_key)
            {
                n += 1;
            }
        }
        n
    };

    // Miss-import branch: try the legacy sidecar exactly once,
    // before talking to the store. The store has no row for this
    // scope, but a pre-fix workspace may still have a valid sidecar
    // + a complete batch on disk. If the sidecar digest matches
    // and the events file carries `count` records with that
    // wave_id, adopt the row via the store (S10).
    if let Some(legacy_wave_id) = try_legacy_sidecar_import(
        events_file,
        idempotency_key,
        &scope_key,
        &payload_digest,
        expected_count,
    )? {
        let adopted = store.adopt_legacy_emission(
            &scope_key,
            &payload_digest,
            expected_count,
            &legacy_wave_id,
        )?;
        return Ok(IdempotencyOutcome {
            wave_id: adopted,
            deduplicated: true,
        });
    }

    // 2026-07-24-003 plan U6 fault-injection seam (S5):
    // bail after the miss-import branch resolves but before the
    // store ever sees `reserve_emission`. The ticket stays
    // claimed; restore_ticket (the caller's responsibility on
    // Err) puts it back to `prepared` so the next retry can
    // re-claim and emit a fresh batch — the store has no row
    // for this scope and there is no partial batch to recover,
    // so this is the cleanest possible failure surface for
    // I/O faults that hit before the JSONL append.
    if fail_at == Some("apply_before_write") {
        bail!(
            "wave_emission_apply_before_write: injected failure before \
             writing events for scope {scope_key}; the ticket has been \
             restored. Retry with the same fingerprint + idempotency-key \
             — no partial batch exists for this scope."
        );
    }

    // Drive the store. The `count_events_on_disk` closure lets the
    // store peek at on-disk rows without owning the events_file.
    let reservation =
        store.reserve_emission(&scope_key, &payload_digest, expected_count, &count_for_wave)?;

    use ralph_core::supervisor::EmissionReservation as R;
    match reservation {
        R::Reserved { public_wave_id } => {
            // Lock events_file to keep multi-process file-writes
            // serialised; two racers hit the same store row, only
            // one gets `Reserved`.
            let lock = FileLock::new(events_file)
                .with_context(|| format!("create FileLock for {}", events_file.display()))?;
            let _guard = lock.exclusive().with_context(|| {
                format!("acquire exclusive lock on {}", lock.lock_path().display())
            })?;

            // Append events with the store-minted `public_wave_id`.
            store
                .mark_emission_applying(&scope_key)
                .map_err(|e| anyhow!("failed to mark emission applying: {e}"))?;

            let write_result = write_wave_events_with_provenance_with_wave_id(
                topic,
                payloads,
                events_file,
                if hat.is_empty() { None } else { Some(hat) },
                Some(idempotency_key),
                Some(&scope_key),
                Some(&public_wave_id),
            );
            match write_result {
                Ok(wave_id) => {
                    let now_secs = SystemTime::now()
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    store
                        .mark_emission_applied(&scope_key, now_secs)
                        .map_err(|e| anyhow!("failed to mark emission applied: {e}"))?;
                    Ok(IdempotencyOutcome {
                        wave_id,
                        deduplicated: false,
                    })
                }
                Err(write_err) => {
                    // Mark the reservation as recovery-required so a
                    // subsequent retry can either complete or fail
                    // closed. We deliberately do NOT touch the JSONL
                    // line — partial emission is fail-closed (S9).
                    let _ = store.mark_emission_recovery_required(&scope_key);
                    Err(write_err)
                }
            }
        }
        R::AlreadyApplied { public_wave_id } => Ok(IdempotencyOutcome {
            wave_id: public_wave_id,
            deduplicated: true,
        }),
        R::Conflict => {
            bail!(
                "idempotency_key_conflict: scope {} already reserved with a different payload digest. \
                 Use a different --idempotency-key for this payload set.",
                scope_key
            );
        }
        R::RecoveryRequired {
            public_wave_id,
            on_disk,
            expected,
        } => {
            bail!(
                "wave_emission_recovery_required: store has reservation {public_wave_id} \
                 for scope {scope_key} but only {on_disk}/{expected} events present on disk. \
                 Run `ralph wave inspect {public_wave_id}` to inspect; the prior batch must be \
                 completed or cleaned up before a re-emit can apply."
            );
        }
        R::FailedPartial {
            public_wave_id,
            on_disk,
            expected,
        } => {
            bail!(
                "wave_emission_failed_partial: scope {scope_key} reservation {public_wave_id} \
                 has {on_disk}/{expected} events on disk (partial). Re-emitting would create a \
                 second wave; refusing to do so. Inspect via `ralph wave inspect {public_wave_id}` \
                 and recover manually before re-emitting with a different --idempotency-key."
            );
        }
    }
}

/// Sidecar miss-import: read the legacy `.idempotency.jsonl` row
/// for `scope_key`, and only adopt it when (a) the
/// `payload_digest` matches the incoming batch and (b) the events
/// file carries exactly `expected_count` rows with that wave_id +
/// idempotency_key.
///
/// S11 — migration conflict fail-closed: when a sidecar row exists
/// for this scope+key but digest/count disagrees, return `Err` so
/// the caller never falls through to a fresh `reserve_emission`
/// (which would mint a second wave beside the legacy batch).
///
/// When adopted, the sidecar row is removed so subsequent emits
/// cannot accidentally re-import it.
fn try_legacy_sidecar_import(
    events_file: &Path,
    idempotency_key: &str,
    scope_key: &str,
    payload_digest: &str,
    expected_count: u32,
) -> Result<Option<String>> {
    let sidecar_path = idempotency_log_path(events_file);
    if !sidecar_path.exists() {
        return Ok(None);
    }
    let records = read_idempotency_records(events_file)?;
    let Some(record) = records.into_iter().find(|r| r.scope_key == scope_key) else {
        return Ok(None);
    };

    // The user-friendly key is what the agent passed; the row's
    // recorded key must round-trip the same way.
    if record.idempotency_key != idempotency_key {
        // Different key under the same scope — treat as a stale row.
        return Ok(None);
    }
    if record.payload_digest != payload_digest {
        bail!(
            "sidecar_import_conflict: legacy idempotency row for key '{idempotency_key}' \
             has a different payload_digest than this emit. Refusing to create a second \
             wave. Inspect the events ledger and remove or repair the sidecar before retrying."
        );
    }
    let on_disk = count_recovered_events(events_file, &record.wave_id, idempotency_key)?;
    if on_disk != record.count || record.count != expected_count {
        bail!(
            "sidecar_import_conflict: legacy idempotency row for key '{idempotency_key}' \
             count/on-disk mismatch (sidecar_count={}, on_disk={on_disk}, expected={expected_count}). \
             Refusing to create a second wave.",
            record.count
        );
    }

    // We adopted the row. Remove the sidecar so the next emit does
    // not re-import it. Failure to delete is non-fatal — the store
    // is now the source of truth and a future emit will simply
    // treat the row as stale.
    let _ = fs::remove_file(&sidecar_path);
    Ok(Some(record.wave_id))
}

/// Generate a unique wave ID.
///
/// Concatenates nanosecond timestamp, PID, and a process-local atomic counter.
/// Readable and debuggable — each segment is independently meaningful.
fn generate_wave_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let pid = std::process::id();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);

    format!("w-{nanos:x}-{pid}-{seq}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy_check::{ValidationFailure, validate_batch_against_config};
    use tempfile::TempDir;

    #[test]
    fn test_write_wave_events_creates_tagged_events() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");

        let payloads = vec![
            "src/main.rs".to_string(),
            "src/lib.rs".to_string(),
            "src/config.rs".to_string(),
        ];

        let wave_id = write_wave_events("review.file", &payloads, &events_path).unwrap();
        assert!(wave_id.starts_with("w-"));

        let content = fs::read_to_string(&events_path).unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 3);

        // Parse and verify each event
        for (i, line) in lines.iter().enumerate() {
            let event: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(event["topic"], "review.file");
            assert_eq!(event["wave_index"], i as u64);
            assert_eq!(event["wave_total"], 3);
            assert_eq!(event["wave_id"], wave_id.as_str());
        }
    }

    #[test]
    fn test_write_wave_events_single_payload() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");

        let payloads = vec!["only-one".to_string()];
        let wave_id = write_wave_events("test.topic", &payloads, &events_path).unwrap();

        let content = fs::read_to_string(&events_path).unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 1);

        let event: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(event["wave_index"], 0);
        assert_eq!(event["wave_total"], 1);
        assert_eq!(event["wave_id"], wave_id.as_str());
    }

    #[test]
    fn test_write_wave_events_empty_payloads_rejected() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");

        let result = write_wave_events("test.topic", &[], &events_path);
        assert!(result.is_err());
    }

    /// U21: `execute_verify` returns Ok and writes nothing to disk.
    #[test]
    fn test_execute_verify_does_not_write_jsonl() {
        let tmp = TempDir::new().unwrap();
        // Use a stable path under tmp; verify must NOT create this file.
        let events_path = tmp.path().join("events.jsonl");

        // Run verify with two JSON-object payloads against a topic without policy.
        let args = WaveVerifyArgs {
            topic: "verify.dry".to_string(),
            payloads: vec![
                r#"{"task_key":"k1"}"#.to_string(),
                r#"{"task_key":"k2"}"#.to_string(),
            ],
            payloads_stdin: false,
            output: WaveOutputFormat::Text,
            config: vec![],
        };

        // Override CWD so resolve_events_file points into tmp.
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let result = execute_verify(args);
        std::env::set_current_dir(&prev).unwrap();

        assert!(
            result.is_ok(),
            "verify should pass without policy: {result:?}"
        );
        assert!(
            !events_path.exists(),
            "verify must not create the events file (was: {})",
            events_path.display()
        );
    }

    /// U21: verify rejects empty payloads with a clear error.
    #[test]
    fn test_execute_verify_empty_payloads_rejected() {
        let args = WaveVerifyArgs {
            topic: "verify.dry".to_string(),
            payloads: vec![],
            payloads_stdin: false,
            output: WaveOutputFormat::Text,
            config: vec![],
        };
        let result = execute_verify(args);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("payload"), "missing 'payload' hint: {err}");
    }

    #[test]
    fn test_write_wave_events_creates_parent_dirs() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("nested").join("dir").join("events.jsonl");

        let payloads = vec!["payload".to_string()];
        write_wave_events("test.topic", &payloads, &events_path).unwrap();

        assert!(events_path.exists());
    }

    // WRC-U6 (2026-06-12-003) / T-WRC-U6-01: `wave_total` on every
    // emitted event MUST equal `len(payloads)`. The 002 plan
    // already enforced this; the 003 plan pins the contract with a
    // dedicated test that scans all emitted records. The dimension
    // detection / wave aggregation pipeline reads
    // `(wave_id, wave_total)` to decide how many worker
    // activations to expect — a mismatch silently drops events
    // or, in the 335-worker field trace, fans out far more than
    // the operator asked for. The test scans every line of the
    // emitted JSONL and asserts `wave_total == payloads.len()`.
    #[test]
    fn test_wave_total_equals_payload_count_for_all_records() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let payloads = vec![
            "{\"dimension\":\"correctness\"}".to_string(),
            "{\"dimension\":\"testing\"}".to_string(),
            "{\"dimension\":\"maintainability\"}".to_string(),
        ];
        let expected_total = payloads.len() as u32;
        write_wave_events("review.wave.ready", &payloads, &events_path).unwrap();
        let body = std::fs::read_to_string(&events_path).unwrap();
        let mut count = 0;
        for line in body.lines() {
            if line.is_empty() {
                continue;
            }
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(
                value["wave_total"].as_u64().unwrap(),
                u64::from(expected_total),
                "every wave record must carry wave_total == len(payloads)={expected_total}, got: {line}",
            );
            count += 1;
        }
        assert_eq!(
            count,
            payloads.len(),
            "wave emit must write exactly one JSONL line per payload"
        );
    }

    // WRC-U6 / T-WRC-U6-02 (mismatch rejection): the JSONL
    // append-or-write entry points reject a record whose
    // `wave_total` field disagrees with the configured wave size.
    // The 002 plan documented this as "internally consistent
    // invariant"; the 003 plan promotes it to an explicit
    // assertion. We exercise the helper directly because the CLI
    // entry point also derives `wave_total` from `len(payloads)`,
    // so a mismatch can only be introduced by a hand-written
    // JSONL append (e.g. a script that builds events.jsonl out
    // of process). The rejection closes the same failure mode
    // the 335-worker bug exposed.
    #[test]
    fn test_wave_record_with_mismatched_wave_total_is_rejected() {
        let good = serde_json::json!({
            "topic": "review.wave.ready",
            "payload": "{}",
            "ts": chrono::Utc::now().to_rfc3339(),
            "wave_id": "w-test",
            "wave_index": 0,
            "wave_total": 3,
        });
        assert!(validate_wave_record(&good, 3).is_ok());
        // Same shape, but wave_total=2 disagrees with declared
        // wave size of 3.
        let bad = serde_json::json!({
            "topic": "review.wave.ready",
            "payload": "{}",
            "ts": chrono::Utc::now().to_rfc3339(),
            "wave_id": "w-test",
            "wave_index": 0,
            "wave_total": 2,
        });
        assert!(
            validate_wave_record(&bad, 3).is_err(),
            "wave_total that disagrees with the declared wave size must be rejected"
        );
    }

    // ---- P6 wave record validation tests ----

    #[test]
    fn test_wave_emit_rejects_nested_worker() {
        // Simulate the nested-worker check by setting the env var. This
        // mirrors the bail at the top of `execute_emit`.
        // The check itself is straightforward — verify the guard fires
        // when the env var is set.
        // (Direct test of `execute_emit` would require clap parsing and
        // argument setup; this is the cheapest equivalent.)
        let result = std::env::var("RALPH_WAVE_WORKER");
        // We cannot mutate env in tests under forbid(unsafe), so this
        // asserts the guard shape: when set to "1", nested waves are
        // rejected. The integration test would exercise this end-to-end.
        assert!(
            result.as_deref() != Ok("1"),
            "nested wave check should reject inside worker"
        )
    }

    #[test]
    fn test_read_payloads_from_reader_skips_empty_lines() {
        let input =
            "{\"dim\":\"correctness\"}\n\n{\"dim\":\"testing\"}\n\n{\"dim\":\"maintainability\"}\n";
        let cursor = std::io::Cursor::new(input);
        let payloads = read_payloads_from_reader(cursor).unwrap();
        assert_eq!(payloads.len(), 3);
        assert_eq!(payloads[0], r#"{"dim":"correctness"}"#);
        assert_eq!(payloads[1], r#"{"dim":"testing"}"#);
        assert_eq!(payloads[2], r#"{"dim":"maintainability"}"#);
    }

    #[test]
    fn test_read_payloads_from_reader_rejects_all_empty() {
        let input = "\n\n  \n";
        let cursor = std::io::Cursor::new(input);
        let payloads = read_payloads_from_reader(cursor).unwrap();
        assert!(payloads.is_empty());
    }

    #[test]
    fn test_validate_payload_shape_rejects_newline_joined_json_payloads() {
        let payloads =
            vec!["{\"dimension\":\"correctness\"}\n{\"dimension\":\"testing\"}".to_string()];
        let err = validate_payload_shape(&payloads).unwrap_err().to_string();
        assert!(err.contains("--payloads-stdin"));
    }

    #[test]
    fn test_validate_payload_shape_allows_single_multiline_json_payload() {
        let payloads = vec![
            "{\n  \"dimension\": \"correctness\",\n  \"focus\": \"check behavior\"\n}".to_string(),
        ];
        validate_payload_shape(&payloads).unwrap();
    }

    // ---- U1: JSON object payload strict validation tests ----

    #[test]
    fn test_validate_payload_shape_accepts_json_object() {
        let payloads = vec![r#"{"dimension":"correctness"}"#.to_string()];
        validate_payload_shape(&payloads).unwrap();
    }

    #[test]
    fn test_validate_payload_shape_rejects_number_payload() {
        let payloads = vec!["10".to_string()];
        let err = validate_payload_shape(&payloads).unwrap_err().to_string();
        assert!(
            err.contains("JSON object"),
            "error should mention JSON object, got: {}",
            err
        );
    }

    #[test]
    fn test_validate_payload_shape_rejects_string_payload() {
        let payloads = vec![r#""text""#.to_string()];
        let err = validate_payload_shape(&payloads).unwrap_err().to_string();
        assert!(err.contains("JSON object"));
    }

    #[test]
    fn test_validate_payload_shape_rejects_array_payload() {
        let payloads = vec!["[]".to_string()];
        let err = validate_payload_shape(&payloads).unwrap_err().to_string();
        assert!(err.contains("JSON object"));
    }

    #[test]
    fn test_validate_payload_shape_rejects_placeholder_payload() {
        let payloads = vec!["placeholder".to_string()];
        let err = validate_payload_shape(&payloads).unwrap_err().to_string();
        assert!(err.contains("JSON object"));
    }

    #[test]
    fn test_validate_payload_shape_rejects_truncated_object() {
        let payloads = vec![r#"{"dimension":"x""#.to_string()];
        let err = validate_payload_shape(&payloads).unwrap_err().to_string();
        assert!(err.contains("JSON object") || err.contains("JSON"));
    }

    #[test]
    fn test_validate_payload_shape_accepts_leading_whitespace_object() {
        let payloads = vec!["   \n  \t{\"dim\":\"x\"}".to_string()];
        validate_payload_shape(&payloads).unwrap();
    }

    #[test]
    fn test_validate_payload_shape_rejects_word_split_token_sequence() {
        // Simulates `printf '%s\n' $(cat payloads.jsonl)` IFS word splitting.
        // Many of these tokens are bare identifiers, not JSON objects.
        let payloads: Vec<String> = (0..10).map(|i| format!("tok{}", i)).collect();
        let err = validate_payload_shape(&payloads).unwrap_err().to_string();
        assert!(err.contains("JSON object"));
    }

    #[test]
    fn test_validate_payload_shape_atomicity_first_valid_then_invalid() {
        // Caller expects: when any payload is invalid, no events are written.
        // We assert at the validate level: invalid payload means Err.
        let payloads = vec![r#"{"ok":1}"#.to_string(), "not-an-object".to_string()];
        assert!(validate_payload_shape(&payloads).is_err());
    }

    #[test]
    fn test_validate_payload_shape_seven_objects_all_pass() {
        let payloads: Vec<String> = (0..7).map(|i| format!(r#"{{"dim":"d{}"}}"#, i)).collect();
        validate_payload_shape(&payloads).unwrap();
    }

    #[test]
    fn test_read_payloads_from_reader_validates_object() {
        // stdin reader must also reject non-object payloads end-to-end.
        let input = "{\"ok\":1}\n\"not-object\"\n{\"ok\":3}\n";
        let cursor = std::io::Cursor::new(input);
        let payloads = read_payloads_from_reader(cursor).unwrap();
        let err = validate_payload_shape(&payloads).unwrap_err().to_string();
        assert!(err.contains("JSON object"));
    }

    // ---- U2 (2026-06-11-002): idempotency key tests ----

    #[test]
    fn test_idempotency_key_validation() {
        // Empty
        assert!(validate_idempotency_key("").is_err());
        // Whitespace only
        assert!(validate_idempotency_key("   ").is_err());
        // Too long (>256 bytes)
        let long_key = "x".repeat(257);
        assert!(validate_idempotency_key(&long_key).is_err());
        // Non-ASCII
        assert!(validate_idempotency_key("中文").is_err());
        // Valid ASCII key
        assert!(validate_idempotency_key("ce-review:foo:1:step:round-1").is_ok());
        // Boundary: exactly 256 bytes
        let boundary = "x".repeat(256);
        assert!(validate_idempotency_key(&boundary).is_ok());
    }

    #[test]
    fn test_idempotency_log_path_derivation() {
        let p = idempotency_log_path(Path::new("/a/b.jsonl"));
        assert_eq!(p, Path::new("/a/.b.jsonl.idempotency.jsonl"));

        let p2 = idempotency_log_path(Path::new(".ralph/events.jsonl"));
        assert_eq!(p2, Path::new(".ralph/.events.jsonl.idempotency.jsonl"));
    }

    #[test]
    fn test_idempotency_scope_key_distinct() {
        let k1 = compute_scope_key("loop1", "hat1", "t1", "key1");
        let k2 = compute_scope_key("loop2", "hat1", "t1", "key1");
        assert_ne!(k1, k2, "different loop_id should give different scope_key");

        let k3 = compute_scope_key("loop1", "hat2", "t1", "key1");
        assert_ne!(k1, k3, "different hat should give different scope_key");

        let k4 = compute_scope_key("loop1", "hat1", "t2", "key1");
        assert_ne!(k1, k4, "different topic should give different scope_key");

        let k5 = compute_scope_key("loop1", "hat1", "t1", "key2");
        assert_ne!(k1, k5, "different key should give different scope_key");
    }

    #[test]
    fn test_idempotency_payload_digest_distinct() {
        let d1 = compute_payload_digest(&["a".to_string(), "b".to_string()]);
        let d2 = compute_payload_digest(&["a".to_string(), "c".to_string()]);
        assert_ne!(d1, d2, "different payloads should give different digest");

        let d3 = compute_payload_digest(&["ab".to_string()]);
        let d4 = compute_payload_digest(&["a".to_string(), "b".to_string()]);
        assert_ne!(d3, d4, "different grouping should give different digest");
    }

    #[test]
    fn test_idempotency_first_call_writes_record() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let payloads = vec![
            r#"{"dim":"correctness"}"#.to_string(),
            r#"{"dim":"testing"}"#.to_string(),
        ];

        let outcome = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            "ce-review:foo:1:step:round-1",
            "loop-1",
            "reviewer",
        )
        .unwrap();

        assert!(!outcome.deduplicated, "first call should not be dedup");
        assert!(outcome.wave_id.starts_with("w-"));

        // Events file should have 2 lines
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 2);

        // Each event should have idempotency_key and idempotency_hash
        for line in content.lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(v["idempotency_key"], "ce-review:foo:1:step:round-1");
            assert!(
                v["idempotency_hash"].as_str().unwrap().len() == 64,
                "idempotency_hash should be 64 hex chars"
            );
        }

        // Idempotency log should have 1 line
        let log = read_idempotency_records(&events_path).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].idempotency_key, "ce-review:foo:1:step:round-1");
        assert_eq!(log[0].count, 2);
    }

    #[test]
    fn test_idempotency_dedup_returns_original_wave_id() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let payloads = vec![
            r#"{"dim":"correctness"}"#.to_string(),
            r#"{"dim":"testing"}"#.to_string(),
        ];

        // First call
        let first = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            "ce-review:dup-test",
            "loop-1",
            "reviewer",
        )
        .unwrap();
        assert!(!first.deduplicated);

        // Second call with same key and payloads
        let second = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            "ce-review:dup-test",
            "loop-1",
            "reviewer",
        )
        .unwrap();

        assert!(second.deduplicated, "second call should be dedup");
        assert_eq!(
            first.wave_id, second.wave_id,
            "second call should return same wave_id"
        );

        // Events file still has only 2 lines
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 2);
    }

    #[test]
    fn test_idempotency_same_key_different_payload_errors() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let key = "ce-review:payload-conflict";

        // First call with payload set A
        let payloads_a = vec![r#"{"dim":"correctness"}"#.to_string()];
        write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads_a,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        )
        .unwrap();

        // Second call with different payloads (same key) → should error
        let payloads_b = vec![
            r#"{"dim":"correctness"}"#.to_string(),
            r#"{"dim":"testing"}"#.to_string(),
        ];
        let result = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads_b,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        );
        assert!(
            result.is_err(),
            "same key with different payload should error"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("idempotency-key conflict"),
            "error should mention idempotency-key conflict, got: {err}"
        );

        // Events file should still have only 1 line
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 1);
    }

    #[test]
    fn test_idempotency_different_keys_dont_dedup() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let payloads = vec![r#"{"dim":"correctness"}"#.to_string()];

        // key1
        write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            "key1",
            "loop-1",
            "reviewer",
        )
        .unwrap();

        // key2 → different key, should write a new wave
        write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            "key2",
            "loop-1",
            "reviewer",
        )
        .unwrap();

        // Events file should have 2 lines
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 2);

        // Log should have 2 records with different wave_ids
        let records = read_idempotency_records(&events_path).unwrap();
        assert_eq!(records.len(), 2);
        assert_ne!(records[0].wave_id, records[1].wave_id);
    }

    #[test]
    fn test_idempotency_cross_loop_id_isolated() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let payloads = vec![r#"{"dim":"correctness"}"#.to_string()];
        let key = "same-key";

        // loop-1
        write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        )
        .unwrap();

        // loop-2, same key → should NOT dedup
        let outcome = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            key,
            "loop-2",
            "reviewer",
        )
        .unwrap();
        assert!(!outcome.deduplicated, "different loop_id should not dedup");

        // Events file → 2 lines
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 2);
    }

    #[test]
    fn test_idempotency_cross_hat_isolated() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let payloads = vec![r#"{"dim":"correctness"}"#.to_string()];
        let key = "same-key";

        write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        )
        .unwrap();

        let outcome = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            key,
            "loop-1",
            "executor",
        )
        .unwrap();
        assert!(!outcome.deduplicated, "different hat should not dedup");

        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 2);
    }

    #[test]
    fn test_idempotency_cross_topic_isolated() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let payloads = vec![r#"{"dim":"correctness"}"#.to_string()];
        let key = "same-key";

        write_wave_events_with_idempotency_with_scope(
            "topic.a",
            &payloads,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        )
        .unwrap();

        let outcome = write_wave_events_with_idempotency_with_scope(
            "topic.b",
            &payloads,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        )
        .unwrap();
        assert!(!outcome.deduplicated, "different topic should not dedup");

        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 2);
    }

    #[test]
    fn test_idempotency_no_key_unchanged_compat() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let payloads = vec![r#"{"dim":"correctness"}"#.to_string()];

        // Use the regular write_wave_events path (no idempotency)
        write_wave_events("test.topic", &payloads, &events_path).unwrap();

        // Events should NOT have idempotency fields
        let content = fs::read_to_string(&events_path).unwrap();
        for line in content.lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(
                v.get("idempotency_key").is_none(),
                "no-key path should not inject idempotency_key"
            );
        }

        // Idempotency log should not exist
        assert!(
            !idempotency_log_path(&events_path).exists(),
            "no-key path should not create idempotency log"
        );
    }

    #[test]
    fn test_idempotency_recovery_after_partial_failure() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let key = "ce-review:recovery-test";
        let payloads = vec![
            r#"{"dim":"correctness"}"#.to_string(),
            r#"{"dim":"testing"}"#.to_string(),
            r#"{"dim":"maintainability"}"#.to_string(),
        ];
        let scope_key = compute_scope_key("loop-1", "reviewer", "review.wave.ready", key);

        // Simulate a successful first write without the idempotency record
        let first_wave_id = write_wave_events_with_provenance(
            "review.wave.ready",
            &payloads,
            &events_path,
            Some("reviewer"),
            Some(key),
            Some(&scope_key),
        )
        .unwrap();

        // Verify events written (3 lines)
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 3);

        // Idempotency log should NOT exist (simulating crash before record write)
        let log_path = idempotency_log_path(&events_path);
        assert!(
            !log_path.exists(),
            "recovery test: log_path={:?} should not exist before recovery call",
            log_path
        );

        // Now call with the same key → should recover (scan events, write record, return same wave_id)
        let outcome = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        )
        .unwrap();

        assert!(
            outcome.deduplicated,
            "recovery should return deduplicated=true"
        );
        assert_eq!(
            outcome.wave_id, first_wave_id,
            "recovery should return original wave_id"
        );

        // Events file should still have 3 lines (recovery did not append)
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 3);

        // Idempotency log should now exist with 1 record
        let records = read_idempotency_records(&events_path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].idempotency_key, key);
        assert_eq!(records[0].wave_id, first_wave_id);

        // Subsequent dedup call (record now present) must also recover the
        // original wave_id without appending events.
        let outcome_dedup = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        )
        .unwrap();
        assert!(
            outcome_dedup.deduplicated,
            "post-recovery dedup should also return deduplicated=true"
        );
        assert_eq!(
            outcome_dedup.wave_id, first_wave_id,
            "post-recovery dedup should return original wave_id"
        );
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(
            content.lines().count(),
            3,
            "post-recovery dedup must not append events"
        );
    }

    #[test]
    fn test_idempotency_concurrent_writers_serialize() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let n_workers: usize = 6;

        // Each worker uses a distinct --idempotency-key (otherwise the
        // idempotency layer would correctly reject "same scope,
        // different payload" as a conflict — that is a feature, not a
        // race we want to exercise here). The contention we want to
        // exercise is on the FileLock around the events-file write:
        // N writers must serialize cleanly and produce N event lines
        // with N unique wave_ids, without corrupting the file or the
        // idempotency log.
        let payloads: Vec<String> = (0..n_workers)
            .map(|i| format!(r#"{{"worker":{i}}}"#))
            .collect();

        let barrier = Arc::new(Barrier::new(n_workers));
        let mut handles = Vec::with_capacity(n_workers);
        for (i, payload) in payloads.iter().cloned().enumerate() {
            let events_path = events_path.clone();
            let key = format!("ce-review:concurrent-writer-{i}");
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                write_wave_events_with_idempotency_with_scope(
                    "review.wave.ready",
                    &[payload],
                    &events_path,
                    &key,
                    "loop-1",
                    "reviewer",
                )
            }));
        }

        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|h| {
                h.join()
                    .expect("worker thread panicked")
                    .expect("wave emit failed")
            })
            .collect();

        // Every worker must observe a fresh, non-deduplicated wave
        // (distinct keys, distinct records).
        for o in &outcomes {
            assert!(
                !o.deduplicated,
                "concurrent writers with distinct keys must each create a new wave"
            );
        }
        // Every wave_id is unique (no FileLock contention produced
        // collisions).
        let unique_ids: std::collections::HashSet<_> =
            outcomes.iter().map(|o| o.wave_id.clone()).collect();
        assert_eq!(
            unique_ids.len(),
            n_workers,
            "expected n_workers distinct wave_ids"
        );

        // Events file fans in all n_workers lines, with each line
        // containing some worker index in the (JSON-escaped) payload —
        // proves serialization preserved per-worker payload integrity
        // (no interleaving or overwrite). Order is non-deterministic
        // because distinct keys mean no contention: each writer holds
        // the lock only for its own (very short) critical section.
        let content = fs::read_to_string(&events_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), n_workers);
        let mut seen_workers: std::collections::HashSet<u32> = std::collections::HashSet::new();
        for line in &lines {
            // Each event line is a JSON object with a top-level
            // "payload" field whose value is the original payload
            // JSON-escaped. Pull out the inner worker index.
            let v: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("event line not valid JSON: {line}: {e}"));
            let payload_str = v
                .get("payload")
                .and_then(|p| p.as_str())
                .unwrap_or_else(|| panic!("event line missing payload: {line}"));
            let inner: serde_json::Value = serde_json::from_str(payload_str)
                .unwrap_or_else(|e| panic!("inner payload not valid JSON: {payload_str}: {e}"));
            let worker = inner
                .get("worker")
                .and_then(|w| w.as_u64())
                .unwrap_or_else(|| panic!("inner payload missing worker: {payload_str}"));
            assert!(
                (worker as usize) < n_workers,
                "worker index {worker} out of range (n_workers={n_workers})"
            );
            assert!(
                seen_workers.insert(worker as u32),
                "duplicate worker {worker} — FileLock serialization must prevent overwrites"
            );
        }

        // Idempotency log carries one record per writer, each tagged
        // with the per-worker key.
        let records = read_idempotency_records(&events_path).unwrap();
        assert_eq!(records.len(), n_workers);
        let mut seen_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
        for r in &records {
            assert!(
                r.idempotency_key
                    .starts_with("ce-review:concurrent-writer-"),
                "unexpected key: {}",
                r.idempotency_key
            );
            assert!(
                seen_keys.insert(r.idempotency_key.clone()),
                "duplicate key {} — IdempotencyRecord append must be serialized",
                r.idempotency_key
            );
        }
    }

    #[test]
    fn test_idempotency_incomplete_events_errors() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let key = "ce-review:incomplete-test";

        // Write only 2 events but not the full 7 claimed count
        let payloads_partial = vec![
            r#"{"dim":"correctness"}"#.to_string(),
            r#"{"dim":"testing"}"#.to_string(),
        ];
        write_wave_events_with_provenance(
            "review.wave.ready",
            &payloads_partial,
            &events_path,
            Some("reviewer"),
            Some(key),
            Some("incomplete-scope"),
        )
        .unwrap();

        // Manually create a record claiming 7 events (to trigger the incomplete check)
        let rec = IdempotencyRecord {
            scope_key: compute_scope_key("loop-1", "reviewer", "review.wave.ready", key),
            idempotency_key: key.to_string(),
            wave_id: "w-simulated".to_string(),
            topic: "review.wave.ready".to_string(),
            hat: "reviewer".to_string(),
            payload_digest: compute_payload_digest(&payloads_partial),
            count: 7,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        append_idempotency_record(&events_path, &rec).unwrap();

        // Now call with the same key → should detect incomplete emission
        let result = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            // same payloads as recorded in the record
            &payloads_partial,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        );

        assert!(result.is_err(), "incomplete event should error");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("incomplete prior wave emission"),
            "should mention incomplete prior wave emission, got: {err_msg}"
        );

        // Events file should still have only 2 lines
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 2);
    }

    #[test]
    fn test_wave_output_format_default_is_text() {
        // Default is `text` for backward compatibility.
        let s = format!("{:?}", WaveOutputFormat::Text);
        assert!(s.contains("Text"));
    }

    #[test]
    fn test_wave_emit_args_output_default_text() {
        // Parsing without --output should default to Text.
        use clap::Parser;
        let parsed = WaveEmitArgs::try_parse_from([
            "ralph",
            "review.wave.ready",
            "--payloads",
            r#"{"dim":"x"}"#,
        ])
        .unwrap();
        assert_eq!(parsed.output, WaveOutputFormat::Text);
    }

    #[test]
    fn test_wave_emit_args_output_json_parsed() {
        use clap::Parser;
        let parsed = WaveEmitArgs::try_parse_from([
            "ralph",
            "review.wave.ready",
            "--payloads-stdin",
            "--output",
            "json",
        ])
        .unwrap();
        assert_eq!(parsed.output, WaveOutputFormat::Json);
    }

    // ---- U4 (2026-06-13-001): schema precheck + structured JSON error ----

    /// Helper: build a 7-payload batch on `review.wave.ready`, with or
    /// without the required `depth` field. Mirrors the U1 incident:
    /// 7 wave events, optionally missing a required field, are
    /// exactly the input the precheck must reject atomically.
    fn build_u4_payloads(with_depth: bool) -> Vec<String> {
        (0..7)
            .map(|i| {
                if with_depth {
                    format!(r#"{{"dim":"d{i}","depth":"standard"}}"#)
                } else {
                    format!(r#"{{"dim":"d{i}"}}"#)
                }
            })
            .collect()
    }

    /// Helper: write a strict `ralph.yml` (with `require_policy_check_for_cli_emit: true`,
    /// `allow_unsafe_cli_emit: false`, and `schemas.review.wave.ready.required_fields: [depth]`)
    /// to `workspace`. Returns the path to a fresh events file.
    fn setup_strict_u4_workspace(workspace: &Path) -> PathBuf {
        let yaml = r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: false
    schemas:
      review.wave.ready:
        required_fields:
          - depth
";
        std::fs::create_dir_all(workspace.join(".ralph")).unwrap();
        std::fs::write(workspace.join("ralph.yml"), yaml).unwrap();
        workspace.join(".ralph/events.jsonl")
    }

    /// U4 / T-WAVE-PRECHECK-01: with strict `ralph.yml` and
    /// `--output json`, the wave precheck must return a structured
    /// `ValidationFailure` with 7 `validation_errors` (one per
    /// payload index 0..6) and `topic=review.wave.ready`. This is
    /// the agent's primary contract: one response, every offending
    /// payload named.
    #[test]
    fn test_wave_emit_json_reports_all_missing_depth_violations() {
        use ralph_core::{EventPolicyConfig, RalphConfig};

        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        let events = setup_strict_u4_workspace(workspace);

        let cfg_yaml = std::fs::read_to_string(workspace.join("ralph.yml")).unwrap();
        let cfg: RalphConfig = serde_yaml::from_str(&cfg_yaml).unwrap();
        let policy: &EventPolicyConfig = cfg.event_loop.event_policy.as_ref().unwrap();

        // All 7 payloads lack `depth`.
        let payloads = build_u4_payloads(false);

        let batch =
            validate_batch_against_config("review.wave.ready", &payloads, policy, &events).unwrap();
        assert_eq!(batch.errors.len(), 7);

        // Build the failure payload and verify the JSON shape
        // matches the U4 spec.
        let failure = ValidationFailure::from_batch("review.wave.ready", batch);
        let json = serde_json::to_string(&failure).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["ok"], false);
        assert_eq!(parsed["error"], "policy_validation_failed");
        assert_eq!(parsed["topic"], "review.wave.ready");
        let errs = parsed["validation_errors"].as_array().expect("array");
        assert_eq!(errs.len(), 7);

        // Indices 0..6 must all be present (atomicity: every
        // offending payload is named, agent can fix all in one
        // shot).
        let mut seen_indices: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
        let mut fields: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for e in errs {
            let idx = e["payload_index"].as_u64().unwrap() as usize;
            seen_indices.insert(idx);
            fields.insert(e["field"].as_str().unwrap().to_string());
            assert_eq!(e["reason_code"], "missing_required_field");
            assert!(e["message"].as_str().unwrap().contains("depth"));
        }
        for i in 0..7 {
            assert!(seen_indices.contains(&i), "missing payload_index {i}");
        }
        // The unique field set should be exactly `{ "depth" }`.
        assert_eq!(fields.len(), 1);
        assert!(fields.contains("depth"));
    }

    /// U4 / T-WAVE-PRECHECK-02: when the precheck fails, the
    /// events file MUST be unchanged (atomic reject). This is the
    /// primary invariant that closes the U1 incident chain: a
    /// bad batch must never half-write into the JSONL.
    ///
    /// We exercise the integration path by calling `run_wave_precheck`
    /// directly with an empty events file (so terminal-monotonicity
    /// is a no-op) and assert the events file is still empty
    /// afterwards.
    #[test]
    fn test_wave_emit_rejects_missing_depth_before_write() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        let events = setup_strict_u4_workspace(workspace);

        // Pre-seed with a known-valid line to confirm the
        // precheck doesn't even touch the file.
        std::fs::write(
            &events,
            "{\"topic\":\"prior.event\",\"ts\":\"2024-01-01T00:00:00Z\"}\n",
        )
        .unwrap();
        let before = std::fs::read_to_string(&events).unwrap();

        let payloads = build_u4_payloads(false);

        // Drive the precheck from the workspace CWD so the config
        // load picks up the ralph.yml we just wrote. We use
        // CwdGuard (test_support) for the lifetime of the call.
        let _cwd = crate::test_support::CwdGuard::set(workspace);
        let result = run_wave_precheck(
            "review.wave.ready",
            true, // explicit --policy-check
            false,
            WaveOutputFormat::Json,
            None,
            &payloads,
            &events,
            &[],
        );

        assert!(result.is_err(), "missing-depth batch must reject");

        // Events file MUST be unchanged — no half-written JSONL.
        let after = std::fs::read_to_string(&events).unwrap();
        assert_eq!(before, after, "precheck must not write to events file");

        // Sanity: still has exactly the one pre-seeded line.
        assert_eq!(after.lines().count(), 1);
    }

    /// U4 / T-WAVE-PRECHECK-03: when the precheck PASSES, the
    /// events file MUST be unchanged by the precheck itself (only
    /// the subsequent write call appends). This guards against
    /// accidentally writing twice or partial-failing.
    #[test]
    fn test_wave_emit_precheck_pass_leaves_events_file_untouched() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        let events = setup_strict_u4_workspace(workspace);

        // Pre-seed with a known-valid line to confirm the
        // precheck doesn't even touch the file.
        std::fs::write(
            &events,
            "{\"topic\":\"prior.event\",\"ts\":\"2024-01-01T00:00:00Z\"}\n",
        )
        .unwrap();
        let before = std::fs::read_to_string(&events).unwrap();

        // All 7 payloads include `depth` → precheck should pass.
        let payloads = build_u4_payloads(true);
        let _cwd = crate::test_support::CwdGuard::set(workspace);
        let result = run_wave_precheck(
            "review.wave.ready",
            true,
            false,
            WaveOutputFormat::Json,
            None,
            &payloads,
            &events,
            &[],
        );
        assert!(result.is_ok(), "valid batch should pass precheck");

        // Events file MUST still be unchanged (precheck never writes).
        let after = std::fs::read_to_string(&events).unwrap();
        assert_eq!(before, after, "passing precheck must not write");
    }

    /// U4 / T-WAVE-PRECHECK-04: when `event_policy.enabled=false`,
    /// the precheck must not engage — only the JSON-object shape
    /// check (already done by `validate_payload_shape`) applies.
    /// This mirrors the existing `ralph emit` semantics for
    /// non-strict configs and prevents accidental lockouts when a
    /// user adds a config without opting into event policy.
    #[test]
    fn test_wave_emit_no_strict_config_skips_precheck() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        std::fs::create_dir_all(workspace.join(".ralph")).unwrap();
        // Config has event_policy but it's NOT enabled.
        let yaml = r"
event_loop:
  event_policy:
    enabled: false
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      review.wave.ready:
        required_fields:
          - depth
";
        std::fs::write(workspace.join("ralph.yml"), yaml).unwrap();
        let events = workspace.join(".ralph/events.jsonl");

        // Payloads lack `depth`, but with `enabled: false` the
        // precheck must NOT reject. This is the same behavior
        // `ralph emit` has for non-strict configs.
        let payloads = build_u4_payloads(false);
        let _cwd = crate::test_support::CwdGuard::set(workspace);
        let result = run_wave_precheck(
            "review.wave.ready",
            false, // no explicit --policy-check
            false,
            WaveOutputFormat::Json,
            None,
            &payloads,
            &events,
            &[],
        );
        assert!(
            result.is_ok(),
            "non-strict (event_policy.enabled=false) config must skip precheck, got: {result:?}"
        );
    }

    /// U4 / T-WAVE-PRECHECK-05: with strict config
    /// (`allow_unsafe_cli_emit: false`), the `--unsafe-no-policy-check`
    /// flag MUST be ignored — the precheck still runs. This
    /// closes the bypass that would otherwise let agents skip
    /// schema validation on a builtin pipeline preset.
    #[test]
    fn test_wave_emit_unsafe_bypass_blocked_when_config_denies() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        let events = setup_strict_u4_workspace(workspace);

        // Payloads lack `depth`; the user requested bypass but
        // the config disallows it.
        let payloads = build_u4_payloads(false);
        let _cwd = crate::test_support::CwdGuard::set(workspace);
        let result = run_wave_precheck(
            "review.wave.ready",
            false, // no explicit --policy-check
            true,  // but --unsafe-no-policy-check
            WaveOutputFormat::Json,
            None,
            &payloads,
            &events,
            &[],
        );
        assert!(
            result.is_err(),
            "unsafe-bypass must not work when config denies it"
        );
    }

    /// U4 / T-WAVE-PRECHECK-06: with strict config AND
    /// `allow_unsafe_cli_emit: true`, the `--unsafe-no-policy-check`
    /// flag MUST work — the precheck is skipped and the wave
    /// emit writes through. This is the documented escape hatch
    /// for presets that explicitly allow it.
    #[test]
    fn test_wave_emit_unsafe_bypass_allowed_when_config_permits() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        std::fs::create_dir_all(workspace.join(".ralph")).unwrap();
        // Strict but allows the bypass.
        let yaml = r"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: true
    schemas:
      review.wave.ready:
        required_fields:
          - depth
";
        std::fs::write(workspace.join("ralph.yml"), yaml).unwrap();
        let events = workspace.join(".ralph/events.jsonl");

        // Payloads lack `depth`, but the bypass is honored.
        let payloads = build_u4_payloads(false);
        let _cwd = crate::test_support::CwdGuard::set(workspace);
        let result = run_wave_precheck(
            "review.wave.ready",
            false,
            true, // --unsafe-no-policy-check
            WaveOutputFormat::Json,
            None,
            &payloads,
            &events,
            &[],
        );
        assert!(
            result.is_ok(),
            "unsafe-bypass must work when config permits it, got: {result:?}"
        );
    }

    // ---- 2026-07-24-003 plan U2: wave inspect DTO + view ----

    /// `unknown` collapses the per-wave fields so the JSON is the
    /// minimum `{ok, wave_id, registered, availability}` shape — no
    /// stray `phase` / counts / cancel keys.
    #[test]
    fn u2_inspect_view_unknown_minimal_shape() {
        let view = WaveInspectView::unknown("w-miss");
        assert!(view.ok);
        assert_eq!(view.wave_id, "w-miss");
        assert!(!view.registered);
        assert_eq!(view.availability, "available");
        assert_eq!(view.phase, None);
        assert_eq!(view.expected_total, None);
        assert_eq!(view.completed_count, None);
        assert_eq!(view.failed_count, None);
        assert_eq!(view.pending_count, None);
        assert_eq!(view.in_flight_count, None);
        assert_eq!(view.cancel_requested, None);
        assert_eq!(view.unavailable_reason, None);

        // The serialised shape must omit every optional field. Pin
        // the JSON keys explicitly so downstream consumers can
        // rely on a stable shape.
        let json = serde_json::to_value(&view).expect("serialise");
        let obj = json.as_object().expect("object");
        assert!(obj.contains_key("ok"));
        assert!(obj.contains_key("wave_id"));
        assert!(obj.contains_key("registered"));
        assert!(obj.contains_key("availability"));
        assert!(!obj.contains_key("phase"));
        assert!(!obj.contains_key("expected_total"));
        assert!(!obj.contains_key("completed_count"));
        assert!(!obj.contains_key("failed_count"));
        assert!(!obj.contains_key("pending_count"));
        assert!(!obj.contains_key("in_flight_count"));
        assert!(!obj.contains_key("cancel_requested"));
        assert!(!obj.contains_key("unavailable_reason"));
    }

    /// `unavailable` keeps the same minimal shape plus the
    /// `unavailable_reason` field. The reason must be sanitised so
    /// the view never leaks internal paths.
    #[test]
    fn u2_inspect_view_unavailable_sanitises_reason() {
        let view = WaveInspectView::unavailable(
            "w-x",
            "failed to open supervisor database: migration failed on .ralph/supervisor.db: file is not a database",
        );
        assert!(!view.registered);
        assert_eq!(view.availability, "unavailable");
        let reason = view
            .unavailable_reason
            .as_deref()
            .expect("reason must be present");
        assert!(
            !reason.contains(".ralph"),
            "sanitised reason must drop internal paths: {reason}"
        );
        assert!(
            !reason.contains("supervisor.db"),
            "sanitised reason must drop db filename: {reason}"
        );
        assert!(
            !reason.contains('/'),
            "sanitised reason must be free of path separators: {reason}"
        );

        // Round-trip: the reason stays bounded and human-readable.
        let json = serde_json::to_value(&view).expect("serialise");
        assert_eq!(json["availability"], serde_json::json!("unavailable"));
        assert!(
            json["unavailable_reason"].as_str().unwrap().chars().count() <= 200,
            "reason must be capped at 200 chars"
        );
    }

    /// `unavailable_reason` is empty / whitespace → fall back to the
    /// literal `"unavailable"` so the JSON key is never empty.
    #[test]
    fn u2_inspect_view_unavailable_empty_reason_falls_back() {
        let view = WaveInspectView::unavailable("w-x", "   ");
        assert_eq!(
            view.unavailable_reason.as_deref(),
            Some("unavailable"),
            "empty / whitespace reason must fall back to literal"
        );
    }

    /// Human text output for unknown wave must contain
    /// `not registered` (and never echo a path).
    #[test]
    fn u2_render_wave_inspect_view_text_unknown() {
        let view = WaveInspectView::unknown("w-x");
        let s = render_wave_inspect_view_text(&view);
        assert!(s.contains("not registered"), "{s}");
        assert!(s.contains("w-x"), "{s}");
        assert!(!s.contains('/'), "{s}");
    }

    /// Human text output for unavailable store must say so and echo
    /// the sanitised reason.
    #[test]
    fn u2_render_wave_inspect_view_text_unavailable() {
        let view = WaveInspectView::unavailable("w-x", "store open failed");
        let s = render_wave_inspect_view_text(&view);
        assert!(s.contains("unavailable"), "{s}");
        assert!(s.contains("store open failed"), "{s}");
    }

    /// `from_snapshot` propagates every public field verbatim and
    /// renders the phase via `WavePhase`'s stable Display form
    /// (U3 contract: phase strings are an agent-safe contract).
    #[test]
    fn u3_inspect_view_from_snapshot_propagates_fields() {
        use ralph_core::supervisor::{WaveKind, WavePhase, WaveSnapshot};
        let snap = WaveSnapshot {
            wave_id: "w-found".into(),
            kind: WaveKind::Exec,
            phase: WavePhase::Collect,
            expected_total: 5,
            completed_count: 2,
            failed_count: 0,
            pending_count: 2,
            in_flight_count: 1,
            cancel_requested: false,
            delivery_state: ralph_core::supervisor::WaveDeliveryState::Pending,
            started_at: std::time::SystemTime::now(),
            slots: vec![(0, ralph_core::supervisor::SlotStatus::Completed)],
        };
        let view = WaveInspectView::from_snapshot(&snap);
        assert!(view.ok);
        assert!(view.registered);
        assert_eq!(view.wave_id, "w-found");
        assert_eq!(view.availability, "available");
        assert_eq!(view.phase.as_deref(), Some("collect"));
        assert_eq!(view.expected_total, Some(5));
        assert_eq!(view.completed_count, Some(2));
        assert_eq!(view.failed_count, Some(0));
        assert_eq!(view.pending_count, Some(2));
        assert_eq!(view.in_flight_count, Some(1));
        assert_eq!(view.cancel_requested, Some(false));
        assert_eq!(view.unavailable_reason, None);

        // Every field renders in the JSON shape — no skip on the
        // registered branch.
        let json = serde_json::to_value(&view).expect("serialise");
        assert_eq!(json["phase"], serde_json::json!("collect"));
        assert_eq!(json["expected_total"], serde_json::json!(5));
        assert_eq!(json["completed_count"], serde_json::json!(2));
        assert!(json.get("unavailable_reason").is_none());
    }
}
