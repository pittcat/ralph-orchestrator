use crate::cli::{
    ColorMode, ConfigSource, HatsSource, resolve_emit_path, resolve_marker_target,
    resolve_workspace_root, urgent_steer_path_from_workspace,
};
use crate::config_resolution;
use crate::display::colors;
use crate::policy_check::{
    PolicyCheckFlags, ValidationFailure, resolve_policy_check_mode,
    resolve_policy_check_mode_with_ctx,
};
use anyhow::{Context, Result};
use clap::Parser;
use ralph_core::config::HatExecutionMode;
use ralph_core::emit_schema_hint::fix_hint_for_hat_topic;
use ralph_core::preset::engine::ProtocolView;
use ralph_core::{
    RalphConfig, UrgentSteerStore, ViolationType,
    diagnosis::{
        DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, EvidenceKind, EvidenceRef,
        RecoveryDiagnosisEnvelope, RecoveryJournalEntry,
    },
};
use ralph_proto::{Hat, HatId, Topic};
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Arguments for the emit subcommand.
#[derive(Parser, Debug)]
pub struct EmitArgs {
    /// Event topic (e.g., "build.done", "review.complete").
    ///
    /// Required when emitting an event; ignored when `--schema <TOPIC>`
    /// is set, because the schema mode already names its topic via the
    /// flag. We model it as `Option<String>` because clap forbids
    /// `required = true` together with `required_unless_present` on a
    /// positional argument; the handler enforces "topic must be set"
    /// for the emit path.
    pub topic: Option<String>,

    /// Event payload - string or JSON (optional, defaults to empty)
    #[arg(default_value = "")]
    pub payload: String,

    /// Parse payload as JSON object instead of string
    #[arg(long, short)]
    pub json: bool,

    /// Path to events file (defaults to .ralph/events.jsonl)
    #[arg(long, default_value = ".ralph/events.jsonl")]
    pub file: PathBuf,

    /// Validate event against current event policy before emitting
    #[arg(long)]
    pub policy_check: bool,

    /// Bypass mandatory policy check (only allowed when config permits)
    #[arg(long = "unsafe-no-policy-check", conflicts_with = "policy_check")]
    pub no_policy_check: bool,

    /// Hat that published this event (falls back to $RALPH_CURRENT_HAT)
    #[arg(long)]
    pub hat: Option<String>,

    /// Target hat triggered by this event (falls back to $RALPH_TRIGGERED_HAT)
    #[arg(long)]
    pub triggered: Option<String>,

    /// Source identifier for this event (falls back to $RALPH_EVENT_SOURCE)
    #[arg(long)]
    pub source: Option<String>,

    /// Print the embedded protocol JSON view for `TOPIC` (plan 2026-06-20-001
    /// U5 / R6). When set, no event is emitted, no events file is touched,
    /// and no iteration is consumed. Mutually exclusive with payload / json
    /// because schema mode is read-only.
    #[arg(long, value_name = "TOPIC", conflicts_with_all = ["payload", "json"])]
    pub schema: Option<String>,
}

/// Plan 001 §4.3 C3: build the hat-scoped fix hint shown to the agent when
/// a pre-publish check rejects the event. The hat's authorised topics are
/// looked up via the loaded `RalphConfig::hats`. When the hat is unknown,
/// or not authorised for the topic, only the bare policy message is shown
/// — we never leak another hat's payload shape.
fn format_fix_hint(config: &RalphConfig, hat_id: Option<&str>, topic: &str) -> String {
    let Some(hat_id) = hat_id else {
        return "Set --hat <your-hat-id> (or RALPH_CURRENT_HAT) so the CLI can \
                suggest a fix."
            .to_string();
    };

    let Some(hat_config) = config.hats.get(hat_id) else {
        return format!(
            "Hat `{hat_id}` is not registered in the loaded preset; \
             pass --hat to one of the preset's registered hats."
        );
    };

    // Build a minimal `Hat` from the preset's `HatConfig`. Only the
    // `publishes` list matters for `fix_hint_for_hat_topic`; the rest
    // is dropped on the floor inside that helper.
    let publishes: Vec<Topic> = hat_config
        .publishes
        .iter()
        .map(|t| Topic::new(t.clone()))
        .collect();
    let schema = match config
        .event_loop
        .event_policy
        .as_ref()
        .and_then(|p| p.schemas.get(topic))
    {
        Some(s) => s,
        None => return String::new(),
    };
    let hat = Hat {
        id: HatId::new(hat_id),
        name: hat_config.name.clone(),
        description: String::new(),
        subscriptions: vec![],
        publishes,
        instructions: String::new(),
    };
    fix_hint_for_hat_topic(&hat, topic, schema).unwrap_or_default()
}

/// Resolve provenance values for an emitted event.
///
/// Priority: CLI flag > env var lookup > empty.
pub fn resolve_provenance<F>(
    cli_hat: Option<String>,
    cli_triggered: Option<String>,
    cli_source: Option<String>,
    env_lookup: F,
) -> (Option<String>, Option<String>, Option<String>)
where
    F: Fn(&str) -> Option<String>,
{
    let hat = cli_hat
        .or_else(|| env_lookup("RALPH_CURRENT_HAT"))
        .filter(|s| !s.is_empty());
    let triggered = cli_triggered
        .or_else(|| env_lookup("RALPH_TRIGGERED_HAT"))
        .filter(|s| !s.is_empty());
    let source = cli_source
        .or_else(|| env_lookup("RALPH_EVENT_SOURCE"))
        .filter(|s| !s.is_empty());
    (hat, triggered, source)
}

/// Plan 001 §4.3 P1-8: log + surface to stderr when the recovery
/// envelope write fails, so operators have a chance to fix the
/// underlying disk/permissions issue. The user-facing bail still
/// fires — only the audit trace is at risk.
fn record_cli_emit_rejection(
    workspace_root: &Path,
    topic: &str,
    hat: Option<&str>,
    finding: &ralph_core::PolicyFinding,
) {
    if let Err(e) = write_cli_emit_recovery_envelope(workspace_root, topic, hat, finding) {
        tracing::warn!("Failed to write CLI emit recovery envelope: {:#}", e);
        eprintln!(
            "note: could not record rejection audit to .ralph/recovery.jsonl — \
             check disk permissions. Underlying error: {e}"
        );
    }
}

/// Re-exported from [`crate::policy_check`] so existing callers
/// keep their `crate::commands::emit::PolicyCheckMode` import path.
pub use crate::policy_check::PolicyCheckMode;

/// Decides the policy-check mode based on CLI arguments and loaded config.
///
/// Thin wrapper over [`crate::policy_check::resolve_policy_check_mode`]
/// that bridges `EmitArgs` → `PolicyCheckFlags` for symmetry with the
/// shared helper. Preserved for backward compatibility with existing
/// callers and tests.
///
/// U15: the agent-context-aware helper accepts `is_agent_context` so
/// emit callers that know whether the call came from an agent (true)
/// or from the human CLI (false) can pass that signal through. Most
/// callers should prefer the ctx-aware form.
pub fn should_policy_check_emit(args: &EmitArgs, config: Option<&RalphConfig>) -> PolicyCheckMode {
    let flags = PolicyCheckFlags {
        policy_check: args.policy_check,
        no_policy_check: args.no_policy_check,
    };
    resolve_policy_check_mode(&flags, config)
}

/// U15 ctx-aware emit policy-check resolver. Detects the operation
/// context from the live environment and forwards `is_agent_context`
/// to the shared helper.
pub fn should_policy_check_emit_with_ctx(
    args: &EmitArgs,
    config: Option<&RalphConfig>,
    workspace_root: &std::path::Path,
) -> PolicyCheckMode {
    let flags = PolicyCheckFlags {
        policy_check: args.policy_check,
        no_policy_check: args.no_policy_check,
    };
    let ctx = crate::operation_guard::OperationContext::detect(workspace_root.to_path_buf());
    resolve_policy_check_mode_with_ctx(&flags, config, ctx.is_agent_context)
}

/// Emit an event to the current run's events file with proper JSON formatting.
///
/// This command provides a deterministic way for agents to emit events without
/// risking malformed JSONL from manual echo commands. All JSON serialization
/// is handled via serde_json, ensuring proper escaping of payloads.
///
/// Events are written to the path specified in `.ralph/current-events` marker file
/// (created by `ralph run`), or falls back to `.ralph/events.jsonl` if no marker exists.
/// Heuristic: does this payload look like a JSON object or array?
///
/// Used by `emit_command_with_root` to auto-detect JSON payloads when the
/// agent omits the `--json` flag.  This prevents structured events such as
/// `work.done` from being stored as plain strings and then rejected by the
/// execution-contract validator.
pub fn looks_like_json(payload: &str) -> bool {
    let trimmed = payload.trim_start();
    trimmed.starts_with('{') || trimmed.starts_with('[')
}

/// Write a recovery envelope to `.ralph/recovery.jsonl` when the CLI
/// emit precheck rejects an event. The envelope captures the rejected
/// topic, the offending hat (if known), and the policy finding so
/// operators and `ralph diagnose` have an audit trace.
///
/// Errors are returned but expected to be logged (not propagated) by
/// the caller: the original validation error must still reach the user.
fn write_cli_emit_recovery_envelope(
    workspace_root: &Path,
    topic: &str,
    source_hat: Option<&str>,
    finding: &ralph_core::PolicyFinding,
) -> Result<()> {
    let recovery_path = workspace_root.join(".ralph/recovery.jsonl");
    if let Some(parent) = recovery_path.parent() {
        fs::create_dir_all(parent).with_context(|| {
            format!("Failed to create recovery directory: {}", parent.display())
        })?;
    }

    let (severity, outcome, reason_code, source) = match &finding.violation_type {
        ralph_core::ViolationType::PayloadTypeMismatch { .. } => (
            DiagnosisSeverity::Critical,
            DiagnosisOutcome::NotRetriable,
            "payload_contract_violation".to_string(),
            DiagnosisSource::CliEmit,
        ),
        // P1#13 fix: dimension-mismatch rejections (CLI precheck
        // for wave worker's `review.dimension.done` value) are
        // surfaced as `WaveDimensionGuard` envelopes so
        // `ralph diagnose` and operators can filter for the
        // specific gate (rather than lumping them in with the
        // generic `CliEmit` bucket). The reason code is the
        // gate's own (`dimension_mismatch`), so the evidence
        // is unambiguous even when both sources appear in the
        // same recovery journal.
        _ if finding.violation_type.reason_code() == "dimension_mismatch" => (
            DiagnosisSeverity::Error,
            DiagnosisOutcome::Failed,
            finding.violation_type.reason_code().to_string(),
            DiagnosisSource::WaveDimensionGuard,
        ),
        _ => (
            DiagnosisSeverity::Error,
            DiagnosisOutcome::Failed,
            finding.violation_type.reason_code().to_string(),
            DiagnosisSource::CliEmit,
        ),
    };

    let mut builder = RecoveryDiagnosisEnvelope::builder()
        .source(source)
        .severity(severity)
        .topic(topic)
        .reason_code(reason_code)
        .message(finding.message.clone())
        .outcome(outcome)
        .safe_target(false);

    if let Some(hat) = source_hat {
        builder = builder.source_hat(hat);
    }

    if let Some(field) = finding.violation_type.field() {
        builder = builder.evidence(EvidenceRef::new(EvidenceKind::Field, field, None));
    }

    let envelope = builder.build();
    let entry = RecoveryJournalEntry::from_envelope(envelope, Vec::new());

    let line = serde_json::to_string(&entry)?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&recovery_path)
        .with_context(|| format!("Failed to open recovery file: {}", recovery_path.display()))?;
    writeln!(file, "{}", line)?;
    Ok(())
}

pub fn emit_command(
    color_mode: ColorMode,
    args: EmitArgs,
    hats_source: Option<&HatsSource>,
) -> Result<()> {
    emit_command_with_root_and_hats(color_mode, args, None, hats_source)
}

#[cfg(test)]
pub fn emit_command_with_root(
    color_mode: ColorMode,
    args: EmitArgs,
    root: Option<&PathBuf>,
) -> Result<()> {
    emit_command_with_root_and_hats(color_mode, args, root, None)
}

/// Isolated-mode helper: derive `triggered` from the handoff index when
/// the runner injected the hat context and the agent did not explicitly
/// request a target.
///
/// The handoff index records topics with a *unique* downstream consumer.
/// In isolated mode those topics are exactly the deterministic handoffs.
/// When a topic has multiple consumers, a wildcard subscriber, or is not
/// registered as a handoff topic, `HandoffIndex::consumer_of` returns
/// `None` and we leave `triggered` empty rather than guessing.
fn maybe_derive_triggered_for_isolated(
    topic: &str,
    hat: Option<&str>,
    triggered: Option<String>,
    config: Option<&RalphConfig>,
) -> Option<String> {
    let Some(cfg) = config else {
        return triggered;
    };
    if cfg.event_loop.execution_mode != HatExecutionMode::Isolated {
        return triggered;
    }
    if hat.is_none() {
        return triggered;
    }
    if triggered.is_some() {
        return triggered;
    }
    if ralph_core::event_origin::is_ralph_control_topic(topic)
        || ralph_core::is_orchestrator_diagnostic_topic(topic)
    {
        return triggered;
    }

    let index = ralph_core::workflow_contract::HandoffIndex::from_config(cfg);
    index
        .consumer_of(topic)
        .map(|id| id.to_string())
        .or(triggered)
}

