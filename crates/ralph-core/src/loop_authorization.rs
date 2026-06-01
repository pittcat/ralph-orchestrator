//! P7: Loop operation authorization helpers.
//!
//! Distinguishes Agent-originated commands from Human CLI commands so that
//! an agent can only act on loops it owns, while a human operator can act
//! on any loop. The caller is identified by the [`OperationContext`] they
//! construct; this module is the single source of truth for the rules.

use crate::loop_registry::LoopEntry;

/// Identifies the caller of a loop operation. Agents construct this from
/// `OperationContext.current_hat_id`; humans construct a `Human` context
/// (typically from the CLI layer that knows it is interactive).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopCaller {
    /// A human operator (interactive CLI). Allowed read-only access to
    /// any loop; destructive access requires explicit confirmation, which
    /// is the responsibility of the caller to collect and pass.
    Human,
    /// An agent acting on behalf of `hat_id`. May only act on loops whose
    /// `owner_hat_id` matches `hat_id` (or that have no owner when
    /// `allow_unowned` is true — the default for read commands).
    Agent {
        hat_id: String,
        /// When `true`, the agent may also act on loops with `owner_hat_id
        /// == None`. The default is conservative: only owner-matching
        /// loops are accessible.
        allow_unowned: bool,
    },
}

impl LoopCaller {
    /// Construct an Agent caller from the current operation context. The
    /// `hat_id` is taken verbatim — Ralph never trusts `RALPH_CURRENT_HAT`
    /// for any other purpose, but the agent context is unambiguous here
    /// because the CLI is dispatching on its own behalf.
    pub fn agent(hat_id: impl Into<String>) -> Self {
        LoopCaller::Agent {
            hat_id: hat_id.into(),
            allow_unowned: false,
        }
    }
}

/// Outcome of an authorization check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthzDecision {
    /// The action is allowed.
    Allow,
    /// The action is denied, with a human-readable reason.
    Deny { reason: String },
}

impl AuthzDecision {
    pub fn is_allowed(&self) -> bool {
        matches!(self, AuthzDecision::Allow)
    }
}

/// P7: check whether `caller` may view the given loop entry. Read-only
/// commands (logs, history, diff) reach this gate. Agents may only view
/// loops they own; humans may view any loop.
pub fn can_view(caller: &LoopCaller, entry: &LoopEntry) -> AuthzDecision {
    match caller {
        LoopCaller::Human => AuthzDecision::Allow,
        LoopCaller::Agent {
            hat_id,
            allow_unowned,
        } => {
            if entry.owner_hat_id.as_deref() == Some(hat_id.as_str()) {
                AuthzDecision::Allow
            } else if entry.owner_hat_id.is_none() && *allow_unowned {
                AuthzDecision::Allow
            } else {
                AuthzDecision::Deny {
                    reason: format!(
                        "Agent hat '{}' may not view loop '{}' (owner: {:?})",
                        hat_id, entry.id, entry.owner_hat_id
                    ),
                }
            }
        }
    }
}

/// P7: check whether `caller` may stop the given loop. Agents may only
/// stop the loop they currently own.
pub fn can_stop(caller: &LoopCaller, entry: &LoopEntry) -> AuthzDecision {
    match caller {
        LoopCaller::Human => AuthzDecision::Allow,
        LoopCaller::Agent { hat_id, .. } => {
            if entry.owner_hat_id.as_deref() == Some(hat_id.as_str()) {
                AuthzDecision::Allow
            } else {
                AuthzDecision::Deny {
                    reason: format!(
                        "Agent hat '{}' may not stop loop '{}' (owner: {:?})",
                        hat_id, entry.id, entry.owner_hat_id
                    ),
                }
            }
        }
    }
}

/// P7: check whether `caller` may discard the given loop's worktree. This
/// is destructive: only the owner (or a human) may discard.
pub fn can_discard(caller: &LoopCaller, entry: &LoopEntry) -> AuthzDecision {
    can_stop(caller, entry)
}

/// P7: check whether `caller` may attach to the given loop's output.
/// Agents are always denied; humans are always allowed.
pub fn can_attach(caller: &LoopCaller, entry: &LoopEntry) -> AuthzDecision {
    match caller {
        LoopCaller::Human => AuthzDecision::Allow,
        LoopCaller::Agent { hat_id, .. } => AuthzDecision::Deny {
            reason: format!(
                "Agent hat '{}' may not attach to loop '{}'; attach is a human-only operation",
                hat_id, entry.id
            ),
        },
    }
}

/// P7: check whether `caller` may merge a queued loop. Agents may only
/// merge their own loop; humans may merge any.
pub fn can_merge(caller: &LoopCaller, entry: &LoopEntry) -> AuthzDecision {
    can_stop(caller, entry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;

    fn entry_with_owner(owner: Option<&str>) -> LoopEntry {
        LoopEntry {
            id: "loop-1".to_string(),
            pid: 1234,
            started: Utc::now(),
            prompt: "test".to_string(),
            worktree_path: None,
            workspace: "/tmp".to_string(),
            owner_hat_id: owner.map(|s| s.to_string()),
        }
    }

    #[test]
    fn test_loop_entry_stamps_owner_hat() {
        let entry = entry_with_owner(Some("executor"));
        assert_eq!(entry.owner_hat_id.as_deref(), Some("executor"));
    }

    #[test]
    fn test_loop_entry_legacy_owner_none_deserializes() {
        // A legacy entry without owner_hat_id deserializes as None.
        let json = r#"{
            "id": "loop-1",
            "pid": 1234,
            "started": "2026-01-01T00:00:00Z",
            "prompt": "test",
            "workspace": "/tmp"
        }"#;
        let entry: LoopEntry = serde_json::from_str(json).unwrap();
        assert!(entry.owner_hat_id.is_none());
    }

    #[test]
    fn test_loop_view_human_allows() {
        let entry = entry_with_owner(Some("executor"));
        assert!(can_view(&LoopCaller::Human, &entry).is_allowed());
    }

    #[test]
    fn test_loop_view_agent_owner_allows() {
        let entry = entry_with_owner(Some("executor"));
        assert!(can_view(&LoopCaller::agent("executor"), &entry).is_allowed());
    }

    #[test]
    fn test_loop_view_agent_other_denies() {
        let entry = entry_with_owner(Some("executor"));
        let decision = can_view(&LoopCaller::agent("reviewer"), &entry);
        assert!(!decision.is_allowed());
    }

    #[test]
    fn test_loop_stop_agent_other_denies() {
        let entry = entry_with_owner(Some("executor"));
        let decision = can_stop(&LoopCaller::agent("reviewer"), &entry);
        assert!(!decision.is_allowed());
    }

    #[test]
    fn test_loop_attach_agent_denied() {
        let entry = entry_with_owner(Some("executor"));
        let decision = can_attach(&LoopCaller::agent("executor"), &entry);
        assert!(!decision.is_allowed());
    }

    #[test]
    fn test_loop_attach_human_allowed() {
        let entry = entry_with_owner(Some("executor"));
        assert!(can_attach(&LoopCaller::Human, &entry).is_allowed());
    }

    #[test]
    fn test_loop_discard_owner_allows() {
        let entry = entry_with_owner(Some("executor"));
        assert!(can_discard(&LoopCaller::agent("executor"), &entry).is_allowed());
    }

    #[test]
    fn test_loop_merge_owner_allows() {
        let entry = entry_with_owner(Some("executor"));
        assert!(can_merge(&LoopCaller::agent("executor"), &entry).is_allowed());
    }
}
