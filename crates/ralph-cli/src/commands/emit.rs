use crate::cli::{
    ColorMode, ConfigSource, HatsSource, resolve_emit_path, resolve_marker_target,
    resolve_workspace_root, urgent_steer_path_from_workspace,
};
use crate::config_resolution;
use crate::display::colors;
use crate::policy_check::{PolicyCheckFlags, resolve_policy_check_mode};
use anyhow::{Context, Result};
use clap::Parser;
use ralph_core::config::HatExecutionMode;
use ralph_core::emit_schema_hint::fix_hint_for_hat_topic;
use ralph_core::{
    RalphConfig, UrgentSteerStore,
    diagnosis::{
        DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource, EvidenceKind, EvidenceRef,
        RecoveryDiagnosisEnvelope, RecoveryJournalEntry,
    },
};
use ralph_proto::{Hat, HatId, Topic};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Arguments for the emit subcommand.
#[derive(Parser, Debug)]
pub struct EmitArgs {
    /// Event topic (e.g., "build.done", "review.complete")
    pub topic: String,

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
pub fn should_policy_check_emit(args: &EmitArgs, config: Option<&RalphConfig>) -> PolicyCheckMode {
    let flags = PolicyCheckFlags {
        policy_check: args.policy_check,
        no_policy_check: args.no_policy_check,
    };
    resolve_policy_check_mode(&flags, config)
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

    let (severity, outcome, reason_code) = match &finding.violation_type {
        ralph_core::ViolationType::PayloadTypeMismatch { .. } => (
            DiagnosisSeverity::Critical,
            DiagnosisOutcome::NotRetriable,
            "payload_contract_violation".to_string(),
        ),
        _ => (
            DiagnosisSeverity::Error,
            DiagnosisOutcome::Failed,
            finding.violation_type.reason_code().to_string(),
        ),
    };

    let mut builder = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::CliEmit)
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

    // Determine whether policy validation is required.
    let check_mode = should_policy_check_emit(&args, config.as_ref());

    // Resolve provenance values: CLI flag > env var > empty
    let (hat, triggered, source) =
        resolve_provenance(args.hat.clone(), args.triggered, args.source, |key| {
            std::env::var(key).ok()
        });

    // Phase 2: in isolated mode the runner controls hat provenance. When the
    // agent is running inside a hat context (RALPH_CURRENT_HAT is set), the
    // CLI flag --hat is ignored and must not disagree with the environment.
    let env_hat = std::env::var("RALPH_CURRENT_HAT")
        .ok()
        .filter(|s| !s.is_empty());
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