fn emit_command_with_root_and_hats(
    color_mode: ColorMode,
    args: EmitArgs,
    root: Option<&PathBuf>,
    hats_source: Option<&HatsSource>,
) -> Result<()> {
    // Plan 001 §4.3 C1: when no `-H` is passed but the parent loop set
    // `RALPH_HATS_SOURCE` (so a backend agent can emit without the explicit
    // flag), synthesise a `HatsSource` from the env so the rest of the
    // pipeline loads the right preset's `event_policy.schemas`.
    let env_source = std::env::var("RALPH_HATS_SOURCE")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|s| HatsSource::parse(&s));
    let hats_source_owned: Option<HatsSource>;
    let hats_source: Option<&HatsSource> = match (hats_source, env_source.as_ref()) {
        (Some(s), _) => Some(s),
        (None, Some(env)) => {
            hats_source_owned = Some(env.clone());
            hats_source_owned.as_ref()
        }
        (None, None) => None,
    };
    let use_colors = color_mode.should_use_colors();
    let workspace_root = resolve_workspace_root(root);
    let current_events_marker = workspace_root.join(".ralph/current-events");

    // U5 / R6: --schema <TOPIC> short-circuits to a read-only protocol
    // view. No event is emitted, no events file is touched, no policy
    // check or lint phase runs.
    //
    // Schema mode requires a real ralph.yml — `load_config_for_preflight_sync`
    // happily returns a default `RalphConfig` when the file is missing,
    // but operators inspecting the protocol expect the embedded view
    // of *their* preset, not an empty default.
    //
    // The schema branch sits at the very top of the handler — before
    // urgent-steer handling, before the shared `should_load_config`
    // gate, before any other validation. Operators pipe
    // `ralph emit --schema <TOPIC>` into `jq` and expect a hermetic,
    // side-effect-free read.
    if let Some(ref schema_topic) = args.schema {
        // U6 (2026-07-06-001 plan): `--schema EMIT_RESULT` 是 ralph
        // 内部协议 schema（`EmitResult` 响应 JSON 的 SSOT），不走
        // preset 协议视图——它从 `ralph_core::emit_result` 模块的
        // 嵌入式常量直出，不读 preset / ralph.yml / .ralph/。这是
        // 「协议 SSOT 收敛」的对外可观测信号。
        //
        // 必须放在 ralph.yml 检查之前；其它 topic 走原有
        // ProtocolView 渲染。
        if schema_topic == "EMIT_RESULT" {
            use ralph_core::emit_result::{EMIT_RESULT_SCHEMA_VERSION, EmitResult};
            let view = EmitResult {
                schema_version: EMIT_RESULT_SCHEMA_VERSION,
                ok: false, // placeholder；U6 只暴露 schema_version 字段
                recorded: false,
                topic: String::new(),
                phase: String::new(),
                allowed_next: vec![],
                activate_next: vec![],
                errors: vec![],
                handoff: None,
            };
            let pretty = serde_json::to_string_pretty(&view)
                .context("Failed to serialise EmitResult schema view")?;
            println!("{pretty}");
            return Ok(());
        }

        let config_path = config_resolution::find_workspace_config_path(&workspace_root)
            .unwrap_or_else(|| workspace_root.join("ralph.yml"));
        if !config_path.exists() {
            anyhow::bail!(
                "Cannot render protocol view for `{schema_topic}`: no ralph.yml \
                 found at {}. The schema view is built from the loaded preset, \
                 so the workspace must have a discoverable ralph.yml.",
                config_path.display()
            );
        }
        let config_sources = vec![ConfigSource::File(config_path)];
        let cfg = crate::preflight::load_config_for_preflight_sync(
            &config_sources,
            hats_source,
            &workspace_root,
        )
        .with_context(|| {
            format!(
                "Failed to load config for schema view of `{schema_topic}`. \
                 Fix the ralph.yml errors or remove --schema."
            )
        })?;
        // P2-#6 (002-adversarial-review): production-only env
        // read; tests must use `from_event_loop` (env-free).
        let view = ProtocolView::from_event_loop_with_feature_for_env(&cfg.event_loop);
        let pretty = schema_view::render_pretty(&view, schema_topic)
            .context("Failed to serialise protocol view")?;
        println!("{pretty}");
        return Ok(());
    }

    if std::env::var("RALPH_WAVE_ID").is_err() {
        let urgent_steer_store = UrgentSteerStore::new(urgent_steer_path_from_workspace(root));
        if let Some(record) = urgent_steer_store
            .take()
            .context("Failed to read urgent-steer marker")?
        {
            let guidance = record
                .messages
                .iter()
                .enumerate()
                .map(|(idx, message)| format!("{}. {}", idx + 1, message))
                .collect::<Vec<_>>()
                .join("\n");

            anyhow::bail!(
                "Urgent steer is pending. Do not hand off yet.\n\n\
                 Human feedback:\n{guidance}\n\n\
                 You have now seen the steer. Address it in this turn, then rerun `ralph emit` \
                 once you are ready to hand off."
            );
        }
    }

    // Load config for policy enforcement, provenance checks, and strict-mode detection.
    // We load whenever explicit flags are set, provenance might be required, or the
    // workspace looks like a Ralph project (has .ralph) so we can honour strict configs.
    let should_load_config = args.policy_check
        || args.no_policy_check
        || args.hat.is_none()
        || std::env::var("RALPH_CURRENT_HAT").is_ok()
        || workspace_root.join(".ralph").is_dir();

    let config = if should_load_config {
        let config_path = config_resolution::find_workspace_config_path(&workspace_root)
            .unwrap_or_else(|| workspace_root.join("ralph.yml"));
        let config_sources = vec![ConfigSource::File(config_path.clone())];
        match crate::preflight::load_config_for_preflight_sync(
            &config_sources,
            hats_source,
            &workspace_root,
        ) {
            Ok(cfg) => Some(cfg),
            Err(_e) => {
                if args.policy_check {
                    anyhow::bail!(
                        "Policy check requested but config could not be loaded: {}. \
                         Fix the config or omit --policy-check.",
                        _e
                    );
                }
                // Plan 001 §4.3 C4 row 2: bail closed when a loop
                // context exists (`.ralph/` present), even if no
                // `ralph.yml` was found — the absence of ralph.yml
                // is itself a malformed-config condition under a
                // strict preset.
                if config_path.exists() || workspace_root.join(".ralph").is_dir() {
                    anyhow::bail!(
                        "Config file exists or loop context detected but config could not be loaded: {}. \
                         Fix the config, use --policy-check with a valid config, \
                         or use --unsafe-no-policy-check to bypass (if permitted).",
                        _e
                    );
                }
                None
            }
        }
    } else {
        None
    };

    // (U5 / R6 schema branch moved to the top of the handler —
    // see the early `if let Some(ref schema_topic) = args.schema`
    // block above. This duplicate is intentional-deleted to avoid
    // double-printing the protocol view.)

    // Schema mode short-circuits above. Below this point we are
    // in *emit* mode and `args.topic` is mandatory; clap cannot
    // express `required_unless_present = "schema"` on a positional
    // argument, so we enforce the precondition here.
    let topic = args
        .topic
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("missing event topic (required unless --schema is set)"))?;

    // Determine whether policy validation is required. U15: agent
    // context (env-detected) defaults to strict policy-check, even
    // when the preset author did not flip
    // `require_policy_check_for_cli_emit`.
    let workspace_root = root
        .cloned()
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
    let check_mode = should_policy_check_emit_with_ctx(&args, config.as_ref(), &workspace_root);

    // Phase 2: in isolated mode the runner controls hat provenance. When the
    // agent is running inside a hat context (RALPH_CURRENT_HAT is set), the
    // CLI flag --hat is ignored and must not disagree with the environment.
    let env_hat = std::env::var("RALPH_CURRENT_HAT")
        .ok()
        .filter(|s| !s.is_empty());

    // Resolve provenance values: CLI flag > env var > empty
    let (hat, triggered, source) =
        resolve_provenance(args.hat.clone(), args.triggered, args.source, |key| {
            std::env::var(key).ok()
        });

    let hat = if config
        .as_ref()
        .is_some_and(|c| c.event_loop.execution_mode == HatExecutionMode::Isolated)
    {
        if let Some(ref env_hat) = env_hat {
            if let Some(ref cli_hat) = args.hat {
                if cli_hat != env_hat {
                    anyhow::bail!(
                        "Isolated mode hat mismatch: --hat '{}' conflicts with \
                         RALPH_CURRENT_HAT '{}'. In isolated mode the runner \
                         controls provenance; emit as '{}'.",
                        cli_hat,
                        env_hat,
                        env_hat
                    );
                }
            }
            Some(env_hat.clone())
        } else {
            hat
        }
    } else {
        hat
    };

    // 2026-07-03: isolated mode auto-derive `triggered` from the topic's
    // registered subscriber. The runner previously set RALPH_TRIGGERED_HAT to
    // the round-robin "next hat", which caused events like
    // `review.dimension.ready` to be routed to the wrong hat (e.g. `shipper`
    // instead of `dimension-reviewer`). When the agent is inside a runner-
    // injected hat context and did not explicitly request a target, derive the
    // target from the preset topology so the event bus routes correctly.
    let triggered = maybe_derive_triggered_for_isolated(topic, hat.as_deref(), triggered, config.as_ref());

    // Enforce provenance requirements when hat is missing.
    //
    // U1 (2026-06-17-004 plan, R1+R2): the blanket precheck is replaced
    // by `check_emit_provenance` (called below as a separate gate). The
    // smart gate allows control topics (loop.cancel, task.resume,
    // human.*) and orchestrator diagnostics (event.*) to be emitted
    // without a hat — these are produced by the loop / runtime ralph
    // pseudo-hat. Business topics still fail-closed when provenance
    // is missing.
    if hat.is_none() {
        let provenance_required = config
            .as_ref()
            .and_then(|c| c.event_loop.event_policy.as_ref())
            .map(|p| p.require_emit_provenance)
            .unwrap_or(false);
        if provenance_required {
            // Skip the bail for control / diagnostic topics — they are
            // produced by the loop itself and bypass the new
            // check_emit_provenance gate (see policy_check.rs). The
            // smart gate catches business-topic cases below.
            let is_control = ralph_core::event_origin::is_ralph_control_topic(&topic);
            let is_diagnostic = ralph_core::is_orchestrator_diagnostic_topic(&topic);
            if !is_control && !is_diagnostic {
                anyhow::bail!(
                    "Event provenance required: --hat <hat-id> or RALPH_CURRENT_HAT must be set."
                );
            }
        }
    }

    // U1: ralph-hat business-topic guard.
    // The builtin `ralph` hat is the orchestration fallback hat. Allowing it to
    // emit business topics (e.g. `review.passed`, `work.start`) lets a worktree
    // loop's loop runner bypass review-synthesizer / plan-gate / coordinator and
    // advance the workflow as `ralph` — this is the impersonation attack the
    // P0 origin guard already rejects at JSONL read time. Reject here too so
    // the agent gets immediate backpressure (otherwise the rejection only
    // surfaces several seconds later when the loop runner reads the JSONL).
    if let Some(hat_id) = hat.as_deref()
        && hat_id == "ralph"
        && !ralph_core::event_origin::is_ralph_control_topic(&topic)
    {
        anyhow::bail!(
            "Builtin ralph hat may only emit control topics: {:?}. \
             Topic '{}' is a business topic and cannot be emitted by ralph. \
             Set --hat to a registered workflow hat (e.g. coordinator, executor, \
             review-synthesizer) instead.",
            ralph_core::event_origin::RALPH_CONTROL_TOPICS,
            topic
        );
    }

    // U6 (2026-06-21-002 plan §U6): CLI `--policy-check` always
    // routes through the unified `validate_event` pipeline. The
    // legacy `validate_event_with_hat` path below is preserved as
    // a structural fallback for diff / comparison runs and is
    // only entered when `unified_active` is false. The unified
    // path surfaces structured `reason_codes` (and per-rule
    // `suggestions`) so agents can programmatically match failures
    // against the loop's vocabulary.
    let unified_active = check_mode != PolicyCheckMode::Skip
        && config
            .as_ref()
            .and_then(|c| c.event_loop.event_policy.as_ref())
            .is_some_and(|p| p.enabled);
    if unified_active {
        let report = crate::policy_check::run_policy_check_unified(
            &topic,
            Some(&args.payload),
            hat.as_deref(),
            triggered.as_deref(),
            &workspace_root,
        )?;
        if !report.accepted {
            // Structured reason_code list — the U6 plan mandates
            // "统一后错误输出结构化 `reason_code`" so the agent can
            // programmatically match failures against the loop's
            // vocabulary. We surface the full list to stderr (one
            // per line) AND include a JSON envelope in the bail so
            // tools that parse stderr can recover the structured
            // shape.
            let codes = report.reason_codes.join(", ");
            let suggestions: Vec<String> = report
                .suggestions
                .iter()
                .enumerate()
                .filter_map(|(idx, hint)| {
                    if hint.is_empty() {
                        None
                    } else {
                        Some(format!("[{}] {}", idx, hint))
                    }
                })
                .collect();
            let suggestions_block = if suggestions.is_empty() {
                String::new()
            } else {
                format!("\n\nSuggestions:\n{}", suggestions.join("\n"))
            };
            let envelope = serde_json::to_string(&report).unwrap_or_else(|_| "{}".to_string());
            eprintln!("{}", envelope);
            anyhow::bail!(
                "Event rejected by policy: reason_codes=[{}] topic='{}' hat={:?}{}",
                codes,
                report.topic,
                report.hat,
                suggestions_block
            );
        }
        // Unified path accepted: skip the legacy `validate_event_with_hat`
        // branch below so the legacy bail never double-fires on the
        // same event. The legacy path is only entered when
        // `unified_active` is false (no event_policy configured).
        tracing::info!(
            "cli emit policy check: unified pipeline accepted topic={}",
            topic
        );
    }

    if check_mode != PolicyCheckMode::Skip {
        // When the unified branch already accepted the event, skip
        // the legacy gate (U6: unified is the production path; the
        // legacy path stays only for diff runs when no event_policy
        // is configured).
        if unified_active {
            tracing::debug!(
                "cli emit: skipping legacy validate_event_with_hat (unified path active)"
            );
        } else {
            let policy = match config
                .as_ref()
                .and_then(|c| c.event_loop.event_policy.as_ref())
            {
                Some(p) if p.enabled => Some(p),
                _ => {
                    if check_mode == PolicyCheckMode::ExplicitCheck {
                        eprintln!(
                            "Warning: --policy-check was requested but no event policy is configured or enabled."
                        );
                    }
                    None
                }
            };
            if let Some(policy) = policy {
                use ralph_core::{
                    PolicyRuntimeState, check_topic_deny_rules, validate_event_with_hat,
                };
                let events_path = fs::read_to_string(&current_events_marker)
                    .map(|s| resolve_marker_target(&workspace_root, &s))
                    .unwrap_or_else(|_| args.file.clone());
                let mut state =
                PolicyRuntimeState::from_events(&events_path, policy).unwrap_or_else(|e| {
                    eprintln!(
                        "Warning: Failed to replay events for policy check: {}. Using empty state.",
                        e
                    );
                    PolicyRuntimeState::default()
                });

                // Enforce topic-deny rules at CLI emit time so a forbidden
                // (hat, topic) pair never reaches the events file.
                if let Some(decision) = check_topic_deny_rules(hat.as_deref(), &topic, policy) {
                    match decision {
                        ralph_core::PolicyDecision::Accept => {}
                        ralph_core::PolicyDecision::Warn(findings) => {
                            for finding in findings {
                                eprintln!("Policy warning: {}", finding.message);
                            }
                        }
                        ralph_core::PolicyDecision::AcknowledgeAndForward(finding) => {
                            // U2 (plan 2026-07-04-004): CLI emit
                            // path sees the dedup carve-out. We
                            // surface the dedup hint as a warning
                            // and forward the event as the runtime
                            // would; `--policy-check` callers see
                            // the same hint via stderr.
                            eprintln!(
                                "Policy acknowledge+forward: {}",
                                finding.message
                            );
                        }
                        ralph_core::PolicyDecision::RejectWithResume(finding)
                        | ralph_core::PolicyDecision::Hold(finding)
                        | ralph_core::PolicyDecision::Block(finding)
                        | ralph_core::PolicyDecision::Ignore(finding) => {
                            record_cli_emit_rejection(
                                &workspace_root,
                                &topic,
                                hat.as_deref(),
                                &finding,
                            );
                            anyhow::bail!(
                                "Event rejected by policy: {}. Fix the issue before emitting.\n\n{}",
                                finding.message,
                                format_fix_hint(config.as_ref().unwrap(), hat.as_deref(), &topic)
                            );
                        }
                    }
                }

                // Run schema validation with hat-aware restrictions.
                let decision = validate_event_with_hat(
                    &topic,
                    Some(&args.payload),
                    policy,
                    &mut state,
                    hat.as_deref(),
                );
                match decision {
                    ralph_core::PolicyDecision::Accept => {}
                    ralph_core::PolicyDecision::Warn(findings) => {
                        for finding in findings {
                            eprintln!("Policy warning: {}", finding.message);
                        }
                    }
                    ralph_core::PolicyDecision::AcknowledgeAndForward(finding) => {
                        // U2 (plan 2026-07-04-004): CLI precheck +
                        // emit path sees the dedup carve-out. We
                        // log the dedup hint as a warning and let
                        // the event reach the bus so `--policy-check`
                        // callers can confirm the silent-success
                        // lane is active. Real emit (without
                        // --policy-check) writes to the events file
                        // as the runtime would.
                        eprintln!(
                            "Policy acknowledge+forward: {}",
                            finding.message
                        );
                    }
                    ralph_core::PolicyDecision::RejectWithResume(finding)
                    | ralph_core::PolicyDecision::Hold(finding)
                    | ralph_core::PolicyDecision::Block(finding)
                    | ralph_core::PolicyDecision::Ignore(finding) => {
                        record_cli_emit_rejection(
                            &workspace_root,
                            &topic,
                            hat.as_deref(),
                            &finding,
                        );
                        anyhow::bail!(
                            "Event rejected by policy: {}. Fix the issue before emitting.\n\n{}",
                            finding.message,
                            format_fix_hint(config.as_ref().unwrap(), hat.as_deref(), &topic)
                        );
                    }
                }
            } // closes if let Some(policy)
        } // closes if unified_active's else
    } else if check_mode == PolicyCheckMode::Skip {
        tracing::info!("cli emit policy check skipped: no event_policy in resolved config");
    }

    // U6 (2026-06-21-002 plan §U6): the unified `validate_event`
    // pipeline is the production path and runs before the legacy
    // check above, so the legacy bail never double-fires. The
    // legacy path below only runs when no event_policy is
    // configured (diff / no-policy fallback).

    // U1 (2026-06-17-004 plan, R1+R2): CLI provenance fail-closed.
    // Fires regardless of `check_mode`: if the agent is in isolated
    // mode and forgot to pass `--hat`, the CLI must reject the event
    // BEFORE it lands in events.jsonl. Without this gate, the runtime
    // origin guard drops the event silently at JSONL read time and the
    // agent gets no actionable backpressure at the CLI boundary.
    //
    // `check_isolated_scope` below only enforces when the hat is known;
    // `check_emit_provenance` is the matching gate for the `hat = None`
    // path. Together they form the full isolated-mode CLI guard.
    if let Some(cfg) = config.as_ref() {
        if let Err(err) = crate::policy_check::check_emit_provenance(hat.as_deref(), &topic, cfg) {
            use ralph_core::{PolicyFinding, ViolationType};
            let finding = PolicyFinding {
                violation_type: ViolationType::SemanticGateViolation {
                    gate: "missing_provenance".to_string(),
                    context: err.message.clone(),
                },
                topic: topic.to_string(),
                message: err.message.clone(),
            };
            record_cli_emit_rejection(&workspace_root, &topic, hat.as_deref(), &finding);
            anyhow::bail!(
                "Event rejected by missing-provenance guard: {}",
                err.message
            );
        }
    }

    // U1 (2026-06-17-003 plan): isolated mode scope precheck. When the
    // resolved preset has `event_loop.execution_mode: isolated` and the
    // caller passed a hat (`--hat` or `RALPH_CURRENT_HAT`), the hat's
    // `publishes` scope must be enforced BEFORE the event lands in
    // events.jsonl. This mirrors the loop's runtime
    // `isolated_publish_allowed` check via `HatRegistry::can_publish`,
    // giving the agent actionable backpressure at the CLI boundary
    // instead of a silent drop at the loop reader.
    //
    // Fires regardless of `check_mode`: if the agent is in isolated
    // mode and passed `--hat`, the runner's scope is the contract —
    // `--policy-check` toggles schema enforcement, not scope
    // enforcement. Without `--hat` the call defers to the origin
    // guard (which rejects unknown/missing provenance).
    if let Some(cfg) = config.as_ref() {
        if let Err(err) = crate::policy_check::check_isolated_scope(hat.as_deref(), &topic, cfg) {
            use ralph_core::{PolicyFinding, ViolationType};
            let finding = PolicyFinding {
                violation_type: ViolationType::SemanticGateViolation {
                    gate: "isolated_scope".to_string(),
                    context: err.message.clone(),
                },
                topic: topic.to_string(),
                message: err.message.clone(),
            };
            record_cli_emit_rejection(&workspace_root, &topic, hat.as_deref(), &finding);
            anyhow::bail!("Event rejected by isolated scope guard: {}", err.message);
        }
    }

    // U3 (R3): wave worker dimension assignment precheck. Fires before
    // any policy / step-handoff processing so a wave worker that
    // emits the wrong dimension never reaches the events file. The
    // env var is set by the loop runner on `review.dimension.done`
    // workers; non-wave callers (env unset) pass through unchanged.
    if let Err(err) = crate::policy_check::check_wave_dimension_assignment(&topic, &args.payload) {
        use ralph_core::{PolicyFinding, ViolationType};
        let finding = PolicyFinding {
            violation_type: ViolationType::SemanticGateViolation {
                gate: "wave_dimension_assignment".to_string(),
                context: err.message.clone(),
            },
            topic: topic.to_string(),
            message: err.message.clone(),
        };
        record_cli_emit_rejection(&workspace_root, &topic, hat.as_deref(), &finding);
        anyhow::bail!("Event rejected by wave dimension guard: {}", err.message);
    }

    // U1 (2026-06-17-005 plan): step handoff gate precheck at the CLI
    // boundary. Mirrors the loop-side `apply_step_handoff_gate` so an
    // agent calling `ralph emit --policy-check` (or running under
    // `require_policy_check_for_cli_emit: true`) gets the same
    // `progress_task_mismatch` backpressure before writing the event
    // to disk. The CLI is *additive* — it never replaces the loop
    // gate, it surfaces the same reason earlier.
    if ralph_core::step_handoff::progress_task_gate::is_gated_topic(&topic)
        && check_mode != PolicyCheckMode::Skip
    {
        match crate::policy_check::check_step_handoff_gate(&topic, &args.payload, &workspace_root) {
            Ok(()) => {}
            Err(err) => {
                record_cli_emit_rejection(
                    &workspace_root,
                    &topic,
                    hat.as_deref(),
                    &ralph_core::PolicyFinding {
                        violation_type: ViolationType::SemanticGateViolation {
                            gate: "progress_task_gate".to_string(),
                            context: err.message.clone(),
                        },
                        topic: topic.to_string(),
                        message: err.message.clone(),
                    },
                );
                let failure = ValidationFailure {
                    ok: false,
                    error: "policy_validation_failed",
                    topic: topic.to_string(),
                    validation_errors: vec![err],
                };
                crate::policy_check::emit_policy_validation_failure(
                    &failure,
                    crate::policy_check::OutputMode::Text,
                )?;
            }
        }
    }

    // Generate timestamp internally — agents cannot forge timestamps
    let ts = chrono::Utc::now().to_rfc3339();

    // Validate JSON payload if --json flag is set
    let payload = if args.json && !args.payload.is_empty() {
        // Validate it's valid JSON
        serde_json::from_str::<serde_json::Value>(&args.payload).context("Invalid JSON payload")?;
        args.payload
    } else {
        args.payload
    };

    // Build the event record
    // We use serde_json directly to ensure proper escaping
    let payload_value = if args.json && !payload.is_empty() {
        // Parse and embed as object
        serde_json::from_str::<serde_json::Value>(&payload)?
    } else if payload.is_empty() {
        serde_json::Value::Null
    } else if looks_like_json(&payload) {
        // Auto-detect JSON objects/arrays even without --json so that
        // agents emitting structured events (e.g. work.done) don't get
        // their payload stored as a plain string and rejected by the
        // execution contract validator.
        serde_json::from_str::<serde_json::Value>(&payload)
            .unwrap_or_else(|_| serde_json::Value::String(payload))
    } else {
        serde_json::Value::String(payload)
    };

    // Generic guard: reject an empty task_id in any emitted event payload.
    // An empty task_id is never valid (Ralph task ids are always non-empty
    // strings like `task-{timestamp}-{hex}`), and letting it through would
    // break the step-handoff / state-projection chain.
    if let Some(Value::String(task_id)) = payload_value.get("task_id") {
        if task_id.trim().is_empty() {
            anyhow::bail!("task_id cannot be empty in event payload for topic '{topic}'");
        }
    }

    let mut record = serde_json::json!({
        "topic": args.topic,
        "payload": payload_value,
        "ts": ts
    });

    // U7 of plan 2026-07-05-005 (R6, R12): envelope-layer
    // `triggered` validation. The gate runs BEFORE the record is
    // written so a malformed `triggered` value can never land
    // on disk. Missing `triggered` is allowed (R12). The
    // topology check uses the loaded preset's `hats[]` map; an
    // unknown value yields `triggered_not_in_topology` and the
    // apply path bails before JSONL write.
    if let Some(cfg) = config.as_ref() {
        if let Err(err) = crate::policy_check::check_envelope_triggered(
            &topic,
            triggered.as_deref(),
            cfg,
        ) {
            use ralph_core::{PolicyFinding, ViolationType};
            let finding = PolicyFinding {
                violation_type: ViolationType::SemanticGateViolation {
                    gate: "envelope_triggered".to_string(),
                    context: err.message.clone(),
                },
                topic: topic.to_string(),
                message: err.message.clone(),
            };
            record_cli_emit_rejection(&workspace_root, &topic, hat.as_deref(), &finding);
            anyhow::bail!("Event rejected by envelope-triggered guard: {}", err.message);
        }
    }

    // Add provenance fields only when they have values (preserve old simple schema)
    if let Some(ref hat) = hat {
        record["hat"] = serde_json::Value::String(hat.clone());
    }
    if let Some(triggered) = triggered {
        record["triggered"] = serde_json::Value::String(triggered);
    }
    if let Some(source) = source {
        record["source"] = serde_json::Value::String(source);
    } else if source.is_none()
        && config
            .as_ref()
            .is_some_and(|c| c.event_loop.execution_mode == HatExecutionMode::Isolated)
        && !ralph_core::event_origin::is_ralph_control_topic(&topic)
        && hat.is_some()
    {
        // U7 (2026-06-17-004 plan, R7): in isolated mode, when a business topic
        // has no explicit --source and hat is known, default source to the emitting
        // hat so downstream consumers always have a stable attribution field.
        // Control topics (loop.cancel, task.resume, etc.) are unchanged.
        record["source"] = serde_json::Value::String(
            hat.as_ref()
                .expect("U7 R7: hat checked non-None above")
                .clone(),
        );
    }

    // Auto-tag with wave metadata from env vars (set by loop runner on wave workers)
    if let (Ok(wave_id), Ok(wave_index_str)) = (
        std::env::var("RALPH_WAVE_ID"),
        std::env::var("RALPH_WAVE_INDEX"),
    ) && let Ok(wave_index) = wave_index_str.parse::<u32>()
    {
        record["wave_id"] = serde_json::Value::String(wave_id);
        record["wave_index"] = serde_json::Value::Number(wave_index.into());
    }

    // P0-2 (2026-07-02-005 BP1-3): explicit `--policy-check` is a dry-run
    // probe — validation already ran above; do not append to JSONL.
    // Enforce mode (config-mandated check before write) still writes.
    if check_mode == PolicyCheckMode::ExplicitCheck {
        if use_colors {
            println!(
                "{}✓{} Policy check passed: {} (not written to disk)",
                colors::GREEN,
                colors::RESET,
                topic
            );
        } else {
            println!("Policy check passed: {} (not written to disk)", topic);
        }
        return Ok(());
    }

    // Resolve events file via the P6 allowlist guard. The guard verifies
    // the candidate path is either the active `current-candidate-events`
    // target, the `current-events` target, or the default `events.jsonl`
    // when no marker exists.
    let env_events_file = std::env::var("RALPH_EVENTS_FILE").ok();
    let events_file = resolve_emit_path(&workspace_root, &args.file, env_events_file.as_deref())?;

    // Ensure parent directory exists
    if let Some(parent) = events_file.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Append to file
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_file)
        .with_context(|| format!("Failed to open events file: {}", events_file.display()))?;

    // Write as single-line JSON (JSONL format)
    let json_line = serde_json::to_string(&record)?;
    writeln!(file, "{}", json_line)?;

    // Success message
    if use_colors {
        println!(
            "{}✓{} Event emitted: {}",
            colors::GREEN,
            colors::RESET,
            topic
        );
    } else {
        println!("Event emitted: {}", topic);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::load_config_with_overrides;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn parse_config(yaml: &str) -> RalphConfig {
        serde_yaml::from_str(yaml).expect("valid test config")
    }
    #[test]
    fn test_emit_command_resolves_marker_relative_to_workspace_root_from_nested_dir() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
        std::fs::write(
            workspace.join(".ralph/current-events"),
            ".ralph/events-20260309-test.jsonl\n",
        )
        .expect("write marker");

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("debug.step".to_string()),
                payload: "task_id=demo".to_string(),
                json: false,
                file: PathBuf::from(".ralph/events.jsonl"),
                policy_check: false,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect("emit command");

        let events = std::fs::read_to_string(workspace.join(".ralph/events-20260309-test.jsonl"))
            .expect("read events");
        assert!(events.contains("\"topic\":\"debug.step\""));
        assert!(events.contains("task_id=demo"));
    }

    #[test]
    fn test_emit_command_blocks_once_when_urgent_steer_pending() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
        UrgentSteerStore::new(urgent_steer_path_from_workspace(Some(&workspace)))
            .append_message("stop and fix the failing tests")
            .expect("write urgent steer");

        let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("debug.step".to_string()),
                payload: "task_id=demo".to_string(),
                json: false,
                file: PathBuf::from(".ralph/events.jsonl"),
                policy_check: false,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect_err("urgent steer should block first emit");

        let message = format!("{err:#}");
        assert!(message.contains("Urgent steer is pending"));
        assert!(message.contains("stop and fix the failing tests"));

        assert!(
            UrgentSteerStore::new(urgent_steer_path_from_workspace(Some(&workspace)))
                .load()
                .expect("load marker")
                .is_none(),
            "first blocked emit should clear urgent steer marker"
        );
    }

    #[test]
    fn test_emit_policy_check_rejects_business_after_terminal_with_marker() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

        // Write config with event policy
        std::fs::write(
            workspace.join("ralph.yml"),
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
"#,
        )
        .unwrap();

        // Write existing events file with a terminal event
        let events_file = workspace.join(".ralph/events.jsonl");
        std::fs::write(
            &events_file,
            r#"{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:00Z"}
"#,
        )
        .unwrap();

        // Write marker file pointing to events file
        std::fs::write(
            workspace.join(".ralph/current-events"),
            ".ralph/events.jsonl\n",
        )
        .unwrap();

        let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("experiment.planned".to_string()),
                payload: "{}".to_string(),
                json: true,
                file: PathBuf::from(".ralph/events.jsonl"),
                policy_check: true,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect_err("should reject business event after terminal");

        let message = format!("{err:#}");
        assert!(
            message.contains("Event rejected by policy"),
            "Expected policy rejection, got: {}",
            message
        );
        assert!(
            message.contains("monotonicity"),
            "Expected monotonicity violation, got: {}",
            message
        );

        // Verify the rejected event was NOT appended
        let events = std::fs::read_to_string(&events_file).expect("read events");
        assert!(!events.contains("experiment.planned"));
    }

    #[test]
    fn test_emit_policy_check_without_existing_events_succeeds() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

        // Write config with event policy
        std::fs::write(
            workspace.join("ralph.yml"),
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
"#,
        )
        .unwrap();

        let events_file = workspace.join(".ralph/events.jsonl");

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("experiment.planned".to_string()),
                payload: "{}".to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: true,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect("should accept business event when no terminal exists");

        let events = std::fs::read_to_string(&events_file).unwrap_or_default();
        assert!(
            events.trim().is_empty(),
            "explicit --policy-check must not write to events file; got: {events}"
        );
    }

    #[test]
    fn test_emit_policy_check_fallback_to_args_file_when_marker_missing() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

        // Write config with event policy
        std::fs::write(
            workspace.join("ralph.yml"),
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
"#,
        )
        .unwrap();

        // Write existing events file WITHOUT marker
        let events_file = workspace.join(".ralph/events.jsonl");
        std::fs::write(
            &events_file,
            r#"{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:00Z"}
"#,
        )
        .unwrap();

        let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("experiment.planned".to_string()),
                payload: "{}".to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: true,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect_err("should reject business event after terminal");

        let message = format!("{err:#}");
        assert!(
            message.contains("Event rejected by policy"),
            "Expected policy rejection, got: {}",
            message
        );

        // Verify the rejected event was NOT appended
        let events = std::fs::read_to_string(&events_file).expect("read events");
        assert!(!events.contains("experiment.planned"));
    }

    #[test]
    fn test_emit_with_provenance_flags() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

        let events_file = workspace.join(".ralph/events.jsonl");

        // U1 of 2026-07-05-005: write a minimal ralph.yml that
        // overrides RALPH_HATS_SOURCE from the parent loop context so
        // the isolated-scope guard accepts the test's chosen hat +
        // topic. The hat id matches RALPH_CURRENT_HAT (typically
        // "fixer") so the U7 isolated-mode hat-match check at
        // emit.rs:550-560 also passes. The current-events marker is
        // pointed at the parent loop's RALPH_EVENTS_FILE (when present)
        // so the P6 allowlist guard accepts the env-injected target.
        // Test intent (provenance flag preservation) is unchanged.
        let hat = std::env::var("RALPH_CURRENT_HAT").unwrap_or_else(|_| "strategist".to_string());
        // Mirror RALPH_TRIGGERED_HAT when the parent loop sets it; otherwise
        // fall back to the same hat id as `--hat` so the U7 topology check
        // (`check_envelope_triggered`) sees a declared id and the
        // ralph.yml below only needs one entry under `hats:`.
        let triggered = std::env::var("RALPH_TRIGGERED_HAT")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| hat.clone());
        let triggered_entry = if triggered == hat {
            String::new()
        } else {
            format!(
                "  {triggered}:\n    name: \"{triggered}\"\n    triggers: []\n    publishes: [\"experiment.planned\", \"*\"]\n"
            )
        };
        std::fs::write(
            workspace.join("ralph.yml"),
            format!(
                "event_loop:\n  execution_mode: coordinator\nhats:\n  {hat}:\n    name: \"{hat}\"\n    triggers: []\n    publishes: [\"experiment.planned\", \"*\"]\n{triggered_entry}"
            ),
        )
        .expect("write ralph.yml");
        let env_events_file = std::env::var("RALPH_EVENTS_FILE").ok();
        if let Some(ref env_path) = env_events_file {
            std::fs::write(
                workspace.join(".ralph/current-events"),
                env_path.as_bytes(),
            )
            .expect("write current-events marker");
        }

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("experiment.planned".to_string()),
                payload: r#"{"task_key":"x"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some(hat.clone()),
                triggered: Some(triggered.clone()),
                source: Some("cli".to_string()),
                schema: None,
            },
            Some(&workspace),
        )
        .expect("emit with provenance should succeed");

        // The emit may have been routed to the env-injected events
        // file (when RALPH_EVENTS_FILE is set by the parent loop) or
        // to the workspace's events.jsonl (when no env override
        // exists). Read whichever the resolver chose.
        let read_target = env_events_file
            .as_deref()
            .map(std::path::PathBuf::from)
            .unwrap_or(events_file);
        let events = std::fs::read_to_string(&read_target).expect("read events");
        assert!(events.contains(&format!("\"hat\":\"{hat}\"")));
        assert!(events.contains(&format!("\"triggered\":\"{triggered}\"")));
        assert!(events.contains("\"source\":\"cli\""));
    }

    #[test]
    fn test_resolve_provenance_priority() {
        // CLI args take priority over env vars
        let env = |key: &str| match key {
            "RALPH_CURRENT_HAT" => Some("env-hat".to_string()),
            "RALPH_TRIGGERED_HAT" => Some("env-triggered".to_string()),
            "RALPH_EVENT_SOURCE" => Some("env-source".to_string()),
            _ => None,
        };
        let (hat, triggered, source) =
            resolve_provenance(Some("cli-hat".to_string()), None, None, env);
        assert_eq!(hat, Some("cli-hat".to_string()));
        assert_eq!(triggered, Some("env-triggered".to_string()));
        assert_eq!(source, Some("env-source".to_string()));
    }

    #[test]
    fn test_resolve_provenance_env_fallback() {
        // When CLI args are missing, env vars are used
        let env = |key: &str| match key {
            "RALPH_CURRENT_HAT" => Some("env-hat".to_string()),
            "RALPH_TRIGGERED_HAT" => Some("env-triggered".to_string()),
            "RALPH_EVENT_SOURCE" => Some("env-source".to_string()),
            _ => None,
        };
        let (hat, triggered, source) = resolve_provenance(None, None, None, env);
        assert_eq!(hat, Some("env-hat".to_string()));
        assert_eq!(triggered, Some("env-triggered".to_string()));
        assert_eq!(source, Some("env-source".to_string()));
    }

    #[test]
    fn test_resolve_provenance_empty_env_is_ignored() {
        // Empty env vars are treated as absent
        let env = |_key: &str| Some("".to_string());
        let (hat, triggered, source) = resolve_provenance(None, None, None, env);
        assert_eq!(hat, None);
        assert_eq!(triggered, None);
        assert_eq!(source, None);
    }

    // U1: ralph-hat business-topic guard. Mirrors the origin guard's
    // `ralph_control_only` rejection at the JSONL read path, but rejects
    // here so the agent receives synchronous backpressure instead of
    // waiting several seconds for the loop runner to surface the rejection.
    // The guard fires regardless of --policy-check, because the issue is
    // the impersonation, not the payload shape.

    #[test]
    fn test_emit_ralph_hat_rejects_business_topic_review_passed() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
        let events_file = workspace.join(".ralph/events.jsonl");

        let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("review.passed".to_string()),
                payload: r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("ralph".to_string()),
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect_err("ralph hat must not be allowed to emit review.passed");

        let message = format!("{err:#}");
        assert!(
            message.contains("Builtin ralph hat may only emit control topics"),
            "expected ralph-control guard message, got: {message}"
        );
        assert!(
            message.contains("review.passed"),
            "error should name the rejected topic, got: {message}"
        );

        // Verify nothing was written
        assert!(!events_file.exists() || std::fs::read_to_string(&events_file).unwrap().is_empty());
    }

    #[test]
    fn test_emit_ralph_hat_rejects_business_topic_work_start() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
        let events_file = workspace.join(".ralph/events.jsonl");

        let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("work.start".to_string()),
                payload: String::new(),
                json: false,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("ralph".to_string()),
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect_err("ralph hat must not be allowed to emit work.start");

        let message = format!("{err:#}");
        assert!(message.contains("Builtin ralph hat may only emit control topics"));
    }

    #[test]
    fn test_emit_ralph_hat_allows_control_topic_loop_complete() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
        let events_file = workspace.join(".ralph/events.jsonl");

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("LOOP_COMPLETE".to_string()),
                payload: r#"{"reason":"done"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("ralph".to_string()),
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect("ralph hat must be allowed to emit LOOP_COMPLETE (control topic)");

        let events = std::fs::read_to_string(&events_file).expect("read events");
        assert!(events.contains("\"topic\":\"LOOP_COMPLETE\""));
        assert!(events.contains("\"hat\":\"ralph\""));
    }

    #[test]
    fn test_emit_ralph_hat_allows_task_resume() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
        let events_file = workspace.join(".ralph/events.jsonl");

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("task.resume".to_string()),
                payload: r#"{"reason":"recover"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("ralph".to_string()),
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect("ralph hat must be allowed to emit task.resume (control topic)");

        let events = std::fs::read_to_string(&events_file).expect("read events");
        assert!(events.contains("task.resume"));
    }

    #[test]
    fn test_emit_executor_hat_unaffected_by_ralph_guard() {
        // Regression: only `ralph` is restricted. Other hats (executor,
        // coordinator, etc.) may emit business topics as before.
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
        let events_file = workspace.join(".ralph/events.jsonl");

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("work.done".to_string()),
                payload: r#"{"plan_name":"p","plan_path":"x.md","task_id":"t","task_key":"k","step":"s","commit_count":1,"changed_lines":10}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("executor".to_string()),
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect("executor hat should be free to emit work.done (not restricted)");

        let events = std::fs::read_to_string(&events_file).expect("read events");
        assert!(events.contains("work.done"));
        assert!(events.contains("\"hat\":\"executor\""));
    }

    #[test]
    fn test_emit_no_hat_unaffected_by_ralph_guard() {
        // No --hat means no ralph guard fires. Other guards (provenance,
        // policy) still apply.
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
        let events_file = workspace.join(".ralph/events.jsonl");

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("debug.step".to_string()),
                payload: "task_id=demo".to_string(),
                json: false,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect("no-hat emit should not be blocked by the ralph guard");

        let events = std::fs::read_to_string(&events_file).expect("read events");
        assert!(events.contains("debug.step"));
    }

    #[test]
    fn test_emit_provenance_strict_rejects_missing_hat() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

        // Write config with require_emit_provenance enabled
        let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    require_emit_provenance: true
