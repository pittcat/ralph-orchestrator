//! L2 CLI ACL — `HatCommandPolicy`.
//!
//! Plan ref: U3 of `docs/plans/2026-07-04-001-feat-opac-isolated-agent-discipline-plan.md`.
//!
//! This module derives command-level authorization from the
//! resolved `RalphConfig` so that:
//!
//! - the L2 ACL is consistent across `ralph tools task *` and the
//!   `ralph wave emit` path (U23),
//! - no hat name is hardcoded (rules derive from
//!   `tasks.coordinator_hats`, hat `publishes`, and `topics.*`),
//! - humans get a permissive bypass + a stderr warning, agents get
//!   a structured hard-deny with recovery hints.
//!
//! The ACL is **layered** with the existing
//! `task_cli::validate_owner_hat_id` (which already rejects
//! non-coordinator owners on the create path). The policy here adds
//! an earlier role-deny + a richer message at the command entry, so
//! the same prefix shows up whether the agent invoked `add`, `ensure`,
//! or any future task-mutating verb.

use crate::operation_guard::OperationContext;
use ralph_core::config::RalphConfig;
use std::fmt;

/// `ralph tools task <subcommand>` verbs. The list is intentionally
/// flat — adding a new verb requires adding a variant here AND a
/// `TaskCommands` arm in `task_cli.rs` (clap-derived enum). The clap
/// surface is the source of truth at the type level, this enum
/// mirrors it for the ACL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskCommand {
    Add,
    Ensure,
    Start,
    Close,
    Fail,
    Reopen,
    List,
    Ready,
    Show,
    Verify,
}

impl TaskCommand {
    /// Parse a subcommand verb into the ACL enum.
    ///
    /// Returns `None` for unknown verbs so callers can decide whether
    /// to default-allow (existing legacy verbs we have not catalogued
    /// yet) or reject.
    pub fn parse(verb: &str) -> Option<Self> {
        Some(match verb {
            "add" => Self::Add,
            "ensure" => Self::Ensure,
            "start" => Self::Start,
            "close" => Self::Close,
            "fail" => Self::Fail,
            "reopen" => Self::Reopen,
            "list" => Self::List,
            "ready" => Self::Ready,
            "show" => Self::Show,
            "verify" => Self::Verify,
            _ => return None,
        })
    }

    /// All verbs known to the ACL. Used by `allowed_task_commands` /
    /// `denied_task_commands` listings on `HatIdentitySnapshot`.
    pub const ALL: &'static [TaskCommand] = &[
        TaskCommand::Add,
        TaskCommand::Ensure,
        TaskCommand::Start,
        TaskCommand::Close,
        TaskCommand::Fail,
        TaskCommand::Reopen,
        TaskCommand::List,
        TaskCommand::Ready,
        TaskCommand::Show,
        TaskCommand::Verify,
    ];

    /// Verbs that mutate cross-hat work items. Only coordinator hats
    /// may invoke them — non-coordinators hit a hard-deny.
    ///
    /// The list intentionally matches `HatIdentitySnapshot`'s
    /// `denied_task_commands` default (add / ensure). Lifecycle
    /// operations on tasks the caller already owns (start / close /
    /// fail / reopen) are allowed for both coordinator and worker
    /// hats via `authorize_lifecycle`, so they are NOT in this set.
    pub const COORDINATOR_ONLY: &'static [TaskCommand] = &[TaskCommand::Add, TaskCommand::Ensure];
}

impl fmt::Display for TaskCommand {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = match self {
            TaskCommand::Add => "add",
            TaskCommand::Ensure => "ensure",
            TaskCommand::Start => "start",
            TaskCommand::Close => "close",
            TaskCommand::Fail => "fail",
            TaskCommand::Reopen => "reopen",
            TaskCommand::List => "list",
            TaskCommand::Ready => "ready",
            TaskCommand::Show => "show",
            TaskCommand::Verify => "verify",
        };
        f.write_str(s)
    }
}