    // Enforce provenance requirements when hat is missing.
    if hat.is_none() {
        let provenance_required = config
            .as_ref()
            .and_then(|c| c.event_loop.event_policy.as_ref())
            .map(|p| p.require_emit_provenance)
            .unwrap_or(false);
        if provenance_required {
            anyhow::bail!(
                "Event provenance required: --hat <hat-id> or RALPH_CURRENT_HAT must be set."
            );
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
        && !ralph_core::event_origin::RALPH_CONTROL_TOPICS
            .iter()
            .any(|t| *t == args.topic.as_str())
    {
        anyhow::bail!(
            "Builtin ralph hat may only emit control topics: {:?}. \
             Topic '{}' is a business topic and cannot be emitted by ralph. \
             Set --hat to a registered workflow hat (e.g. coordinator, executor, \
             review-synthesizer) instead.",
            ralph_core::event_origin::RALPH_CONTROL_TOPICS,
            args.topic
        );
    }

    if check_mode != PolicyCheckMode::Skip {
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
            use ralph_core::{PolicyRuntimeState, check_topic_deny_rules, validate_event_with_hat};
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
            if let Some(decision) = check_topic_deny_rules(hat.as_deref(), &args.topic, policy) {
                match decision {
                    ralph_core::PolicyDecision::Accept => {}
                    ralph_core::PolicyDecision::Warn(findings) => {
                        for finding in findings {
                            eprintln!("Policy warning: {}", finding.message);
                        }
                    }
                    ralph_core::PolicyDecision::RejectWithResume(finding)
                    | ralph_core::PolicyDecision::Hold(finding)
                    | ralph_core::PolicyDecision::Block(finding)
                    | ralph_core::PolicyDecision::Ignore(finding) => {
                        record_cli_emit_rejection(
                            &workspace_root,
                            &args.topic,
                            hat.as_deref(),
                            &finding,
                        );
                        anyhow::bail!(
                            "Event rejected by policy: {}. Fix the issue before emitting.\n\n{}",
                            finding.message,
                            format_fix_hint(config.as_ref().unwrap(), hat.as_deref(), &args.topic)
                        );
                    }
                }
            }

            // Run schema validation with hat-aware restrictions.
            let decision = validate_event_with_hat(
                &args.topic,
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
                ralph_core::PolicyDecision::RejectWithResume(finding)
                | ralph_core::PolicyDecision::Hold(finding)
                | ralph_core::PolicyDecision::Block(finding)
                | ralph_core::PolicyDecision::Ignore(finding) => {
                    record_cli_emit_rejection(
                        &workspace_root,
                        &args.topic,
                        hat.as_deref(),
                        &finding,
                    );
                    anyhow::bail!(
                        "Event rejected by policy: {}. Fix the issue before emitting.\n\n{}",
                        finding.message,
                        format_fix_hint(config.as_ref().unwrap(), hat.as_deref(), &args.topic)
                    );
                }
            }
        }
    } else if check_mode == PolicyCheckMode::Skip {
        tracing::info!("cli emit policy check skipped: no event_policy in resolved config");
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

    let mut record = serde_json::json!({
        "topic": args.topic,
        "payload": payload_value,
        "ts": ts
    });

    // Add provenance fields only when they have values (preserve old simple schema)
    if let Some(hat) = hat {
        record["hat"] = serde_json::Value::String(hat);
    }
    if let Some(triggered) = triggered {
        record["triggered"] = serde_json::Value::String(triggered);
    }
    if let Some(source) = source {
        record["source"] = serde_json::Value::String(source);
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
            args.topic
        );
    } else {
        println!("Event emitted: {}", args.topic);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::load_config_with_overrides;
    use std::path::PathBuf;
    use tempfile::TempDir;
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
                topic: "debug.step".to_string(),
                payload: "task_id=demo".to_string(),
                json: false,
                file: PathBuf::from(".ralph/events.jsonl"),
                policy_check: false,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
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
                topic: "debug.step".to_string(),
                payload: "task_id=demo".to_string(),
                json: false,
                file: PathBuf::from(".ralph/events.jsonl"),
                policy_check: false,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
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
                topic: "experiment.planned".to_string(),
                payload: "{}".to_string(),
                json: true,
                file: PathBuf::from(".ralph/events.jsonl"),
                policy_check: true,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
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
                topic: "experiment.planned".to_string(),
                payload: "{}".to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: true,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
            },
            Some(&workspace),
        )
        .expect("should accept business event when no terminal exists");