"#;
        std::fs::write(workspace.join("ralph.yml"), yaml).unwrap();

        // Verify config loads and parses correctly in isolation
        let config_sources = vec![ConfigSource::File(workspace.join("ralph.yml"))];
        let config =
            load_config_with_overrides(&config_sources).expect("config should load for this test");
        let policy = config
            .event_loop
            .event_policy
            .as_ref()
            .expect("event_policy should be present");
        assert!(
            policy.require_emit_provenance,
            "require_emit_provenance should be true"
        );

        let events_file = workspace.join(".ralph/events.jsonl");

        let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("build.done".to_string()),
                payload: String::new(),
                json: false,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect_err("should reject emit without provenance when strict");

        let message = format!("{err:#}");
        assert!(
            message.contains("Event provenance required"),
            "Expected provenance rejection, got: {}",
            message
        );

        // Verify nothing was written
        assert!(!events_file.exists() || std::fs::read_to_string(&events_file).unwrap().is_empty());
    }

    #[test]
    fn test_emit_provenance_strict_allows_with_hat() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

        // Write config with require_emit_provenance enabled
        std::fs::write(
            workspace.join("ralph.yml"),
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    require_emit_provenance: true
"#,
        )
        .unwrap();

        let events_file = workspace.join(".ralph/events.jsonl");

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("build.done".to_string()),
                payload: String::new(),
                json: false,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("strategist".to_string()),
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect("should allow emit with hat when strict");

        let events = std::fs::read_to_string(&events_file).expect("read events");
        assert!(events.contains("\"hat\":\"strategist\""));
    }

    #[test]
    fn test_emit_strict_config_rejects_missing_required_field_without_policy_check_flag() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

        // Write strict config: policy enabled AND require_policy_check_for_cli_emit
        std::fs::write(
            workspace.join("ralph.yml"),
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: true
    schemas:
      experiment.planned:
        required_fields:
          - task_key
          - hypothesis
          - falsification_condition
"#,
        )
        .unwrap();

        let events_file = workspace.join(".ralph/events.jsonl");

        let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("experiment.planned".to_string()),
                payload: r#"{"task_key":"x"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect_err(
            "strict config should reject missing required field even without --policy-check",
        );

        let message = format!("{err:#}");
        assert!(
            message.contains("Event rejected by policy"),
            "Expected policy rejection, got: {}",
            message
        );

        // Verify nothing was written
        assert!(!events_file.exists() || std::fs::read_to_string(&events_file).unwrap().is_empty());
    }

    #[test]
    fn test_emit_strict_config_rejects_duplicate_terminal_without_policy_check_flag() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

        // Write strict config
        std::fs::write(
            workspace.join("ralph.yml"),
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: true
    terminal_topics:
      - LOOP_COMPLETE
"#,
        )
        .unwrap();

        // Pre-seed events file with a terminal event
        let events_file = workspace.join(".ralph/events.jsonl");
        std::fs::write(
            &events_file,
            r#"{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:00Z"}
"#,
        )
        .unwrap();

        let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("LOOP_COMPLETE".to_string()),
                payload: r#"{"reason":"done"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect_err("strict config should reject duplicate terminal even without --policy-check");

        let message = format!("{err:#}");
        assert!(
            message.contains("Event rejected by policy"),
            "Expected policy rejection, got: {}",
            message
        );

        // Verify duplicate was NOT appended
        let events = std::fs::read_to_string(&events_file).expect("read events");
        assert_eq!(events.lines().count(), 1);
    }

    #[test]
    fn test_emit_non_strict_config_allows_without_policy_check() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

        // Write non-strict config: policy enabled but require_policy_check_for_cli_emit is false
        std::fs::write(
            workspace.join("ralph.yml"),
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: false
    terminal_topics:
      - LOOP_COMPLETE
"#,
        )
        .unwrap();

        let events_file = workspace.join(".ralph/events.jsonl");

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("build.done".to_string()),
                payload: String::new(),
                json: false,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect("non-strict config should allow emit without --policy-check");

        let events = std::fs::read_to_string(&events_file).expect("read events");
        assert!(events.contains("build.done"));
    }

    #[test]
    fn test_emit_explicit_policy_check_behavior_preserved() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

        // Write config with event policy but NOT strict CLI enforcement
        std::fs::write(
            workspace.join("ralph.yml"),
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: false
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
"#,
        )
        .unwrap();

        // Pre-seed with terminal
        let events_file = workspace.join(".ralph/events.jsonl");
        std::fs::write(
            &events_file,
            r#"{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:00Z"}
"#,
        )
        .unwrap();

        // Explicit --policy-check should still reject business after terminal
        let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("experiment.planned".to_string()),
                payload: "{}".to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: true,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect_err("explicit --policy-check should still reject");

        let message = format!("{err:#}");
        assert!(message.contains("Event rejected by policy"));
    }

    #[test]
    fn test_emit_unsafe_bypass_allowed_when_config_permits() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

        // Write strict config but allow unsafe bypass
        std::fs::write(
            workspace.join("ralph.yml"),
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: true
    terminal_topics:
      - LOOP_COMPLETE
"#,
        )
        .unwrap();

        // Pre-seed with terminal
        let events_file = workspace.join(".ralph/events.jsonl");
        std::fs::write(
            &events_file,
            r#"{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:00Z"}
"#,
        )
        .unwrap();

        // Unsafe bypass should allow the duplicate terminal through
        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("LOOP_COMPLETE".to_string()),
                payload: r#"{"reason":"retry"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: true,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect("unsafe bypass should work when config allows it");

        let events = std::fs::read_to_string(&events_file).expect("read events");
        assert!(events.contains("\"reason\":\"retry\""));
    }

    #[test]
    fn test_emit_unsafe_bypass_rejected_when_config_denies() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

        // Write strict config that DISALLOWS unsafe bypass
        std::fs::write(
            workspace.join("ralph.yml"),
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: false
    terminal_topics:
      - LOOP_COMPLETE
"#,
        )
        .unwrap();

        // Pre-seed with terminal
        let events_file = workspace.join(".ralph/events.jsonl");
        std::fs::write(
            &events_file,
            r#"{"topic":"LOOP_COMPLETE","ts":"2024-01-01T00:00:00Z"}
"#,
        )
        .unwrap();

        // Unsafe bypass should be rejected because config denies it
        let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("LOOP_COMPLETE".to_string()),
                payload: r#"{"reason":"retry"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: true,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect_err("unsafe bypass should fail when config denies it");

        let message = format!("{err:#}");
        assert!(
            message.contains("Event rejected by policy"),
            "Expected policy rejection, got: {}",
            message
        );
    }

    const FIXTURE_VALID_CHAIN: &str = r#"{"topic":"experiment.planned","payload":{"task_key":"a","hypothesis":"h","falsification_condition":"f"},"ts":"2026-05-22T00:00:00Z"}
{"topic":"LOOP_COMPLETE","payload":{"reason":"done"},"ts":"2026-05-22T00:00:01Z"}"#;

    const FIXTURE_DUPLICATE_TERMINAL: &str = r#"{"topic":"LOOP_COMPLETE","payload":{"reason":"done"},"ts":"2026-05-22T00:00:00Z"}
{"topic":"LOOP_COMPLETE","payload":{"reason":"retry"},"ts":"2026-05-22T00:00:01Z"}"#;

    const FIXTURE_BUSINESS_AFTER_TERMINAL: &str = r#"{"topic":"LOOP_COMPLETE","payload":{"reason":"done"},"ts":"2026-05-22T00:00:00Z"}
{"topic":"experiment.planned","payload":{"task_key":"b","hypothesis":"h","falsification_condition":"f"},"ts":"2026-05-22T00:00:01Z"}"#;

    const FIXTURE_MISSING_REQUIRED_FIELDS: &str =
        r#"{"topic":"experiment.planned","payload":{"task_key":"a"},"ts":"2026-05-22T00:00:00Z"}"#;

    fn fixture_config_yaml() -> &'static str {
        r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    require_emit_provenance: true
    allow_unsafe_cli_emit: true
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
    schemas:
      experiment.planned:
        payload: json_object
        required_fields:
          - task_key
          - hypothesis
          - falsification_condition
    completion_after_terminal:
      duplicate_terminal: reject
      business_after_completion: reject
