//! `apply_projection` — typed wrapper around the legacy
//! `state_projector::StateProjector::apply`. Plan ref: U3b.
//!
//! The full engine would replace the projector wholesale; for
//! now the engine reads `protocol.state_projection.actions_chain`
//! (preferred) or the legacy `actions` map and dispatches in
//! order. Failures short-circuit the rest of the chain so a
//! `mark_step_completed` step does not run when the preceding
//! `close_task` failed.

use serde_json::Value;

use super::protocol::ProtocolView;
use crate::config::StateProjectionAction;

/// Apply a single event's projection chain. Returns the same
/// `(applied, rejected)` accounting as the legacy projector so
/// callers can record a `ProjectionReport` unchanged.
///
/// `apply_fn` is the legacy dispatch closure (one action at a
/// time). The engine wraps the closure into an ordered chain,
/// short-circuiting on the first error.
pub fn apply_projection<F>(
    view: &ProtocolView,
    topic: &str,
    payload: &Value,
    mut apply_fn: F,
) -> ProjectionReport
where
    F: FnMut(&StateProjectionAction, &Value) -> Result<(), String>,
{
    let mut report = ProjectionReport::default();
    let Some(chain) = resolve_chain(view, topic) else {
        return report;
    };
    let mut chain_failed = false;
    for action in chain {
        if chain_failed {
            break;
        }
        match apply_fn(action, payload) {
            Ok(()) => report.applied += 1,
            Err(reason) => {
                report.rejected += 1;
                report.rejections.push(rejection(topic, reason));
                chain_failed = true;
            }
        }
    }
    report
}

/// Resolve the chain for `topic`. Engine precedence (U3b):
///   1. `actions_chain` (plan 2026-06-20-001 preferred form).
///   2. Legacy `actions` map (single action wrapped in a vec).
///   3. `None` — projector is inert for this topic.
pub fn resolve_chain<'a>(
    view: &'a ProtocolView,
    topic: &str,
) -> Option<Vec<&'a StateProjectionAction>> {
    let cfg = view.state_projection.as_ref()?;
    if !cfg.enabled {
        return None;
    }
    if let Some(chain) = cfg.actions_chain.get(topic) {
        if chain.is_empty() {
            return None;
        }
        return Some(chain.iter().collect());
    }
    if let Some(single) = cfg.actions.get(topic) {
        return Some(vec![single]);
    }
    None
}

/// Per-event projection report (mirrors the legacy
/// `state_projector::ProjectionReport` field set so callers can
/// wrap it).
#[derive(Debug, Default, Clone)]
pub struct ProjectionReport {
    pub applied: usize,
    pub rejected: usize,
    pub rejections: Vec<ProjectionRejection>,
}

#[derive(Debug, Clone)]
pub struct ProjectionRejection {
    pub topic: String,
    pub reason: String,
}

fn rejection(topic: &str, reason: String) -> ProjectionRejection {
    ProjectionRejection {
        topic: topic.to_string(),
        reason,
    }
}