/// Outcome of an ACL check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyDecision {
    /// The command is allowed. The accompanying `human_warning` is
    /// `Some` only when a human-context caller triggered the policy
    /// (e.g. crossing loop boundaries) and the operator should be
    /// told, but the call still proceeds.
    Allow { human_warning: Option<String> },
    /// The command is hard-denied. `reason` is a stable
    /// machine-readable code; `hint` is the human-readable recovery
    /// suggestion surfaced in the `bail!` message.
    Deny { reason: &'static str, hint: String },
}

impl PolicyDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self, Self::Allow { .. })
    }

    pub fn is_deny(&self) -> bool {
        matches!(self, Self::Deny { .. })
    }
}

/// L2 CLI ACL entry point.
///
/// Derive policy from the resolved `RalphConfig` and the live
/// `OperationContext` rather than hardcoding hat names. Returns
/// `PolicyDecision::Allow` for humans (with an optional warning) and
/// `Deny` for agents that violate the role rules.
pub struct HatCommandPolicy;

impl HatCommandPolicy {
    /// Check whether the caller may invoke `task <subcmd>`.
    ///
    /// Rules (in order, first match wins):
    ///
    /// 1. **Unknown verb** → `Allow` (legacy / future verbs are
    ///    permissive; the underlying clap parser already rejects
    ///    malformed verbs at the CLI boundary).
    /// 2. **Human CLI** (`!ctx.is_agent_context`) → `Allow` + warning
    ///    when the call crosses loop boundaries. Human operators
    ///    must not be locked out — same bypass philosophy as
    ///    `task_cli::authorize_lifecycle`.
    /// 3. **Agent + no current hat** → `Deny` (`reason =
    ///    "missing_hat"`). An agent without a hat cannot be
    ///    classified against any role rule; fail-closed.
    /// 4. **Agent + coordinator-only verb + non-coordinator hat** →
    ///    `Deny` (`reason = "non_coordinator_owner"`). Mirrors
    ///    `validate_owner_hat_id` but at command-entry time so the
    ///    failure appears before any store mutation.
    /// 5. **Otherwise** → `Allow`.
    pub fn check_task(
        ctx: &OperationContext,
        config: &RalphConfig,
        verb: &str,
    ) -> PolicyDecision {
        let Some(cmd) = TaskCommand::parse(verb) else {
            return PolicyDecision::Allow {
                human_warning: None,
            };
        };

        // Rule 2: human CLI bypass with a warning when crossing loops.
        if !ctx.is_agent_context {
            // We cannot know the loop without parsing the task, but
            // the legacy `authorize_lifecycle` path already surfaces
            // that warning when the task exists. Keep the bypass
            // path narrow here.
            return PolicyDecision::Allow {
                human_warning: None,
            };
        }

        // Rule 3: agent context must carry a hat.
        let Some(caller_hat) = ctx.current_hat_id.as_deref() else {
            return PolicyDecision::Deny {
                reason: "missing_hat",
                hint: "agent context requires a current hat (set RALPH_CURRENT_HAT)".to_string(),
            };
        };

        // Rule 4: coordinator-only verbs hard-deny non-coordinators.
        if TaskCommand::COORDINATOR_ONLY.contains(&cmd) {
            let is_coordinator = config
                .tasks
                .coordinator_hats
                .iter()
                .any(|h| h == caller_hat);
            if !is_coordinator {
                let allow_hint = if config.tasks.coordinator_hats.is_empty() {
                    "tasks.coordinator_hats is empty in ralph.yml; add the hat id (e.g. `coordinator`) \
                     to tasks.coordinator_hats before dispatching work."
                } else {
                    "ask the coordinator hat to invoke `ralph tools task add/ensure` on your behalf."
                };
                return PolicyDecision::Deny {
                    reason: "non_coordinator_owner",
                    hint: format!(
                        "task {verb}: hat '{caller_hat}' is not in tasks.coordinator_hats {:?}; \
                         only coordinator hats may create tasks. {allow_hint}",
                        config.tasks.coordinator_hats
                    ),
                };
            }
        }

        PolicyDecision::Allow {
            human_warning: None,
        }
    }