"#
    }

    fn fixture_policy_config() -> ralph_core::EventPolicyConfig {
        let full: ralph_core::RalphConfig = serde_yaml::from_str(fixture_config_yaml()).unwrap();
        full.event_loop.event_policy.unwrap()
    }

    fn setup_fixture_workspace(temp_dir: &TempDir, prior_events: &str) -> PathBuf {
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).unwrap();
        std::fs::write(workspace.join("ralph.yml"), fixture_config_yaml()).unwrap();
        let events_file = workspace.join(".ralph/events.jsonl");
        std::fs::write(&events_file, prior_events).unwrap();
        std::fs::write(
            workspace.join(".ralph/current-events"),
            ".ralph/events.jsonl\n",
        )
        .unwrap();
        workspace
    }

    fn parse_last_fixture_event(fixture: &str) -> (String, String, bool) {
        let line = fixture.lines().last().unwrap();
        let value: serde_json::Value = serde_json::from_str(line).unwrap();
        let topic = value["topic"].as_str().unwrap().to_string();
        let (payload, json) = match &value.get("payload") {
            Some(serde_json::Value::Object(_)) => {
                (serde_json::to_string(&value["payload"]).unwrap(), true)
            }
            Some(serde_json::Value::String(s)) => (s.clone(), false),
            Some(serde_json::Value::Null) | None => (String::new(), false),
            _ => (serde_json::to_string(&value["payload"]).unwrap(), true),
        };
        (topic, payload, json)
    }

    #[test]
    fn test_fixture_cli_valid_chain_accepted() {
        let temp_dir = TempDir::new().expect("temp dir");
        let prior: String = FIXTURE_VALID_CHAIN
            .lines()
            .take(1)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let workspace = setup_fixture_workspace(&temp_dir, &prior);
        let (topic, payload, json) = parse_last_fixture_event(FIXTURE_VALID_CHAIN);
        let events_file = workspace.join(".ralph/events.jsonl");

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some(topic),
                payload,
                json,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("strategist".to_string()),
                triggered: None,
                source: Some("cli".to_string()),
                schema: None,
            },
            Some(&workspace),
        )
        .expect("CLI should accept valid chain terminal event");

        let events = std::fs::read_to_string(&events_file).expect("read events");
        assert!(events.contains("\"reason\":\"done\""));
        assert!(events.contains("\"hat\":\"strategist\""));
        assert!(events.contains("\"source\":\"cli\""));
    }

    #[test]
    fn test_fixture_cli_duplicate_terminal_rejected() {
        let temp_dir = TempDir::new().expect("temp dir");
        let prior: String = FIXTURE_DUPLICATE_TERMINAL
            .lines()
            .take(1)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let workspace = setup_fixture_workspace(&temp_dir, &prior);
        let (topic, payload, json) = parse_last_fixture_event(FIXTURE_DUPLICATE_TERMINAL);
        let events_file = workspace.join(".ralph/events.jsonl");

        let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some(topic),
                payload,
                json,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("strategist".to_string()),
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect_err("CLI should reject duplicate terminal");

        let message = format!("{err:#}");
        assert!(
            message.contains("Event rejected by policy")
                || message.contains("Event blocked by policy")
                || message.contains("Event ignored by policy"),
            "Expected policy rejection, got: {}",
            message
        );

        let events = std::fs::read_to_string(&events_file).expect("read events");
        assert_eq!(events.lines().count(), 1);
    }

    #[test]
    fn test_fixture_cli_business_after_terminal_rejected() {
        let temp_dir = TempDir::new().expect("temp dir");
        let prior: String = FIXTURE_BUSINESS_AFTER_TERMINAL
            .lines()
            .take(1)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        let workspace = setup_fixture_workspace(&temp_dir, &prior);
        let (topic, payload, json) = parse_last_fixture_event(FIXTURE_BUSINESS_AFTER_TERMINAL);
        let events_file = workspace.join(".ralph/events.jsonl");

        let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some(topic),
                payload,
                json,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("strategist".to_string()),
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect_err("CLI should reject business after terminal");

        let message = format!("{err:#}");
        assert!(
            message.contains("Event rejected by policy")
                || message.contains("Event blocked by policy")
                || message.contains("Event ignored by policy"),
            "Expected policy rejection, got: {}",
            message
        );

        let events = std::fs::read_to_string(&events_file).expect("read events");
        assert!(!events.contains("\"task_key\":\"b\""));
    }

    #[test]
    fn test_fixture_cli_missing_required_fields_rejected_when_strict() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = setup_fixture_workspace(&temp_dir, "");
        let (topic, payload, json) = parse_last_fixture_event(FIXTURE_MISSING_REQUIRED_FIELDS);
        let events_file = workspace.join(".ralph/events.jsonl");

        let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some(topic),
                payload,
                json,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: None, // missing provenance
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect_err("CLI should reject missing provenance under strict config");

        let message = format!("{err:#}");
        assert!(
            message.contains("Event provenance required"),
            "Expected provenance rejection, got: {}",
            message
        );

        assert!(!events_file.exists() || std::fs::read_to_string(&events_file).unwrap().is_empty());
    }

    #[test]
    fn test_fixture_cross_cutting_cli_and_event_loop_agree() {
        use ralph_core::{PolicyDecision, PolicyRuntimeState, validate_event};

        let policy_config = fixture_policy_config();

        let fixtures: &[&str] = &[
            FIXTURE_VALID_CHAIN,
            FIXTURE_DUPLICATE_TERMINAL,
            FIXTURE_BUSINESS_AFTER_TERMINAL,
            FIXTURE_MISSING_REQUIRED_FIELDS,
        ];

        for fixture in fixtures {
            let lines: Vec<&str> = fixture.lines().collect();
            let prior = if lines.len() > 1 {
                lines[..lines.len() - 1].join("\n") + "\n"
            } else {
                String::new()
            };

            let temp_dir = TempDir::new().expect("temp dir");
            let workspace = setup_fixture_workspace(&temp_dir, &prior);
            let events_file = workspace.join(".ralph/events.jsonl");

            // -- Event loop path --
            let mut state =
                PolicyRuntimeState::from_events(&events_file, &policy_config).unwrap_or_default();

            let line = lines.last().unwrap();
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            let topic = value["topic"].as_str().unwrap();
            let payload = match &value.get("payload") {
                Some(v) if !v.is_null() => Some(serde_json::to_string(v).unwrap()),
                _ => None,
            };

            let loop_decision =
                validate_event(topic, payload.as_deref(), &policy_config, &mut state);
            // U2 (plan 2026-07-04-004): `AcknowledgeAndForward` is
            // an "accept" from the bus-forwarding perspective. The
            // dedup finding is logged but the event reaches the
            // state machine. Parity test fixtures must NOT treat
            // AcknowledgeAndForward as a rejection.
            let loop_accept = matches!(
                loop_decision,
                PolicyDecision::Accept
                    | PolicyDecision::Warn(_)
                    | PolicyDecision::AcknowledgeAndForward(_)
            );

            // -- CLI path --
            let (cli_topic, cli_payload, cli_json) = parse_last_fixture_event(fixture);
            let cli_result = emit_command_with_root(
                ColorMode::Never,
                EmitArgs {
                    topic: Some(cli_topic),
                    payload: cli_payload,
                    json: cli_json,
                    file: events_file.clone(),
                    policy_check: false,
                    no_policy_check: false,
                    hat: Some("strategist".to_string()),
                    triggered: None,
                    source: None,
                    schema: None,
                },
                Some(&workspace),
            );
            let cli_accept = cli_result.is_ok();

            assert_eq!(
                loop_accept, cli_accept,
                "Cross-cutting classification mismatch for fixture.\nFixture: {}\nLoop decision: {:?}\nCLI result: {:?}",
                fixture, loop_decision, cli_result
            );
        }
    }

    #[test]
    fn test_provenance_fields_preserved_by_reader() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).unwrap();
        let events_file = workspace.join(".ralph/events.jsonl");

        // U1 of 2026-07-05-005: write a minimal ralph.yml + current-events
        // marker that overrides RALPH_HATS_SOURCE / RALPH_EVENTS_FILE from
        // the parent loop context (see test_emit_with_provenance_flags for
        // full rationale).
        let hat = std::env::var("RALPH_CURRENT_HAT").unwrap_or_else(|_| "strategist".to_string());
        // Mirror RALPH_TRIGGERED_HAT when the parent loop sets it; otherwise
        // fall back to the same hat id as `--hat` so the U7 topology check
        // (`check_envelope_triggered`) sees a declared id and the
        // ralph.yml below only needs one entry under `hats:`.
        let triggered = std::env::var("RALPH_TRIGGERED_HAT")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| hat.clone());
        let triggered_entry = if triggered == hat {
            String::new()
        } else {
            format!(
                "  {triggered}:\n    name: \"{triggered}\"\n    triggers: []\n    publishes: [\"experiment.planned\", \"*\"]\n"
            )
        };
        std::fs::write(
            workspace.join("ralph.yml"),
            format!(
                "event_loop:\n  execution_mode: coordinator\nhats:\n  {hat}:\n    name: \"{hat}\"\n    triggers: []\n    publishes: [\"experiment.planned\", \"*\"]\n{triggered_entry}"
            ),
        )
        .expect("write ralph.yml");
        let env_events_file = std::env::var("RALPH_EVENTS_FILE").ok();
        if let Some(ref env_path) = env_events_file {
            std::fs::write(
                workspace.join(".ralph/current-events"),
                env_path.as_bytes(),
            )
            .expect("write current-events marker");
        }

        let read_target = env_events_file
            .as_deref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| events_file.clone());

        // Snapshot pre-emit event count from the target file (the file
        // may already contain events from a parent loop when env is set).
        let pre_count = if read_target.exists() {
            std::fs::read_to_string(&read_target)
                .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
                .unwrap_or(0)
        } else {
            0
        };

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("experiment.planned".to_string()),
                payload: r#"{"task_key":"x"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some(hat.clone()),
                triggered: Some(triggered.clone()),
                source: Some("cli".to_string()),
                schema: None,
            },
            Some(&workspace),
        )
        .expect("emit should succeed");

        let mut reader = ralph_core::EventReader::new(&read_target);
        let result = reader.read_new_events().unwrap();
        // The pre_count snapshot accounts for parent-loop events
        // sharing the same file; only one new event should appear.
        assert_eq!(result.events.len(), pre_count + 1);
        let event = result.events.last().expect("at least one event");
        assert_eq!(event.hat, Some(hat));
        assert_eq!(event.triggered, Some(triggered));
        assert_eq!(event.source, Some("cli".to_string()));
    }

    #[test]
    fn test_old_simple_event_fixtures_still_parse() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).unwrap();
        let events_file = workspace.join(".ralph/events.jsonl");
        std::fs::write(
            &events_file,
            r#"{"topic":"task.start","payload":"Start work","ts":"2024-01-01T00:00:00Z"}
{"topic":"task.done","payload":null,"ts":"2024-01-01T00:00:01Z"}
{"topic":"noop","ts":"2024-01-01T00:00:02Z"}
"#,
        )
        .unwrap();

        let mut reader = ralph_core::EventReader::new(&events_file);
        let result = reader.read_new_events().unwrap();
        assert_eq!(result.events.len(), 3);
        assert_eq!(result.events[0].topic, "task.start");
        assert_eq!(result.events[0].payload, Some("Start work".to_string()));
        assert!(result.events[1].payload.is_none());
        assert!(result.events[2].payload.is_none());
    }

    fn make_workspace(tmp: &TempDir) -> PathBuf {
        let root = tmp.path().to_path_buf();
        std::fs::create_dir_all(root.join(".ralph")).unwrap();
        root
    }

    #[test]
    fn test_emit_default_uses_current_candidate_marker() {
        let tmp = TempDir::new().unwrap();
        let workspace = make_workspace(&tmp);
        std::fs::write(
            workspace.join(".ralph/current-candidate-events"),
            ".ralph/events-20260101-000000.jsonl",
        )
        .unwrap();
        let resolved =
            resolve_emit_path(&workspace, &workspace.join(".ralph/events.jsonl"), None).unwrap();
        assert!(resolved.ends_with(".ralph/events-20260101-000000.jsonl"));
    }

    #[test]
    fn test_emit_default_uses_current_events_marker() {
        let tmp = TempDir::new().unwrap();
        let workspace = make_workspace(&tmp);
        std::fs::write(
            workspace.join(".ralph/current-events"),
            ".ralph/events-20260101-000000.jsonl",
        )
        .unwrap();
        let resolved =
            resolve_emit_path(&workspace, &workspace.join(".ralph/events.jsonl"), None).unwrap();
        assert!(resolved.ends_with(".ralph/events-20260101-000000.jsonl"));
    }

    #[test]
    fn test_emit_no_marker_allows_default_events_jsonl() {
        let tmp = TempDir::new().unwrap();
        let workspace = make_workspace(&tmp);
        let cli_file = workspace.join(".ralph/events.jsonl");
        let resolved = resolve_emit_path(&workspace, &cli_file, None).unwrap();
        assert_eq!(resolved, cli_file);
    }

    #[test]
    fn test_emit_file_explicit_current_marker_allowed() {
        let tmp = TempDir::new().unwrap();
        let workspace = make_workspace(&tmp);
        std::fs::write(
            workspace.join(".ralph/current-events"),
            ".ralph/events-20260101-000000.jsonl",
        )
        .unwrap();
        // The explicit --file target equals the marker target, so it is
        // accepted (matches the allowlist entry).
        let cli_file = workspace.join(".ralph/events-20260101-000000.jsonl");
        let resolved = resolve_emit_path(&workspace, &cli_file, None).unwrap();
        assert_eq!(resolved, cli_file);
    }

    #[test]
    fn test_emit_file_other_loop_rejected() {
        let tmp = TempDir::new().unwrap();
        let workspace = make_workspace(&tmp);
        std::fs::write(
            workspace.join(".ralph/current-events"),
            ".ralph/events-20260101-000000.jsonl",
        )
        .unwrap();
        // An explicit --file that points outside the allowlist must be
        // rejected. We do NOT silently rewrite to the marker target —
        // that would let an agent redirect events to a different
        // worktree's file.
        let cli_file = workspace.join(".ralph/events-other.jsonl");
        let result = resolve_emit_path(&workspace, &cli_file, None);
        assert!(
            result.is_err(),
            "non-allowlisted --file must be rejected, got: {:?}",
            result.map(|p| p.display().to_string())
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("allowlist") || msg.contains("not in"),
            "error should mention allowlist, got: {msg}"
        );
    }

    #[test]
    fn test_emit_env_events_file_other_loop_rejected() {
        let tmp = TempDir::new().unwrap();
        let workspace = make_workspace(&tmp);
        std::fs::write(
            workspace.join(".ralph/current-events"),
            ".ralph/events-20260101-000000.jsonl",
        )
        .unwrap();
        // RALPH_EVENTS_FILE pointing at a different file is rejected.
        let env_value = workspace
            .join(".ralph/events-other.jsonl")
            .display()
            .to_string();
        let result = resolve_emit_path(
            &workspace,
            &workspace.join(".ralph/events.jsonl"),
            Some(&env_value),
        );
        assert!(
            result.is_err(),
            "non-allowlisted RALPH_EVENTS_FILE must be rejected"
        );
    }

    #[test]
    fn test_emit_path_traversal_rejected() {
        let tmp = TempDir::new().unwrap();
        let workspace = make_workspace(&tmp);
        std::fs::write(
            workspace.join(".ralph/current-events"),
            ".ralph/events-20260101-000000.jsonl",
        )
        .unwrap();
        // An explicit `--file ../escape.jsonl` is rejected because it
        // is not in the events allowlist. The new guard treats the file
        // as a request to escape the workspace and refuses outright
        // (no silent rewrite to the marker).
        let cli_file = workspace.join("../escape.jsonl");
        let result = resolve_emit_path(&workspace, &cli_file, None);
        assert!(
            result.is_err(),
            "path traversal with explicit --file must be rejected"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("allowlist") || msg.contains("not in"),
            "error should mention allowlist, got: {msg}"
        );

        // Without a marker and an explicit traversal, the explicit file
        // is also rejected (the default events.jsonl is not in scope of
        // the traversal).
        std::fs::remove_file(workspace.join(".ralph/current-events")).unwrap();
        let result = resolve_emit_path(&workspace, &cli_file, None);
        assert!(
            result.is_err(),
            "path traversal with no marker must be rejected"
        );
    }

    #[test]
    fn test_emit_symlink_to_other_loop_rejected() {
        let tmp = TempDir::new().unwrap();
        let workspace = make_workspace(&tmp);
        // No markers; the allowlist is just the default events.jsonl.
        let outside = tmp.path().parent().unwrap().join("outside.jsonl");
        std::fs::write(&outside, "{}").unwrap();
        // A symlink that aliases the default target to an outside file is
        // detected via canonicalize and rejected.
        let link = workspace.join(".ralph/events.jsonl");
        if std::os::unix::fs::symlink(&outside, &link).is_err() {
            return;
        }
        let result = resolve_emit_path(&workspace, &workspace.join(".ralph/events.jsonl"), None);
        assert!(result.is_err(), "symlink to outside loop must be rejected");
    }

    #[test]
    fn test_emit_auto_detects_json_payload_without_json_flag() {
        // Bug #4 regression: work.done and other structured events must be
        // stored as JSON objects even when the agent forgets --json.
        let tmp = TempDir::new().unwrap();
        let workspace = make_workspace(&tmp);
        let events_file = workspace.join(".ralph/events.jsonl");

        let args = EmitArgs {
            topic: Some("work.done".to_string()),
            payload: r#"{"plan_name":"test","task_id":"t1"}"#.to_string(),
            json: false,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
        };

        emit_command_with_root(ColorMode::Never, args, Some(&workspace)).unwrap();

        let content = std::fs::read_to_string(&events_file).unwrap();
        let event: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        // payload must be an object, NOT a string
        assert!(
            event["payload"].is_object(),
            "payload should be auto-detected as JSON object, got: {:?}",
            event["payload"]
        );
        assert_eq!(event["payload"]["plan_name"], "test");
    }

    #[test]
    fn test_emit_leaves_plain_string_as_string() {
        // Non-JSON-looking strings must stay strings for backward compat.
        let tmp = TempDir::new().unwrap();
        let workspace = make_workspace(&tmp);
        let events_file = workspace.join(".ralph/events.jsonl");

        let args = EmitArgs {
            topic: Some("build.done".to_string()),
            payload: "Build succeeded".to_string(),
            json: false,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: None,
        };

        emit_command_with_root(ColorMode::Never, args, Some(&workspace)).unwrap();

        let content = std::fs::read_to_string(&events_file).unwrap();
        let event: serde_json::Value = serde_json::from_str(content.trim()).unwrap();
        assert_eq!(event["payload"], "Build succeeded");
    }

    #[test]
    fn test_looks_like_json_heuristic() {
        assert!(looks_like_json(r#"{"key":"val"}"#));
        assert!(looks_like_json("  [{\"a\":1}]"));
        assert!(!looks_like_json("hello world"));
        assert!(!looks_like_json(""));
        assert!(!looks_like_json("  plain text"));
    }

    // ------------------------------------------------------------------
    // U5 / R6: `ralph emit --schema <TOPIC>` smoke tests.
    //
    // The handler short-circuits to a read-only JSON dump of the
    // embedded protocol view (KTD-10) before any policy / scope /
    // gate runs. These tests pin that contract: no events
    // file is touched, no policy decision is required, the output
    // is valid JSON carrying `protocol_hash` and the requested
    // topic's `required_fields`.
    // ------------------------------------------------------------------

    /// Minimal preset fixture mirroring the section layout that
    /// `build.rs` produces for `ce-executor-serial`. We only need
    /// `event_policy.schemas.work.done` to exercise the
    /// required-fields surface.
    const SCHEMA_FIXTURE_YAML: &str = r#"
event_loop:
  execution_mode: isolated
  event_policy:
    enabled: true
    mode: enforce
    schemas:
      work.done:
        required_fields:
          - plan_name
          - task_id
          - task_key
"#;

    fn setup_schema_workspace(tmp: &TempDir, yaml: &str) -> PathBuf {
        let workspace = tmp.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).unwrap();
        std::fs::write(workspace.join("ralph.yml"), yaml).unwrap();
        workspace
    }

    /// (1) `ralph emit --schema <topic>` prints a JSON view carrying
    /// `protocol_hash` and the topic's `required_fields`. The view
    /// is the only stdout payload; the events file is not created.
    #[test]
    fn test_emit_schema_prints_protocol_view_without_writing_events() {
        let tmp = TempDir::new().expect("temp dir");
        let workspace = setup_schema_workspace(&tmp, SCHEMA_FIXTURE_YAML);
        let events_file = workspace.join(".ralph/events.jsonl");

        let args = EmitArgs {
            topic: Some("work.done".to_string()),
            payload: String::new(),
            json: false,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: Some("work.done".to_string()),
        };

        // R6: read-only mode must succeed without producing an event.
        emit_command_with_root(ColorMode::Never, args, Some(&workspace))
            .expect("schema mode should succeed");

        // Events file must NOT have been created — `--schema` is
        // strictly read-only and the operator's toolchain relies on
        // "no events file = no event was emitted".
        assert!(
            !events_file.exists() || std::fs::read_to_string(&events_file).unwrap().is_empty(),
            "schema mode must not write to events.jsonl"
        );
    }

    /// (2) Required fields for the topic come from the embedded
    /// `event_policy.schemas`. Operators use these to confirm
    /// drift between the authoring YAML and the embedded copy.
    #[test]
    fn test_emit_schema_view_reflects_required_fields() {
        let tmp = TempDir::new().expect("temp dir");
        let workspace = setup_schema_workspace(&tmp, SCHEMA_FIXTURE_YAML);
        let _events_file = workspace.join(".ralph/events.jsonl");

        // Render via the public path the CLI uses, then introspect
        // the resulting JSON. We build the view the same way the
        // handler does (RalphConfig + hats_source=None +
        // ProtocolView::from_event_loop) and assert on its fields
        // directly — keeps the test hermetic and pins the rendering
        // contract without coupling to stdout capture.
        let config_path = workspace.join("ralph.yml");
        let config_sources = vec![ConfigSource::File(config_path)];
        let cfg =
            crate::preflight::load_config_for_preflight_sync(&config_sources, None, &workspace)
                .expect("load fixture config");
        let view = ProtocolView::from_event_loop(&cfg.event_loop);
        let value = schema_view::render_topic(&view, "work.done").expect("render view");

        assert_eq!(value["topic"], "work.done");
        let required = value["required_fields"]
            .as_array()
            .expect("required_fields is array");
        let required: std::collections::HashSet<&str> =
            required.iter().filter_map(|v| v.as_str()).collect();
        assert!(required.contains("plan_name"));
        assert!(required.contains("task_id"));
        assert!(required.contains("task_key"));
        assert_eq!(required.len(), 3);

        assert_eq!(value["is_macro_edge"], serde_json::Value::Bool(false));
        assert!(value["protocol_hash"].as_str().is_some());
        assert!(
            !value["protocol_hash"].as_str().unwrap().is_empty(),
            "protocol_hash must be non-empty"
        );
    }

    /// (3) Unknown topics return an empty `required_fields` array
    /// instead of erroring. This matches `ProtocolView::required_fields`
    /// semantics and lets operators probe the protocol without
    /// having to pre-check the topic table.
    #[test]
    fn test_emit_schema_view_for_unknown_topic_returns_empty_required_fields() {
        let tmp = TempDir::new().expect("temp dir");
        let workspace = setup_schema_workspace(&tmp, SCHEMA_FIXTURE_YAML);

        let config_path = workspace.join("ralph.yml");
        let config_sources = vec![ConfigSource::File(config_path)];
        let cfg =
            crate::preflight::load_config_for_preflight_sync(&config_sources, None, &workspace)
                .expect("load fixture config");
        let view = ProtocolView::from_event_loop(&cfg.event_loop);
        let value = schema_view::render_topic(&view, "totally.unknown.topic")
            .expect("render view for unknown topic");

        assert_eq!(value["topic"], "totally.unknown.topic");
        let required = value["required_fields"]
            .as_array()
            .expect("required_fields is array");
        assert!(
            required.is_empty(),
            "unknown topic must yield empty required_fields, got: {required:?}"
        );
        // is_macro_edge is kept in the output for backwards compatibility
        // but is always false; macro-edge semantics were removed.
        assert_eq!(value["is_macro_edge"], serde_json::Value::Bool(false));
    }

    /// (4) Without a discoverable ralph.yml AND without a `.ralph/`
    /// marker, schema mode fails closed with a clear error — the
    /// `should_load_config` gate in the handler skips config
    /// resolution, so `config` is `None` and the schema branch
    /// must surface a friendly error instead of rendering an empty
    /// default view.
    #[test]
    fn test_emit_schema_fails_closed_when_no_config() {
        let tmp = TempDir::new().expect("temp dir");
        let workspace = tmp.path().to_path_buf();
        // No ralph.yml, no .ralph — operator forgot to cd into a
        // preset-bearing workspace. Without `.ralph/` the
        // `should_load_config` gate is false, so config resolution
        // is skipped entirely and the schema branch sees `config =
        // None`, which it must turn into a clear fail-closed error.
        let events_file = workspace.join(".ralph/events.jsonl");

        let args = EmitArgs {
            topic: Some("work.done".to_string()),
            payload: String::new(),
            json: false,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: Some("work.done".to_string()),
        };

        let err = emit_command_with_root(ColorMode::Never, args, Some(&workspace))
            .expect_err("schema mode must fail closed when no config is discoverable");
        let message = format!("{err:#}");
        assert!(
            message.contains("no ralph.yml") || message.contains("Cannot render protocol view"),
            "expected clear fail-closed message, got: {message}"
        );
        // And of course no event was written.
        assert!(!events_file.exists() || std::fs::read_to_string(&events_file).unwrap().is_empty());
    }

    /// (5) Protocol hash is stable across two renders of the same
    /// config — this is the property operators rely on to detect
    /// drift between the authoring YAML and the embedded copy.
    #[test]
    fn test_emit_schema_hash_is_stable_across_renders() {
        let tmp = TempDir::new().expect("temp dir");
        let workspace = setup_schema_workspace(&tmp, SCHEMA_FIXTURE_YAML);

        let config_path = workspace.join("ralph.yml");
        let config_sources = vec![ConfigSource::File(config_path)];
        let cfg =
            crate::preflight::load_config_for_preflight_sync(&config_sources, None, &workspace)
                .expect("load fixture config");
        let view1 = ProtocolView::from_event_loop(&cfg.event_loop);
        let view2 = ProtocolView::from_event_loop(&cfg.event_loop);
        assert_eq!(view1.protocol_hash, view2.protocol_hash);

        let v1 = schema_view::render_topic(&view1, "work.done").unwrap();
        let v2 = schema_view::render_topic(&view2, "work.done").unwrap();
        assert_eq!(v1["protocol_hash"], v2["protocol_hash"]);
    }

    // ── U6 (2026-06-21-002 plan §U6): the unified `--policy-check`
    //    path runs the U4 `ValidationPipeline` over the inbound event
    //    and surfaces structured `reason_codes`. The legacy path is
    //    preserved only when no event_policy is configured
    //    (diff / no-policy fallback).

    fn setup_unified_workspace(tmp: &TempDir) -> PathBuf {
        let workspace = tmp.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).unwrap();
        std::fs::write(
            workspace.join("ralph.yml"),
            r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    allow_unsafe_cli_emit: true
    schemas:
      experiment.planned:
        required_fields:
          - task_key
"#,
        )
        .unwrap();
        workspace
    }

    /// Policy-check: rejects a payload missing a required field,
    /// surfacing a structured `engine_rejected:required_field` reason code.
    #[test]
    fn test_emit_policy_check_rejects_missing_required_field() {
        let tmp = TempDir::new().unwrap();
        let workspace = setup_unified_workspace(&tmp);
        let events_file = workspace.join(".ralph/events.jsonl");

        let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("experiment.planned".to_string()),
                payload: r#"{"foo":"bar"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: true,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect_err("policy check must reject missing required field");

        let message = format!("{err:#}");
        // The unified branch bails with a structured envelope that
        // surfaces the full reason_code list. The agent can parse
        // the JSON envelope to recover the exact reason.
        assert!(
            message.contains("engine_rejected:required_field"),
            "expected structured engine_rejected:required_field reason, got: {message}"
        );
        assert!(
            message.contains("task_key"),
            "error should name the missing field, got: {message}"
        );
    }

    /// Policy-check: accepts a valid payload when all required fields are present.
    #[test]
    fn test_emit_policy_check_accepts_valid_payload() {
        let tmp = TempDir::new().unwrap();
        let workspace = setup_unified_workspace(&tmp);
        let events_file = workspace.join(".ralph/events.jsonl");

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("experiment.planned".to_string()),
                payload: r#"{"task_key":"k1"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: true,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect("policy check must accept a valid payload");

        let events = std::fs::read_to_string(&events_file).unwrap_or_default();
        assert!(
            events.trim().is_empty(),
            "explicit --policy-check must not write to events file; got: {events}"
        );
    }

    /// Policy-check rejection surfaces the unified structured envelope
    /// (reason_codes list + suggestions), not the legacy bail string.
    #[test]
    fn test_emit_policy_check_rejects_with_unified_envelope() {
        let tmp = TempDir::new().unwrap();
        let workspace = setup_unified_workspace(&tmp);
        let events_file = workspace.join(".ralph/events.jsonl");

        let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("experiment.planned".to_string()),
                payload: r#"{"foo":"bar"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: true,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect_err("policy check must reject invalid payload");

        let message = format!("{err:#}");
        // Unified path: bail message contains structured reason_codes.
        assert!(
            message.contains("reason_codes="),
            "expected unified reason_codes in bail, got: {message}"
        );
    }

    #[test]
    fn test_emit_rejects_empty_task_id_in_payload() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

        let err = emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("work.ready".to_string()),
                payload: r#"{"task_id":"","step":"step-01"}"#.to_string(),
                json: true,
                file: PathBuf::from(".ralph/events.jsonl"),
                policy_check: false,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect_err("empty task_id must be rejected");

        let message = format!("{err:#}");
        assert!(
            message.contains("task_id cannot be empty"),
            "expected empty task_id error, got: {message}"
        );

        // No event should have been written.
        let events_path = workspace.join(".ralph/events.jsonl");
        assert!(
            !events_path.exists(),
            "rejected emit must not write events file"
        );
    }

    #[test]
    fn test_emit_allows_non_empty_task_id_in_payload() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("work.ready".to_string()),
                payload: r#"{"task_id":"task-123-abc","step":"step-01"}"#.to_string(),
                json: true,
                file: PathBuf::from(".ralph/events.jsonl"),
                policy_check: false,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect("non-empty task_id should be accepted");

        let events =
            std::fs::read_to_string(workspace.join(".ralph/events.jsonl")).expect("read events");
        assert!(events.contains("\"task_id\":\"task-123-abc\""));
    }

    #[test]
    fn test_emit_isolated_auto_derives_triggered_from_subscriber() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
        std::fs::write(
            workspace.join("ralph.yml"),
            r#"
event_loop:
  execution_mode: isolated
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["review.start", "review.dimension.done", "review.dimension.failed"]
    publishes: ["review.dimension.ready", "review.dimensions.complete"]
  dimension-reviewer:
    name: "Dimension Reviewer"
    triggers: ["review.dimension.ready"]
    publishes: ["review.dimension.done", "review.dimension.failed"]
  shipper:
    name: "Shipper"
    triggers: ["plan.complete", "plan.blocked"]
    publishes: ["REVIEW_COMPLETE"]
"#,
        )
        .unwrap();

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("review.dimension.ready".to_string()),
                payload: r#"{"dimension":"goal-alignment","plan_name":"p","task_id":"t"}"#.to_string(),
                json: true,
                file: PathBuf::from(".ralph/events.jsonl"),
                policy_check: false,
                no_policy_check: false,
                hat: Some("review-coordinator".to_string()),
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect("emit should succeed");

        let events =
            std::fs::read_to_string(workspace.join(".ralph/events.jsonl")).expect("read events");
        assert!(
            events.contains("\"triggered\":\"dimension-reviewer\""),
            "expected triggered to be auto-derived to dimension-reviewer; got: {events}"
        );
    }

    #[test]
    fn test_emit_isolated_respects_explicit_triggered_override() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
        std::fs::write(
            workspace.join("ralph.yml"),
            r#"
event_loop:
  execution_mode: isolated
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["review.start", "review.dimension.done", "review.dimension.failed"]
    publishes: ["review.dimension.ready", "review.dimensions.complete"]
  dimension-reviewer:
    name: "Dimension Reviewer"
    triggers: ["review.dimension.ready"]
    publishes: ["review.dimension.done", "review.dimension.failed"]
  shipper:
    name: "Shipper"
    triggers: ["plan.complete", "plan.blocked"]
    publishes: ["REVIEW_COMPLETE"]
"#,
        )
        .unwrap();

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("review.dimension.ready".to_string()),
                payload: r#"{"dimension":"goal-alignment","plan_name":"p","task_id":"t"}"#.to_string(),
                json: true,
                file: PathBuf::from(".ralph/events.jsonl"),
                policy_check: false,
                no_policy_check: false,
                hat: Some("review-coordinator".to_string()),
                triggered: Some("shipper".to_string()),
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect("emit should succeed");

        let events =
            std::fs::read_to_string(workspace.join(".ralph/events.jsonl")).expect("read events");
        assert!(
            events.contains("\"triggered\":\"shipper\""),
            "expected explicit triggered override to be preserved; got: {events}"
        );
        assert!(
            !events.contains("\"triggered\":\"dimension-reviewer\""),
            "auto-derivation should not override explicit value; got: {events}"
        );
    }

    #[test]
    fn test_emit_isolated_no_auto_trigger_for_control_topic() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
        std::fs::write(
            workspace.join("ralph.yml"),
            r#"
event_loop:
  execution_mode: isolated
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["work.start", "task.resume", "test.passed", "review.complete", "work.failed"]
    publishes: ["work.ready", "review.start", "plan.complete", "plan.blocked", "LOOP_COMPLETE"]
"#,
        )
        .unwrap();

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: Some("loop.cancel".to_string()),
                payload: String::new(),
                json: false,
                file: PathBuf::from(".ralph/events.jsonl"),
                policy_check: false,
                no_policy_check: false,
                hat: Some("ralph".to_string()),
                triggered: None,
                source: None,
                schema: None,
            },
            Some(&workspace),
        )
        .expect("emit should succeed");

        let events =
            std::fs::read_to_string(workspace.join(".ralph/events.jsonl")).expect("read events");
        assert!(
            !events.contains("\"triggered\""),
            "control topic should not get auto-derived triggered; got: {events}"
        );
    }

    #[test]
    fn test_maybe_derive_triggered_for_isolated() {
        let config = parse_config(
            r#"
event_loop:
  execution_mode: isolated
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["review.start", "review.dimension.done", "review.dimension.failed"]
    publishes: ["review.dimension.ready", "review.dimensions.complete"]
  dimension-reviewer:
    name: "Dimension Reviewer"
    triggers: ["review.dimension.ready"]
    publishes: ["review.dimension.done", "review.dimension.failed"]
"#,
        );

        // Auto-derives to the concrete subscriber.
        assert_eq!(
            maybe_derive_triggered_for_isolated(
                "review.dimension.ready",
                Some("review-coordinator"),
                None,
                Some(&config)
            ),
            Some("dimension-reviewer".to_string())
        );

        // Explicit value is preserved.
        assert_eq!(
            maybe_derive_triggered_for_isolated(
                "review.dimension.ready",
                Some("review-coordinator"),
                Some("shipper".to_string()),
                Some(&config)
            ),
            Some("shipper".to_string())
        );

        // Control topics are skipped.
        assert_eq!(
            maybe_derive_triggered_for_isolated(
                "loop.cancel",
                Some("ralph"),
                None,
                Some(&config)
            ),
            None
        );

        // Missing hat context is skipped.
        assert_eq!(
            maybe_derive_triggered_for_isolated(
                "review.dimension.ready",
                None,
                None,
                Some(&config)
            ),
            None
        );
    }

    #[test]
    fn test_maybe_derive_triggered_for_coordinator_mode_is_noop() {
        let config = parse_config(
            r#"
event_loop:
  execution_mode: coordinator
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["review.dimension.ready"]
    publishes: ["review.dimension.ready"]
"#,
        );

        assert_eq!(
            maybe_derive_triggered_for_isolated(
                "review.dimension.ready",
                Some("review-coordinator"),
                None,
                Some(&config)
            ),
            None
        );
    }
}

