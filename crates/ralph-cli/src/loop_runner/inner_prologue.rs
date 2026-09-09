//! Startup prologue helpers extracted from `inner.rs` to keep that
//! file under the 5000-line hard cap. Behaviour is unchanged — these
//! are the exact blocks `run_loop_impl_inner` used to run inline.

use std::sync::Arc;

use anyhow::Result;
use ralph_core::config::{HatConfig, RalphConfig};

use super::payload_contract_gate::enforce_payload_contract_gate;
use super::preset_lint_gate::{
    enforce_preset_lint_gate_with_preset_name, write_preset_lint_artifact,
};

/// Startup hard gates (U5 payload contract + U4 preset static lint).
/// Both run BEFORE any backend is spawned; in strict mode (always on
/// for `ralph run`) any error is fatal.
///
/// P1 finding #5: the lint failure path propagates a typed
/// `PresetLintGateError` instead of calling `std::process::exit`.
/// `process::exit` would skip the RAII drop chain (tracing flush,
/// scoped guards, lock release) and is hostile to any future
/// `TempDir` / `LockGuard` / `tracing::subscriber::with_default`
/// added near the top of `run_loop_impl`. The outer `Result`-driven
/// flow maps the error to exit code 2 *after* drops have run; see
/// `commands::run::run_command` and `main.rs`.
///
/// WRC-U3: `source_is_builtin_embedded` scopes the WAC severity
/// upgrade (KTD-7) to builtin presets even outside `--strict` mode.
/// 2026-07-09-001 plan (U1 / A7): `hats_source_label` scopes the U7
/// emit-feedback lint gate to the preset whitelist; without it
/// `preset_name` resolves to `""` inside the gate and the lint
/// silently bypasses the rule on
/// `ralph run -H builtin:ce-executor-pipeline-loop`.
pub(super) fn run_startup_gates(
    config: &RalphConfig,
    source_is_builtin_embedded: bool,
    hats_source_label: Option<&str>,
) -> Result<()> {
    enforce_payload_contract_gate(config)?;
    if let Err(lint_error) = enforce_preset_lint_gate_with_preset_name(
        config,
        source_is_builtin_embedded,
        hats_source_label,
    ) {
        let diagnostics_dir = std::path::Path::new(".").join(".ralph").join("diagnostics");
        let _artifact_path = write_preset_lint_artifact(&diagnostics_dir, &lint_error);
        eprintln!(
            "\nPreset lint gate failed with {} error(s). No backend was started.\n\
             Fix the preset configuration and retry.",
            lint_error.error_count
        );
        return Err(anyhow::Error::new(lint_error));
    }
    Ok(())
}

/// U5/U6 production wiring (P1.1–P1.4): construct the `DriftEngine`
/// that owns the drift observer, detector, and per-iteration
/// responder glue. Enabled iff `telemetry.runtime_diagnosis.enabled`;
/// when disabled (the default) every per-iteration method is a cheap
/// no-op so the loop runs unchanged.
pub(super) fn build_drift_engine(config: &RalphConfig) -> ralph_core::drift::DriftEngine {
    let telemetry_config = Arc::new(config.telemetry.runtime_diagnosis.clone());
    if telemetry_config.enabled {
        let required_fields = ralph_core::drift::engine::required_fields_from_config(
            config.event_loop.event_policy.as_ref(),
            config.event_loop.execution_contracts.as_ref(),
        );
        let hat_configs: Vec<HatConfig> = config.hats.values().cloned().collect();
        let declared_edges = ralph_core::drift::engine::declared_edges_from_hats(&hat_configs);
        ralph_core::drift::DriftEngine::enabled(telemetry_config, required_fields, declared_edges)
    } else {
        ralph_core::drift::DriftEngine::disabled(telemetry_config)
    }
}
