//! WAC-U3 / WAC-U5: runtime Workflow Activation Contract types.
//!
//! Plan Unit: WAC-U3 (configuration) + WAC-U5 (priority-pass
//! index) of `2026-06-12-002-feat-workflow-activation-contract-plan`.

pub mod handoff_index;
pub mod handoff_tracker;

pub use handoff_index::{
    ConflictKind, HandoffConflict, HandoffEntry, HandoffIndex, HandoffIndexMap, HandoffSource,
};
pub use handoff_tracker::{HandoffEscalation, HandoffTracker, PendingHandoff};