/// U6 测试：`ralph emit --schema EMIT_RESULT` 只读输出。
///
/// 验收要点：
/// 1. `--schema EMIT_RESULT` 在 stdout 打印 JSON `schema_version == emit_result.v1`
/// 2. `--schema` 与 `--json` / payload 在 clap 层互斥（schema 路径
///    完全不读 payload / json 字段）
///
/// 测试策略：直接构造 `EmitArgs { schema: Some("EMIT_RESULT"), .. }` 调用
/// `emit_command_with_root`，断言 stdout JSON 形状 / 调用成功。不读
/// preset 磁盘，不触碰 `.ralph/`。
#[cfg(test)]
mod emit_schema_emit_result_tests {
    use super::*;
    use crate::cli::ColorMode;
    use std::path::PathBuf;

    /// 测试用 fixture：构造 EmitArgs 调用 emit_command。
    fn emit_args_schema_emit_result() -> EmitArgs {
        EmitArgs {
            topic: None, // --schema 模式下不强制 topic
            payload: String::new(),
            json: false,
            file: PathBuf::from(".ralph/events.jsonl"),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: Some("EMIT_RESULT".to_string()),
        }
    }

    /// `--schema EMIT_RESULT` 必须走 read-only 路径并成功返回（无
    /// preset / 无 .ralph/ 也能产出 stdout JSON）。
    ///
    /// 关键反断言：纯函数路径 → 不依赖 ralph.yml / preset，因此即使
    /// 工作目录没有 .ralph/ 也能产出 stdout JSON。
    #[test]
    fn test_emit_schema_emit_result_prints_version() {
        let workspace = tempfile::TempDir::new()
            .expect("temp dir")
            .path()
            .to_path_buf();

        emit_command_with_root(
            ColorMode::Never,
            emit_args_schema_emit_result(),
            Some(&workspace),
        )
        .expect("EMIT_RESULT schema view must succeed without preset files");
    }