        let events = std::fs::read_to_string(&events_file).expect("read events");
        assert!(events.contains("experiment.planned"));
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
                topic: "experiment.planned".to_string(),
                payload: "{}".to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: true,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
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

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: "experiment.planned".to_string(),
                payload: r#"{"task_key":"x"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("strategist".to_string()),
                triggered: Some("implementer".to_string()),
                source: Some("cli".to_string()),
            },
            Some(&workspace),
        )
        .expect("emit with provenance should succeed");

        let events = std::fs::read_to_string(&events_file).expect("read events");
        assert!(events.contains("\"hat\":\"strategist\""));
        assert!(events.contains("\"triggered\":\"implementer\""));
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
                topic: "review.passed".to_string(),
                payload: r#"{"plan_name":"p","task_id":"t","task_key":"k","step":"s","findings_count":0,"fix_round":0,"verdict":"pass","skip_reason":"empty_diff"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("ralph".to_string()),
                triggered: None,
                source: None,
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
                topic: "work.start".to_string(),
                payload: String::new(),
                json: false,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("ralph".to_string()),
                triggered: None,
                source: None,
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
                topic: "LOOP_COMPLETE".to_string(),
                payload: r#"{"reason":"done"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("ralph".to_string()),
                triggered: None,
                source: None,
            },
            Some(&workspace),
        )
        .expect("ralph hat must be allowed to emit LOOP_COMPLETE (control topic)");

        let events = std::fs::read_to_string(&events_file).expect("read events");
        assert!(events.contains("\"topic\":\"LOOP_COMPLETE\""));
        assert!(events.contains("\"hat\":\"ralph\""));
    }

    #[test]
    fn test_emit_ralph_hat_allows_control_topic_human_guidance() {
        let temp_dir = TempDir::new().expect("temp dir");
        let workspace = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(workspace.join(".ralph")).expect("ralph dir");
        let events_file = workspace.join(".ralph/events.jsonl");

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: "human.guidance".to_string(),
                payload: r#"{"messages":["continue"]}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("ralph".to_string()),
                triggered: None,
                source: None,
            },
            Some(&workspace),
        )
        .expect("ralph hat must be allowed to emit human.guidance (control topic)");

        let events = std::fs::read_to_string(&events_file).expect("read events");
        assert!(events.contains("human.guidance"));
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
                topic: "task.resume".to_string(),
                payload: r#"{"reason":"recover"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("ralph".to_string()),
                triggered: None,
                source: None,
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
                topic: "work.done".to_string(),
                payload: r#"{"plan_name":"p","plan_path":"x.md","task_id":"t","task_key":"k","step":"s","commit_count":1,"changed_lines":10}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("executor".to_string()),
                triggered: None,
                source: None,
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
                topic: "debug.step".to_string(),
                payload: "task_id=demo".to_string(),
                json: false,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
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
                topic: "build.done".to_string(),
                payload: String::new(),
                json: false,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
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
                topic: "build.done".to_string(),
                payload: String::new(),
                json: false,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("strategist".to_string()),
                triggered: None,
                source: None,
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
                topic: "experiment.planned".to_string(),
                payload: r#"{"task_key":"x"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
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
                topic: "LOOP_COMPLETE".to_string(),
                payload: r#"{"reason":"done"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
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
                topic: "build.done".to_string(),
                payload: String::new(),
                json: false,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
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
                topic: "experiment.planned".to_string(),
                payload: "{}".to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: true,
                no_policy_check: false,
                hat: None,
                triggered: None,
                source: None,
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
                topic: "LOOP_COMPLETE".to_string(),
                payload: r#"{"reason":"retry"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: true,
                hat: None,
                triggered: None,
                source: None,
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
                topic: "LOOP_COMPLETE".to_string(),
                payload: r#"{"reason":"retry"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: true,
                hat: None,
                triggered: None,
                source: None,
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
                topic,
                payload,
                json,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("strategist".to_string()),
                triggered: None,
                source: Some("cli".to_string()),
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
                topic,
                payload,
                json,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("strategist".to_string()),
                triggered: None,
                source: None,
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
                topic,
                payload,
                json,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("strategist".to_string()),
                triggered: None,
                source: None,
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
                topic,
                payload,
                json,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: None, // missing provenance
                triggered: None,
                source: None,
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
            let loop_accept = matches!(
                loop_decision,
                PolicyDecision::Accept | PolicyDecision::Warn(_)
            );

            // -- CLI path --
            let (cli_topic, cli_payload, cli_json) = parse_last_fixture_event(fixture);
            let cli_result = emit_command_with_root(
                ColorMode::Never,
                EmitArgs {
                    topic: cli_topic,
                    payload: cli_payload,
                    json: cli_json,
                    file: events_file.clone(),
                    policy_check: false,
                    no_policy_check: false,
                    hat: Some("strategist".to_string()),
                    triggered: None,
                    source: None,
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

        emit_command_with_root(
            ColorMode::Never,
            EmitArgs {
                topic: "experiment.planned".to_string(),
                payload: r#"{"task_key":"x"}"#.to_string(),
                json: true,
                file: events_file.clone(),
                policy_check: false,
                no_policy_check: false,
                hat: Some("strategist".to_string()),
                triggered: Some("implementer".to_string()),
                source: Some("cli".to_string()),
            },
            Some(&workspace),
        )
        .expect("emit should succeed");

        let mut reader = ralph_core::EventReader::new(&events_file);
        let result = reader.read_new_events().unwrap();
        assert_eq!(result.events.len(), 1);
        let event = &result.events[0];
        assert_eq!(event.hat, Some("strategist".to_string()));
        assert_eq!(event.triggered, Some("implementer".to_string()));
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
            topic: "work.done".to_string(),
            payload: r#"{"plan_name":"test","task_id":"t1"}"#.to_string(),
            json: false,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
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
            topic: "build.done".to_string(),
            payload: "Build succeeded".to_string(),
            json: false,
            file: events_file.clone(),
            policy_check: false,
            no_policy_check: false,
            hat: None,
            triggered: None,
            source: None,
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
}
