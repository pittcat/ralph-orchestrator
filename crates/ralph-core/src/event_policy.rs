//! Event policy validation for typed payload schema enforcement.
//!
//! Provides pure-function validation that can be used by the event loop,
//! CLI emit commands, and API layers.

#![allow(unused_imports)]

use crate::config::RalphConfig;
use crate::event_reader::EventReader;
use crate::hat_registry::HatRegistry;
use ralph_proto::HatId;
use ralph_proto::Topic;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

// Re-export config types for convenience
pub use crate::config::{
    CompletionAfterTerminalAction, EventPolicyConfig, EventPolicyMode, PayloadType, ViolationAction,
};

pub mod projection;
pub mod runtime;
#[cfg(test)]
pub mod tests;
pub mod types;
pub mod validation;

// Re-export public items from types
pub use types::{
    DuplicateWorkDoneHint, PolicyDecision, PolicyFinding, PolicyRejection, ReasonClass,
    ViolationType, is_recoverable_policy_finding,
};

// Re-export public items from runtime
pub use runtime::{PolicyRuntimeState, precheck_proposed_dedup_key};

// Re-export public items from validation
pub use validation::{
    DefaultHandoffConfig, EventLoopHandoffConfig, HandoffEnvelopeConfigAccess,
    NULL_PAYLOAD_REJECT_TOPICS, build_allowed_topics, check_completion_guard,
    check_completion_honored, check_handoff_envelope, check_topic_deny_rules, check_topic_format,
    handoff_envelope_validation_enabled, is_null_payload_rejected_topic, is_system_control_topic,
    is_system_topic, matches_topic_rule, validate_event, validate_event_with_hat,
    validate_event_with_options,
};

// Re-export public items from projection
pub use projection::{
    CandidateEmitPreview, CandidateHatEntry, NextHatCandidates, PolicyReasonEntry,
    ProjectionAction, ProjectionPreview, evaluate_candidate_emit,
};