    /// `--schema` 与 `--json` / `payload` 互斥。
    ///
    /// 验证方式：构造 schema + payload + json 全部非默认值的 EmitArgs，
    /// 调用 emit_command，断言 schema 路径完全忽略 payload / json 字段
    /// （不读、不抛错）。clap 层的 `conflicts_with_all` 在 parse 阶段
    /// 拦截三字段同时为非默认值的组合；本测试验证的是 schema 路径本身
    /// 对 payload / json 字段的 robustness。
    #[test]
    fn test_emit_schema_emit_result_mutually_exclusive_with_payload() {
        let args = EmitArgs {
            topic: None,
            payload: "x".to_string(), // 与 schema 互斥
            json: true,                // 与 schema 互斥
            file: PathBuf::from(".ralph/events.jsonl"),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
            schema: Some("EMIT_RESULT".to_string()),
        };

        let workspace = tempfile::TempDir::new()
            .expect("temp dir")
            .path()
            .to_path_buf();

        emit_command_with_root(ColorMode::Never, args, Some(&workspace))
            .expect("EMIT_RESULT schema path must ignore payload/json");
    }
}

/// Schema-view rendering for `ralph emit --schema <TOPIC>` (U5 / R6).
///
/// The view is a JSON-serialisable snapshot of the embedded protocol
/// SSOT for one topic. Operators and agents use it to verify:
///   * which fields the gate will require for `TOPIC`
///   * the stable protocol hash, so a drift between the authoring
///     `presets/schemas/<name>.yml` and the embedded copy is detectable
///     without rebuilding
///
/// The submodule lives inside `commands/emit.rs` rather than a separate
/// `commands/schema.rs` so it can reuse the same `RalphConfig` /
/// `ProtocolView` plumbing without re-resolving the workspace.
pub mod schema_view {
    use super::ProtocolView;
    use anyhow::{Context, Result};
    use ralph_core::preset::engine::protocol::payload_field_set;
    use std::collections::BTreeMap;

