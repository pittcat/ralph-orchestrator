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

/// U1: distinguishable configuration faults that map to the four
/// "empty in ralph.yml" failure modes the policy can detect when an
/// agent triggers Rule 4 (coordinator-only verb, non-coordinator hat).
///
/// Each variant carries its own `hint()` text so the operator sees the
/// specific missing piece instead of the generic "tasks.coordinator_hats
/// is empty" message we had before. The ACL upstream surfaces these in
/// the `Deny { reason, hint }` payload; the hint is machine-stable but
/// human-readable so `ralph` agents can match on it for recovery.
///
/// The split here replaces a previous split-brain design where
/// `task_cli::load_coordinator_hats` parsed `ralph.yml` ad-hoc while
/// `HatCommandPolicy::check_task` read the same field through
/// `RalphConfig.tasks.coordinator_hats`. With both paths now reading
/// the same config object, the only thing left to disambiguate is the
/// shape of the failure — which is what `ConfigFault` captures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigFault {
    /// `ralph.yml` (or `ralph.yaml`) does not exist in the workspace root.
    MissingRalphYml,
    /// `ralph.yml` exists but does not declare a `tasks:` section.
    MissingTasksSection,
    /// `ralph.yml` declares `tasks:` but no `coordinator_hats` key inside it.
    MissingCoordinatorHatsKey,
    /// `tasks.coordinator_hats` is present but empty (`[]` or missing entries).
    CoordinatorHatsEmpty,
}

