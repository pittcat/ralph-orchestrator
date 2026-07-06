//! CLI emit 路径的 phase / `allowed_next` 解析（读 config + ledger，无 loop 内存态）。
//!
//! `ralph emit --output json` 在 policy-check / apply 成功时通过本模块把
//! `mechanism.phase_authority` 声明与 `.ralph/` ledger 上的
//! `workflow_phase` 投影合成 EmitResult 路由字段。

use std::path::Path;

use crate::RalphConfig;
use crate::event_loop::phase_authority::config::PhaseAuthorityConfig;
use crate::event_loop::phase_authority::declaration::PhaseAuthorityDeclaration;
use crate::event_loop::phase_authority::snapshot::PhaseSnapshot;
use crate::state::StateLedger;

use super::allowed_next::allowed_next_for_hat_phase;

/// phase + allowed_next 路由上下文（EmitResult 接线用）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmitRoutingContext {
    pub phase: String,
    pub allowed_next: Vec<String>,
}

/// 从 preset phase authority 配置 + workspace ledger 解析路由上下文。
pub fn resolve_emit_routing_context(
    phase_cfg: Option<&PhaseAuthorityConfig>,
    workspace: &Path,
    hat_id: Option<&str>,
) -> EmitRoutingContext {
    let Some(cfg) = phase_cfg.filter(|c| c.enabled) else {
        return EmitRoutingContext {
            phase: "unknown".to_string(),
            allowed_next: Vec::new(),
        };
    };
    let decl = match PhaseAuthorityDeclaration::try_from_config(cfg) {
        Ok(d) => d,
        Err(_) => {
            return EmitRoutingContext {
                phase: "unknown".to_string(),
                allowed_next: Vec::new(),
            };
        }
    };

    let ledger_phase = load_ledger_workflow_phase(workspace);
    let phase_id = ledger_phase
        .as_ref()
        .map(|s| s.phase_id.clone())
        .or_else(|| decl.initial_phase.clone())
        .unwrap_or_else(|| "unknown".to_string());

    let allowed_next = hat_id
        .map(|hat| allowed_next_for_hat_phase(hat, &phase_id, &decl))
        .unwrap_or_default();

    EmitRoutingContext {
        phase: phase_id,
        allowed_next,
    }
}

/// 从已加载的 [`RalphConfig`] 解析路由上下文。
pub fn resolve_emit_routing_from_config(
    config: Option<&RalphConfig>,
    workspace: &Path,
    hat_id: Option<&str>,
) -> EmitRoutingContext {
    let phase_cfg = config
        .and_then(|c| c.event_loop.mechanism.as_ref())
        .and_then(|m| m.phase_authority.as_ref());
    resolve_emit_routing_context(phase_cfg, workspace, hat_id)
}

fn load_ledger_workflow_phase(workspace: &Path) -> Option<PhaseSnapshot> {
    let events_path = workspace.join(".ralph/events.jsonl");
    if !events_path.exists() {
        return None;
    }
    StateLedger::replay_from_disk(workspace)
        .ok()
        .and_then(|snap| snap.workflow_phase)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_loop::phase_authority::config::{PhaseAuthorityConfig, PhaseDeclConfig};
    use std::collections::BTreeMap;

    fn minimal_phase_cfg() -> PhaseAuthorityConfig {
        let mut allowed_emits = BTreeMap::new();
        allowed_emits.insert(
            "coordinator".to_string(),
            vec!["work.ready".to_string(), "work.done".to_string()],
        );
        PhaseAuthorityConfig {
            enabled: true,
            initial_phase: Some("unit_loop".to_string()),
            phases: vec![PhaseDeclConfig {
                id: "unit_loop".to_string(),
                label: None,
                allowed_emits,
            }],
            transitions: vec![],
            violation_policy: Default::default(),
            progress_projection: Default::default(),
        }
    }

    #[test]
    fn test_resolve_emit_routing_disabled_phase_authority_yields_unknown() {
        let mut cfg = minimal_phase_cfg();
        cfg.enabled = false;
        let ctx =
            resolve_emit_routing_context(Some(&cfg), Path::new("/tmp/nope"), Some("coordinator"));
        assert_eq!(ctx.phase, "unknown");
        assert!(ctx.allowed_next.is_empty());
    }

    #[test]
    fn test_resolve_emit_routing_uses_initial_phase_and_allowed_next() {
        let cfg = minimal_phase_cfg();
        let ctx =
            resolve_emit_routing_context(Some(&cfg), Path::new("/tmp/nope"), Some("coordinator"));
        assert_eq!(ctx.phase, "unit_loop");
        assert!(ctx.allowed_next.contains(&"work.ready".to_string()));
        assert!(ctx.allowed_next.contains(&"work.done".to_string()));
    }

    #[test]
    fn test_resolve_emit_routing_without_hat_yields_empty_allowed_next() {
        let cfg = minimal_phase_cfg();
        let ctx = resolve_emit_routing_context(Some(&cfg), Path::new("/tmp/nope"), None);
        assert_eq!(ctx.phase, "unit_loop");
        assert!(ctx.allowed_next.is_empty());
    }
}