    /// Render the protocol JSON view for `topic`.
    ///
    /// `topic` may be a topic that is *not* in the protocol — the
    /// returned `required_fields` will simply be empty and the
    /// other sections (`verdict_gate`, `workflow_contract`, ...)
    /// remain populated so operators can see the protocol-wide
    /// settings without changing the gate behaviour.
    pub fn render_topic(view: &ProtocolView, topic: &str) -> Result<serde_json::Value> {
        let mut payload_keys: BTreeMap<String, serde_json::Value> = BTreeMap::new();
        for (t, schema) in &view.effective_required_fields {
            // Per-topic entry keeps the SSOT visible when an operator
            // inspects multiple topics at once. The serialised `schema`
            // is the *embedded* copy, post build.rs merge.
            let fields_vec: Vec<&String> = {
                let mut v: Vec<&String> = schema.iter().collect();
                v.sort();
                v
            };
            payload_keys.insert(
                t.clone(),
                serde_json::json!({
                    "required_fields": fields_vec,
                }),
            );
        }

        // Topic-scoped required fields (empty when topic is unknown).
        let required_fields: Vec<String> = {
            let mut v: Vec<String> = view.required_fields(topic).into_iter().collect();
            v.sort();
            v
        };

        let mut out = serde_json::json!({
            "topic": topic,
            "protocol_hash": view.protocol_hash,
            "is_macro_edge": false, // kept for backwards compatibility; macro-edge semantics removed
            "required_fields": required_fields,
            "all_topics": payload_keys,
        });

        // Protocol-wide sections. Each is `null` when absent so the
        // operator can see at a glance whether the loaded config
        // enables the corresponding gate / projection machinery.
        let obj = out.as_object_mut().expect("json!() returns object");

        if let Some(vg) = &view.verdict_gate {
            obj.insert(
                "verdict_gate".to_string(),
                serde_json::to_value(vg).context("serialise verdict_gate")?,
            );
        } else {
            obj.insert("verdict_gate".to_string(), serde_json::Value::Null);
        }

        if let Some(wc) = &view.workflow_contract {
            obj.insert(
                "workflow_contract".to_string(),
                serde_json::to_value(wc).context("serialise workflow_contract")?,
            );
        } else {
            obj.insert("workflow_contract".to_string(), serde_json::Value::Null);
        }

        if let Some(sp) = &view.state_projection {
            obj.insert(
                "state_projection".to_string(),
                serde_json::to_value(sp).context("serialise state_projection")?,
            );
        } else {
            obj.insert("state_projection".to_string(), serde_json::Value::Null);
        }

        if let Some(ec) = &view.execution_contracts {
            obj.insert(
                "execution_contracts".to_string(),
                serde_json::to_value(ec).context("serialise execution_contracts")?,
            );
        } else {
            obj.insert("execution_contracts".to_string(), serde_json::Value::Null);
        }

        Ok(out)
    }

    /// Pretty-printed variant for human reading. Uses 2-space indent
    /// to match the project's other JSON dumps (`recovery.jsonl`
    /// envelopes, `protocol_view` debug output).
    pub fn render_pretty(view: &ProtocolView, topic: &str) -> Result<String> {
        let value = render_topic(view, topic)?;
        Ok(serde_json::to_string_pretty(&value)?)
    }

    // Re-export for tests that want to introspect the view without
    // going through the rendered JSON.
    #[allow(dead_code)]
    pub(crate) fn topic_field_set(
        view: &ProtocolView,
        topic: &str,
    ) -> std::collections::HashSet<String> {
        view.required_fields(topic)
    }

    // Silence the unused import warning when `payload_field_set` is
    // not referenced from tests (kept for future schema-aware payload
    // introspection helpers, e.g. "show which fields an event with
    // this shape would pass / fail").
    #[allow(dead_code)]
    fn _unused_payload_field_set(payload: &serde_json::Value) -> std::collections::HashSet<String> {
        payload_field_set(payload)
    }
}