impl ConfigFault {
    /// Human-readable, operator-actionable recovery hint.
    ///
    /// Each hint is intentionally different enough that an operator (or
    /// an LLM agent) can match the failure shape from the message alone
    /// and apply the right fix without re-reading `ralph.yml`.
    pub fn hint(&self) -> String {
        match self {
            // 2026-07-13-001 plan U5 + review #C1: advertise every
            // supported discovery path (`-c` / `$RALPH_CONFIG` /
            // `ralph.yml` / `ralph.yaml`) instead of telling the
            // operator to symlink their custom file to `ralph.yml`.
            Self::MissingRalphYml => "no project config found (looked for -c file, $RALPH_CONFIG, ralph.yml, ralph.yaml); pass `ralph -c <file> …`, export RALPH_CONFIG, or add ralph.yml with tasks.coordinator_hats".into(),
            Self::MissingTasksSection => "ralph.yml has no `tasks:` section; add one with `coordinator_hats: [coordinator]`".into(),
            Self::MissingCoordinatorHatsKey => "ralph.yml `tasks:` block exists but does not declare `coordinator_hats`; add `coordinator_hats: [coordinator]`".into(),
            Self::CoordinatorHatsEmpty => "tasks.coordinator_hats is empty in ralph.yml; add the hat id (e.g. `coordinator`) to tasks.coordinator_hats before dispatching work".into(),
        }
    }
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
        coordinator_hats: &[String],
        coordinator_err: Option<&crate::task_cli::CoordinatorHatsError>,
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
            let is_coordinator = coordinator_hats.iter().any(|h| h == caller_hat);
            if !is_coordinator {
                // Surface the typed `CoordinatorHatsError` (if any) so
                // the operator sees the *shape* of the failure
                // (missing ralph.yml / missing tasks: / missing key /
                // empty value) instead of a generic "is empty" line.
                let allow_hint = match coordinator_err {
                    Some(err) => err.to_string(),
                    None => {
                        if coordinator_hats.is_empty() {
                            "tasks.coordinator_hats is empty in ralph.yml; add the hat id \
                             (e.g. `coordinator`) to tasks.coordinator_hats before dispatching \
                             work."
                                .to_string()
                        } else {
                            "ask the coordinator hat to invoke `ralph tools task add/ensure` on \
                             your behalf."
                                .to_string()
                        }
                    }
                };
                return PolicyDecision::Deny {
                    reason: "non_coordinator_owner",
                    hint: format!(
                        "task {verb}: hat '{caller_hat}' is not in tasks.coordinator_hats {:?}; \
                         only coordinator hats may create tasks. {allow_hint}",
                        coordinator_hats
                    ),
                };
            }
        }

        PolicyDecision::Allow {
            human_warning: None,
        }
    }

    /// When `event_loop.state_projection.enabled` is true, the projector
    /// (driven by `work.ready`) is the sole writer for plan-unit tasks.
    /// Agent-context `task add` / plain `task ensure` race the projector
    /// and produce duplicate `task_id` rows (2026-07-07 e2e stall).
    ///
    /// Fix-unit minting remains allowed via `task ensure --for-fix-unit`.
    pub fn check_projector_task_create(
        ctx: &OperationContext,
        config: &RalphConfig,
        verb: &str,
        is_for_fix_unit: bool,
    ) -> PolicyDecision {
        if !config.event_loop.state_projection.enabled || !ctx.is_agent_context {
            return PolicyDecision::Allow {
                human_warning: None,
            };
        }

        let Some(cmd) = TaskCommand::parse(verb) else {
            return PolicyDecision::Allow {
                human_warning: None,
            };
        };

        if !TaskCommand::COORDINATOR_ONLY.contains(&cmd) {
            return PolicyDecision::Allow {
                human_warning: None,
            };
        }

        if cmd == TaskCommand::Ensure && is_for_fix_unit {
            return PolicyDecision::Allow {
                human_warning: None,
            };
        }

        let hint = if cmd == TaskCommand::Add {
            "This loop creates runtime tasks via a handoff event (see your hat Trigger State Table), \
             not via `ralph tools task add`. Emit the event with `task_key` + `step`, then read \
             `task_id` from the trigger payload or `## ORCHESTRATOR CONTEXT` on the next activation."
                .to_string()
        } else {
            "Plain `task ensure --key` is not allowed in this loop. If your hat instructions \
             describe a fix-unit mint path, use only that documented command shape; otherwise \
             create tasks via the handoff event in your Trigger State Table."
                .to_string()
        };

        PolicyDecision::Deny {
            reason: "projector_ssot_task_create_forbidden",
            hint,
        }
    }

    /// Combined L2 check: role gate + projector SSOT gate.
    pub fn check_task_with_config(
        ctx: &OperationContext,
        coordinator_hats: &[String],
        coordinator_err: Option<&crate::task_cli::CoordinatorHatsError>,
        config: Option<&RalphConfig>,
        verb: &str,
        is_for_fix_unit: bool,
    ) -> PolicyDecision {
        match Self::check_task(ctx, coordinator_hats, coordinator_err, verb) {
            deny @ PolicyDecision::Deny { .. } => deny,
            PolicyDecision::Allow { .. } => {
                let Some(cfg) = config else {
                    return PolicyDecision::Allow {
                        human_warning: None,
                    };
                };
                Self::check_projector_task_create(ctx, cfg, verb, is_for_fix_unit)
            }
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

    fn isolated_coordinator_slice() -> &'static [String] {
        // Mirrors the previous `isolated_config_with_coordinator`
        // builder: a single coordinator hat named "coordinator".
        // Returned as a static slice so check_task's `&[String]`
        // argument can borrow from it without lifetime gymnastics.
        // Construct the slice on the stack to avoid `static` mutability.
        // This is a 1-element helper so the allocation cost is
        // negligible across the 7 existing tests.
        // We deliberately leak the box to obtain `'static` lifetime
        // for the tests below.
        Box::leak(Box::new(vec!["coordinator".to_string()]))
    }

    #[test]
    fn unknown_verb_is_allow() {
        let hats = isolated_coordinator_slice();
        let decision = HatCommandPolicy::check_task(&empty_ctx(), hats, None, "future-verb");
        assert!(decision.is_allow());
    }

    #[test]
    fn human_cli_task_add_is_allowed() {
        let hats = isolated_coordinator_slice();
        let decision = HatCommandPolicy::check_task(&empty_ctx(), hats, None, "add");
        assert!(
            decision.is_allow(),
            "human CLI must not be locked out: {decision:?}"
        );
    }

    #[test]
    fn agent_without_hat_is_denied() {
        let hats = isolated_coordinator_slice();
        let ctx = OperationContext {
            workspace_root: PathBuf::from("/tmp"),
            current_loop_id: Some("loop-1".to_string()),
            current_hat_id: None,
            is_agent_context: true,
        };
        let decision = HatCommandPolicy::check_task(&ctx, hats, None, "add");
        let deny = match decision {
            PolicyDecision::Deny { reason, .. } => reason,
            other => panic!("expected deny, got {other:?}"),
        };
        assert_eq!(deny, "missing_hat");
    }

    #[test]
    fn coordinator_hat_can_add() {
        let hats = isolated_coordinator_slice();
        let decision = HatCommandPolicy::check_task(&agent_ctx("coordinator"), hats, None, "add");
        assert!(
            decision.is_allow(),
            "coordinator must be allowed: {decision:?}"
        );
    }

    #[test]
    fn worker_hat_add_is_denied() {
        let hats = isolated_coordinator_slice();
        let decision = HatCommandPolicy::check_task(&agent_ctx("worker"), hats, None, "add");
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
        let hats = isolated_coordinator_slice();
        let decision = HatCommandPolicy::check_task(&agent_ctx("worker"), hats, None, "ensure");
        assert!(decision.is_deny());
    }

    #[test]
    fn worker_hat_close_is_allowed_for_owner() {
        let hats = isolated_coordinator_slice();
        let decision = HatCommandPolicy::check_task(&agent_ctx("worker"), hats, None, "close");
        assert!(
            decision.is_allow(),
            "lifecycle verbs pass the role gate; ownership is enforced by authorize_lifecycle: {decision:?}"
        );
    }

    #[test]
    fn empty_coordinator_hats_fail_closed() {
        let decision = HatCommandPolicy::check_task(&agent_ctx("ghost"), &[], None, "add");
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
        assert!(
            decision.is_allow(),
            "dispatcher must be allowed: {decision:?}"
        );
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

    // ---- U1 deny hint 区分 4 类配置失败 ----

    #[test]
    fn deny_hint_distinguishes_missing_ralph_yml() {
        // 缺 ralph.yml → ConfigFault::MissingRalphYml → hint 同时
        // 提示 `-c` / `$RALPH_CONFIG` / `ralph.yml` / `ralph.yaml`
        // 四条恢复路径（plan 2026-07-13-001 plan U5 + R6）。
        let fault = ConfigFault::MissingRalphYml;
        let hint = fault.hint();
        assert!(
            hint.contains("ralph.yml") && hint.contains("$RALPH_CONFIG") && hint.contains("-c"),
            "MissingRalphYml hint should advertise every recovery path: {hint}"
        );
        assert!(
            hint.contains("no project config found"),
            "MissingRalphYml hint should signal the missing-config state: {hint}"
        );
    }

    #[test]
    fn deny_hint_distinguishes_missing_tasks_section() {
        let fault = ConfigFault::MissingTasksSection;
        let hint = fault.hint();
        assert!(
            hint.contains("tasks:") && hint.contains("coordinator_hats"),
            "MissingTasksSection hint should mention `tasks:` and `coordinator_hats`: {hint}"
        );
        // 与 MissingRalphYml 区分开(不出现 "no ralph.yml" 字面)
        assert!(
            !hint.contains("no ralph.yml"),
            "MissingTasksSection should NOT say 'no ralph.yml': {hint}"
        );
    }

    #[test]
    fn deny_hint_distinguishes_missing_coordinator_hats_key() {
        let fault = ConfigFault::MissingCoordinatorHatsKey;
        let hint = fault.hint();
        assert!(
            hint.contains("coordinator_hats"),
            "MissingCoordinatorHatsKey hint should mention coordinator_hats: {hint}"
        );
        assert!(
            hint.contains("tasks:"),
            "MissingCoordinatorHatsKey hint should reference tasks: block: {hint}"
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // U7 (2026-07-04-003 plan): `check_task` with external
    // `coordinator_hats` slice + `CoordinatorHatsError` hint.
    //
    // These tests intentionally do NOT read ralph.yml from disk.
    // The slice + typed error are passed in directly so the policy
    // surface is testable without an actual workspace.
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn agent_worker_not_in_slice_denies_and_lists_slice() {
        let hats: &[String] = &["coordinator".to_string()];
        let decision = HatCommandPolicy::check_task(&agent_ctx("executor"), hats, None, "add");
        let deny = match decision {
            PolicyDecision::Deny { reason, hint } => (reason, hint),
            other => panic!("expected Deny, got {other:?}"),
        };
        assert_eq!(deny.0, "non_coordinator_owner");
        assert!(
            deny.1.contains("coordinator"),
            "hint should list the allowed slice: {hint}",
            hint = deny.1
        );
        assert!(
            !deny.1.contains("empty in ralph.yml"),
            "hint should NOT contain the misleading 'empty in ralph.yml' line: {hint}",
            hint = deny.1
        );
    }

    #[test]
    fn agent_coordinator_in_slice_allowed() {
        let hats: &[String] = &["coordinator".to_string()];
        let decision = HatCommandPolicy::check_task(&agent_ctx("coordinator"), hats, None, "add");
        assert!(
            decision.is_allow(),
            "coordinator must be allowed: {decision:?}"
        );
    }

    #[test]
    fn empty_slice_denies_with_typed_error_hint() {
        let err = crate::task_cli::CoordinatorHatsError::MissingRalphYml;
        let decision = HatCommandPolicy::check_task(&agent_ctx("executor"), &[], Some(&err), "add");
        let deny = match decision {
            PolicyDecision::Deny { reason, hint } => (reason, hint),
            other => panic!("expected Deny, got {other:?}"),
        };
        assert_eq!(deny.0, "non_coordinator_owner");
        // The typed error should be surfaced verbatim instead of
        // the generic "empty in ralph.yml" line.
        // 2026-07-13-001 plan U3 / U5: the hint now advertises
        // every supported discovery path (`-c` / `$RALPH_CONFIG`
        // / `ralph.yml` / `ralph.yaml`) so operators no longer
        // think the only fix is symlinking their custom file.
        assert!(
            deny.1.contains("no project config found")
                && deny.1.contains("$RALPH_CONFIG")
                && deny.1.contains("-c"),
            "hint should surface the typed CoordinatorHatsError and reference every recovery path, got: {hint}",
            hint = deny.1
        );
    }

    #[test]
    fn human_cli_always_allowed_even_with_typed_error() {
        let err = crate::task_cli::CoordinatorHatsError::MissingRalphYml;
        let decision = HatCommandPolicy::check_task(&empty_ctx(), &[], Some(&err), "add");
        assert!(
            decision.is_allow(),
            "human CLI must always be allowed: {decision:?}"
        );
    }

    fn config_with_projection_enabled() -> RalphConfig {
        let mut cfg = isolated_config_with_coordinator();
        cfg.event_loop.state_projection.enabled = true;
        cfg
    }

    #[test]
    fn projector_ssot_denies_coordinator_add_in_agent_context() {
        let cfg = config_with_projection_enabled();
        let decision = HatCommandPolicy::check_projector_task_create(
            &agent_ctx("coordinator"),
            &cfg,
            "add",
            false,
        );
        let deny = match decision {
            PolicyDecision::Deny { reason, hint } => (reason, hint),
            other => panic!("expected Deny, got {other:?}"),
        };
        assert_eq!(deny.0, "projector_ssot_task_create_forbidden");
        assert!(deny.1.contains("handoff event") || deny.1.contains("Trigger State Table"));
    }

    #[test]
    fn projector_ssot_allows_for_fix_unit_ensure() {
        let cfg = config_with_projection_enabled();
        let decision = HatCommandPolicy::check_projector_task_create(
            &agent_ctx("coordinator"),
            &cfg,
            "ensure",
            true,
        );
        assert!(
            decision.is_allow(),
            "for-fix-unit ensure must pass: {decision:?}"
        );
    }

    #[test]
    fn projector_ssot_denies_plain_ensure_in_agent_context() {
        let cfg = config_with_projection_enabled();
        let decision = HatCommandPolicy::check_projector_task_create(
            &agent_ctx("coordinator"),
            &cfg,
            "ensure",
            false,
        );
        assert!(
            matches!(decision, PolicyDecision::Deny { .. }),
            "plain ensure must be denied: {decision:?}"
        );
    }
}
