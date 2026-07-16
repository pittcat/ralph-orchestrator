//! Workflow guard configuration for enforcing ordered event chains.

use serde::{Deserialize, Serialize};

/// Opt-in workflow state guards for enforcing ordered event chains.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct WorkflowGuardsConfig {
    /// List of workflow chains. An empty list or None means guards are disabled.
    #[serde(default)]
    pub chains: Vec<WorkflowChain>,
}


/// A named workflow chain that enforces ordered topic sequences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowChain {
    /// Human-readable name for this chain.
    pub name: String,

    /// Event topics in the required order.
    pub topics: Vec<String>,

    /// Enforcement mode.
    #[serde(default)]
    pub mode: WorkflowChainMode,

    /// Optional correlation key extraction.
    #[serde(default)]
    pub correlation: Option<CorrelationConfig>,
}

/// Enforcement mode for a workflow chain.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowChainMode {
    /// Strict ordered enforcement: each topic must follow the previous.
    /// Out-of-order events are rejected.
    #[default]
    Strict,

    /// Permissive: topics are recorded when seen but out-of-order events
    /// are not rejected. Useful for optional workflows or side-channels.
    Advisory,
}

/// Hat execution mode.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HatExecutionMode {
    /// Default coordinator behavior: Ralph acts as a central coordinator,
    /// all active hats' instructions are injected into a single prompt.
    #[default]
    Coordinator,

    /// Isolated execution: each hat runs in a separate backend process
    /// with only its own instructions and allowed events visible.
    Isolated,
}

/// Configuration for extracting a correlation key from an event payload.
///
/// When specified, the guard tracks workflow progress per unique instance key
/// rather than globally. This allows parallel workflow instances (e.g., multiple
/// experiments) to be guarded independently.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorrelationConfig {
    /// The JSON path within the payload to extract the instance key.
    /// Example: "experiment_id" extracts `payload.experiment_id`.
    ///
    /// Supports dot notation for nested fields (e.g., "data.experiment_id").
    pub from_payload: String,

    /// The event topic whose payload contains the correlation key.
    /// Typically the chain entry point (first topic in `topics`).
    #[serde(default)]
    pub from_topic: Option<String>,
}