    /// Check whether the caller may invoke `ralph wave emit` /
    /// `ralph wave verify`.
    ///
    /// U23: derive dispatcher status from `publishes ∩ *.unit.ready`
    /// topics. A hat that publishes wave-dispatch topics (`*.unit.ready`)
    /// is a dispatcher; otherwise it is a worker and wave fan-out is
    /// denied.
    ///
    /// The function is intentionally string-based (no preset names)
    /// so the rule generalises across all 7 builtin presets and any
    /// user-authored preset. A future plan can refine the heuristic
    /// with an explicit `is_wave_dispatcher: true` flag in the hat
    /// config; until then the topic-list check keeps the contract
    /// stable without inflating the public surface.
    pub fn check_wave_emit(ctx: &OperationContext, config: &RalphConfig) -> PolicyDecision {
        // Human CLI: always allowed (with the existing
        // `RALPH_WAVE_WORKER` cross-context check handled by the
        // wave command itself).
        if !ctx.is_agent_context {
            return PolicyDecision::Allow {
                human_warning: None,
            };
        }

        let Some(caller_hat) = ctx.current_hat_id.as_deref() else {
            return PolicyDecision::Deny {
                reason: "missing_hat",
                hint: "agent context requires a current hat (set RALPH_CURRENT_HAT)".to_string(),
            };
        };

        let is_dispatcher = config
            .hats
            .get(caller_hat)
            .map(|h| {
                h.publishes
                    .iter()
                    .any(|t| t.ends_with(".unit.ready") || t == "review.wave.ready")
            })
            .unwrap_or(false);

        if is_dispatcher {
            PolicyDecision::Allow {
                human_warning: None,
            }
        } else {
            PolicyDecision::Deny {
                reason: "non_dispatcher_hat",
                hint: format!(
                    "hat '{caller_hat}' does not declare a wave-dispatcher topic in `publishes` \
                     (expected something ending in `.unit.ready`); wave fan-out is reserved for \
                     dispatcher hats. Worker hats must use `ralph emit`."
                ),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn empty_ctx() -> OperationContext {
        OperationContext {
            workspace_root: PathBuf::from("/tmp"),
            current_loop_id: None,
            current_hat_id: None,
            is_agent_context: false,
        }
    }

    fn agent_ctx(hat: &str) -> OperationContext {
        OperationContext {
            workspace_root: PathBuf::from("/tmp"),
            current_loop_id: Some("loop-1".to_string()),
            current_hat_id: Some(hat.to_string()),
            is_agent_context: true,
        }
    }

    fn isolated_config_with_coordinator() -> RalphConfig {
        let yaml = r#"
event_loop:
  execution_mode: isolated
tasks:
  enabled: true
  coordinator_hats:
    - coordinator
hats:
  coordinator:
    name: "Coordinator"
    publishes: ["work.ready"]
  worker:
    name: "Worker"
    publishes: ["work.done"]
"#;
        serde_yaml::from_str(yaml).unwrap()
    }

    #[test]
    fn unknown_verb_is_allow() {
        let cfg = isolated_config_with_coordinator();
        let decision = HatCommandPolicy::check_task(&empty_ctx(), &cfg, "future-verb");
        assert!(decision.is_allow());
    }

    #[test]
    fn human_cli_task_add_is_allowed() {
        let cfg = isolated_config_with_coordinator();
        let decision = HatCommandPolicy::check_task(&empty_ctx(), &cfg, "add");
        assert!(decision.is_allow(), "human CLI must not be locked out: {decision:?}");
    }

    #[test]
    fn agent_without_hat_is_denied() {
        let cfg = isolated_config_with_coordinator();
        let ctx = OperationContext {
            workspace_root: PathBuf::from("/tmp"),
            current_loop_id: Some("loop-1".to_string()),
            current_hat_id: None,
            is_agent_context: true,
        };
        let decision = HatCommandPolicy::check_task(&ctx, &cfg, "add");
        let deny = match decision {
            PolicyDecision::Deny { reason, .. } => reason,
            other => panic!("expected deny, got {other:?}"),
        };
        assert_eq!(deny, "missing_hat");
    }

    #[test]
    fn coordinator_hat_can_add() {
        let cfg = isolated_config_with_coordinator();
        let decision = HatCommandPolicy::check_task(&agent_ctx("coordinator"), &cfg, "add");
        assert!(decision.is_allow(), "coordinator must be allowed: {decision:?}");
    }

    #[test]
    fn worker_hat_add_is_denied() {
        let cfg = isolated_config_with_coordinator();
        let decision = HatCommandPolicy::check_task(&agent_ctx("worker"), &cfg, "add");
        let deny = match decision {
            PolicyDecision::Deny { reason, hint } => (reason, hint),
            other => panic!("expected deny, got {other:?}"),
        };
        assert_eq!(deny.0, "non_coordinator_owner");
        assert!(deny.1.contains("worker"));
        assert!(deny.1.contains("coordinator"));
    }

    #[test]
    fn worker_hat_ensure_is_denied() {
        let cfg = isolated_config_with_coordinator();
        let decision = HatCommandPolicy::check_task(&agent_ctx("worker"), &cfg, "ensure");
        assert!(decision.is_deny());
    }

    #[test]
    fn worker_hat_close_is_allowed_for_owner() {
        let cfg = isolated_config_with_coordinator();
        let decision = HatCommandPolicy::check_task(&agent_ctx("worker"), &cfg, "close");
        assert!(
            decision.is_allow(),
            "lifecycle verbs pass the role gate; ownership is enforced by authorize_lifecycle: {decision:?}"
        );
    }

    #[test]
    fn empty_coordinator_hats_fail_closed() {
        let yaml = r#"
event_loop:
  execution_mode: isolated
tasks:
  enabled: true
  coordinator_hats: []
hats:
  ghost:
    name: "Ghost"
"#;
        let cfg: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let decision = HatCommandPolicy::check_task(&agent_ctx("ghost"), &cfg, "add");
        let deny = match decision {
            PolicyDecision::Deny { reason, hint } => (reason, hint),
            other => panic!("expected deny, got {other:?}"),
        };
        assert_eq!(deny.0, "non_coordinator_owner");
        assert!(
            deny.1.contains("tasks.coordinator_hats is empty"),
            "hint should point operator to the empty allowlist: {hint}",
            hint = deny.1
        );
    }

    #[test]
    fn wave_dispatcher_hat_is_allowed() {
        let yaml = r#"
event_loop:
  execution_mode: isolated
tasks:
  enabled: true
  coordinator_hats:
    - coordinator
hats:
  coordinator:
    name: "Coordinator"
    publishes: ["exec.unit.ready", "work.ready"]
"#;
        let cfg: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let decision = HatCommandPolicy::check_wave_emit(&agent_ctx("coordinator"), &cfg);
        assert!(decision.is_allow(), "dispatcher must be allowed: {decision:?}");
    }

    #[test]
    fn wave_worker_hat_is_denied() {
        let yaml = r#"
event_loop:
  execution_mode: isolated
tasks:
  enabled: true
  coordinator_hats:
    - coordinator
hats:
  worker:
    name: "Worker"
    publishes: ["work.done"]
"#;
        let cfg: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let decision = HatCommandPolicy::check_wave_emit(&agent_ctx("worker"), &cfg);
        let deny = match decision {
            PolicyDecision::Deny { reason, hint } => (reason, hint),
            other => panic!("expected deny, got {other:?}"),
        };
        assert_eq!(deny.0, "non_dispatcher_hat");
        assert!(deny.1.contains("worker"));
    }

    #[test]
    fn wave_human_cli_is_allowed() {
        let cfg = isolated_config_with_coordinator();
        let decision = HatCommandPolicy::check_wave_emit(&empty_ctx(), &cfg);
        assert!(decision.is_allow(), "human CLI must not be locked out");
    }

    #[test]
    fn wave_unknown_hat_in_isolated_mode_is_denied() {
        let cfg = isolated_config_with_coordinator();
        let decision = HatCommandPolicy::check_wave_emit(&agent_ctx("ghost"), &cfg);
        let deny = match decision {
            PolicyDecision::Deny { reason, .. } => reason,
            other => panic!("expected deny, got {other:?}"),
        };
        assert_eq!(deny, "non_dispatcher_hat");
    }

    #[test]
    fn command_round_trip_through_display_and_parse() {
        for cmd in TaskCommand::ALL {
            let s = cmd.to_string();
            let parsed = TaskCommand::parse(&s).expect("display output should parse");
            assert_eq!(*cmd, parsed);
        }
    }
}