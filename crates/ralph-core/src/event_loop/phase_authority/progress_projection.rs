//! 2026-07-02-006 plan U19: `apply_progress_on_phase_enter`.
//!
//! Pure function: `PhaseAuthorityConfig.progress_projection` +
//! phase id + step context → markdown fragment string. No
//! disk I/O. The runtime's `progress.md` writer concatenates
//! the returned fragment after writing the canonical
//! per-step lines; the function is the *per-phase* hook.

use super::config::ProgressProjectionConfig;

/// Inputs the runtime has when a phase is entered. The
/// function is pure; the runtime snapshots the relevant
/// state into this struct so the projection logic stays
/// deterministic.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PhaseEnterContext {
    /// Phase id the runtime is entering.
    pub phase_id: String,
    /// Id of the most recently completed step. `None` when
    /// no step has completed yet (cold start).
    pub last_completed_step: Option<String>,
    /// Whether the fix-unit queue is exhausted (drives the
    /// `active_fix_step` rendering).
    pub fix_unit_queue_exhausted: bool,
}

/// Render a markdown fragment for `phase.enter` per the
/// declaration's `progress_projection.on_enter` map.
///
/// `phase_id` is the key into the on_enter map; when the
/// map has no entry the function returns an empty string.
/// Recognised entries:
///
/// - `plan_end: { write_current_step: last_completed_step_or_none }`
/// - `fix_units: { write_current_step: active_fix_step }`
///
/// Future presets can add new keys by extending the match
/// below. Unknown keys render as the literal key followed
/// by a colon, which is the safe default for
/// forwards-compatible evolution.
pub fn apply_progress_on_phase_enter(
    cfg: &ProgressProjectionConfig,
    ctx: &PhaseEnterContext,
) -> String {
    let Some(entry) = cfg.on_enter.get(&ctx.phase_id) else {
        return String::new();
    };

    let entry_map = match entry.as_mapping() {
        Some(m) => m,
        None => return format!("{}: <invalid entry>\n", ctx.phase_id),
    };

    // `write_current_step` is the only directive the engine
    // understands today; other keys are silently ignored so
    // a future preset can experiment without breaking the
    // serial snapshot's behaviour.
    let directive = entry_map
        .get(&serde_yaml::Value::String(
            "write_current_step".to_string(),
        ))
        .and_then(|v| v.as_str());

    match directive {
        Some("last_completed_step_or_none") => render_current_step(
            &ctx.phase_id,
            ctx.last_completed_step.as_deref(),
        ),
        Some("active_fix_step") => render_current_step(
            &ctx.phase_id,
            if ctx.fix_unit_queue_exhausted {
                None
            } else {
                ctx.last_completed_step.as_deref()
            },
        ),
        Some(other) => format!("{}: write_current_step={}\n", ctx.phase_id, other),
        None => format!("{}:\n", ctx.phase_id),
    }
}

fn render_current_step(phase_id: &str, step: Option<&str>) -> String {
    match step {
        Some(s) => format!("{}: current_step={}\n", phase_id, s),
        None => format!("{}: current_step=<none>\n", phase_id),
    }
}

#[cfg(test)]
mod tests {
    use super::super::config::ProgressProjectionConfig;
    use super::*;
    use serde_yaml::Value;

    fn entry_with(directive: &str) -> Value {
        serde_yaml::from_str(&format!(
            "write_current_step: \"{}\"\n",
            directive
        ))
        .unwrap()
    }

    #[test]
    fn missing_phase_returns_empty_string() {
        let cfg = ProgressProjectionConfig::default();
        let ctx = PhaseEnterContext {
            phase_id: "review".to_string(),
            ..Default::default()
        };
        assert!(apply_progress_on_phase_enter(&cfg, &ctx).is_empty());
    }

    #[test]
    fn plan_end_renders_last_completed_step() {
        let mut cfg = ProgressProjectionConfig::default();
        cfg.on_enter.insert(
            "plan_end".to_string(),
            entry_with("last_completed_step_or_none"),
        );
        let ctx = PhaseEnterContext {
            phase_id: "plan_end".to_string(),
            last_completed_step: Some("step-03".to_string()),
            fix_unit_queue_exhausted: false,
        };
        let out = apply_progress_on_phase_enter(&cfg, &ctx);
        assert!(out.contains("plan_end"));
        assert!(out.contains("current_step=step-03"));
    }

    #[test]
    fn plan_end_with_no_step_renders_placeholder() {
        let mut cfg = ProgressProjectionConfig::default();
        cfg.on_enter.insert(
            "plan_end".to_string(),
            entry_with("last_completed_step_or_none"),
        );
        let ctx = PhaseEnterContext {
            phase_id: "plan_end".to_string(),
            last_completed_step: None,
            fix_unit_queue_exhausted: false,
        };
        let out = apply_progress_on_phase_enter(&cfg, &ctx);
        assert!(out.contains("current_step=<none>"));
    }

    #[test]
    fn fix_units_renders_active_fix_step_when_queue_open() {
        let mut cfg = ProgressProjectionConfig::default();
        cfg.on_enter.insert(
            "fix_units".to_string(),
            entry_with("active_fix_step"),
        );
        let ctx = PhaseEnterContext {
            phase_id: "fix_units".to_string(),
            last_completed_step: Some("step-05".to_string()),
            fix_unit_queue_exhausted: false,
        };
        let out = apply_progress_on_phase_enter(&cfg, &ctx);
        assert!(out.contains("current_step=step-05"));
    }

    #[test]
    fn fix_units_renders_placeholder_when_queue_exhausted() {
        let mut cfg = ProgressProjectionConfig::default();
        cfg.on_enter.insert(
            "fix_units".to_string(),
            entry_with("active_fix_step"),
        );
        let ctx = PhaseEnterContext {
            phase_id: "fix_units".to_string(),
            last_completed_step: Some("step-05".to_string()),
            fix_unit_queue_exhausted: true,
        };
        let out = apply_progress_on_phase_enter(&cfg, &ctx);
        assert!(out.contains("current_step=<none>"));
    }
}