//! U10 (plan 2026-07-30-004): Parallel Forge plan handoff verification.
//!
//! The planner hat submits an `execution-plan.yml` reference + identity +
//! digest via `forge.plan.ready`. The runtime verifies the artifact
//! against the canonical digest (U9) before projecting the task DAG.
//!
//! # Handoff protocol
//!
//! 1. The planner submits only the artifact path and plan identity.
//! 2. The CLI policy-check/apply path canonicalizes the bounded artifact and
//!    stamps the `plan_digest`; direct runtime ingress may omit the digest.
//! 3. The runtime reads the workspace-bounded artifact once, derives the task
//!    DAG, and compares any supplied digest. A mismatch rejects the handoff
//!    before task projection.

use crate::artifact_canonicalizer::{ArtifactError, CanonicalArtifact, canonicalize};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::path::Path;

// ── U2 (plan 2026-09-03-0959) canonical artifact v2 ────────────────────
//
// Typed resource capacity/claim pairs + a canonical per-Unit `target_branch`
// are added to the execution-plan schema. The canonical digest covers all of
// them so identical logical plans always hash to the same value regardless
// of input list order. See `compute_resource_aware_digest` for the
// normalization rules and `validate_plan_v2` for the validation rules.

/// Typed resource capacity declared at plan scope.
///
/// Each `ResourceClaim` made by a Unit must reference a `key` declared here
/// (validated post-parse). The runtime admission engine (Unit 4) enforces
/// that no Unit's claim exceeds the capacity declared here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceCapacity {
    pub key: String,
    pub capacity: u32,
}

/// Typed resource claim attached to a Unit.
///
/// `permits == 0` is rejected at parse time (D6: typed capacity+permits;
/// zero is not meaningful). A claim whose `permits > capacity` is accepted
/// at parse time and re-checked by the runtime admission engine in Unit 4.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceClaim {
    pub key: String,
    pub permits: u32,
}

/// Error from v2 validation (resources / target_branch / digest).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanV2Error {
    /// A `ResourceClaim` references a capacity key that the plan does not
    /// declare.
    UnknownResource {
        unit_id: String,
        resource_key: String,
    },
    /// Two `ResourceCapacity` entries share the same `key`.
    DuplicateCapacityKey { key: String },
    /// Two `ResourceClaim` entries within one Unit share the same `key`.
    DuplicateClaimKey {
        unit_id: String,
        resource_key: String,
    },
    /// A `ResourceClaim` has `permits == 0`.
    ZeroPermits {
        unit_id: String,
        resource_key: String,
    },
    /// A `ResourceCapacity` has `capacity == 0`.
    ZeroCapacity { resource_key: String },
    /// A Unit's `target_branch` is empty.
    EmptyTargetBranch { unit_id: String },
    /// A Unit's `target_branch` violates the [`git check-ref-format
    /// --branch`](https://git-scm.com/docs/git-check-ref-format) rules.
    UnsafeTargetBranch {
        unit_id: String,
        branch: String,
        reason: &'static str,
    },
}

impl std::fmt::Display for PlanV2Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PlanV2Error::UnknownResource {
                unit_id,
                resource_key,
            } => write!(
                f,
                "unit '{unit_id}' claims resource '{resource_key}' which is not declared in plan.resource_capacities"
            ),
            PlanV2Error::DuplicateCapacityKey { key } => {
                write!(f, "plan.resource_capacities contains duplicate key '{key}'")
            }
            PlanV2Error::DuplicateClaimKey {
                unit_id,
                resource_key,
            } => write!(
                f,
                "unit '{unit_id}' contains duplicate resource_claims entry for key '{resource_key}'"
            ),
            PlanV2Error::ZeroPermits {
                unit_id,
                resource_key,
            } => write!(
                f,
                "unit '{unit_id}' declares resource_claims[{resource_key}] with permits=0; zero permits is rejected"
            ),
            PlanV2Error::ZeroCapacity { resource_key } => write!(
                f,
                "plan.resource_capacities[{resource_key}] has capacity=0; zero capacity is rejected"
            ),
            PlanV2Error::EmptyTargetBranch { unit_id } => {
                write!(f, "unit '{unit_id}' has empty target_branch")
            }
            PlanV2Error::UnsafeTargetBranch {
                unit_id,
                branch,
                reason,
            } => write!(
                f,
                "unit '{unit_id}' has unsafe target_branch '{branch}': {reason}"
            ),
        }
    }
}

impl std::error::Error for PlanV2Error {}

/// Validate a `git check-ref-format --branch`-style branch name. Mirrors the
/// subset of rules that catch the unsafe patterns we care about (no spaces,
/// no `..`/`~`/`^`/`:`/`?`/`*`/`[`/`/`, no leading `-`, no trailing `/`
/// or `.lock`, no `@{`, no `//`, no trailing `.`, max 255 bytes). Anything
/// else passes. The `(reason)` string explains which rule was violated.
pub fn validate_target_branch(branch: &str) -> Result<(), &'static str> {
    if branch.is_empty() {
        return Err("target_branch must not be empty");
    }
    if branch.len() > 255 {
        return Err("target_branch must be ≤ 255 bytes");
    }
    if branch.starts_with('-') {
        return Err("target_branch must not start with '-'");
    }
    if branch.starts_with('/') {
        return Err("target_branch must not start with '/'");
    }
    if branch.ends_with('/') {
        return Err("target_branch must not end with '/'");
    }
    if branch.ends_with(".lock") {
        return Err("target_branch must not end with '.lock'");
    }
    if branch.ends_with('.') {
        return Err("target_branch must not end with '.'");
    }
    if branch.contains("//") {
        return Err("target_branch must not contain '//'");
    }
    if branch.contains("..") {
        return Err("target_branch must not contain '..'");
    }
    if branch.contains("@{") {
        return Err("target_branch must not contain '@{'");
    }
    if branch.contains(' ') {
        return Err("target_branch must not contain spaces");
    }
    if branch.contains('~') {
        return Err("target_branch must not contain '~'");
    }
    if branch.contains('^') {
        return Err("target_branch must not contain '^'");
    }
    if branch.contains(':') {
        return Err("target_branch must not contain ':'");
    }
    if branch.contains('?') {
        return Err("target_branch must not contain '?'");
    }
    if branch.contains('*') {
        return Err("target_branch must not contain '*'");
    }
    if branch.contains('[') {
        return Err("target_branch must not contain '['");
    }
    if branch.contains('\\') {
        return Err("target_branch must not contain '\\'");
    }
    if branch.chars().any(|c| c.is_ascii_control()) {
        return Err("target_branch must not contain ASCII control characters");
    }
    Ok(())
}

/// Compute the resource-aware canonical digest of a parsed v2 plan.
///
/// Normalization rules (per U2 spec):
/// - `resource_capacities` are sorted by `key`.
/// - `resource_claims` within each unit are sorted by `key`.
/// - Units themselves are NOT sorted (their declared order is meaningful
///   and is preserved by the YAML canonical form).
/// - `target_branch` and `depends_on` participate in the digest verbatim.
///
/// The digest uses SHA-256 over a stable `|`-delimited textual form. Two
/// plans that differ only in the order of `resource_capacities` entries
/// (or the order of `resource_claims` within a unit) produce the same
/// digest.
pub fn compute_resource_aware_digest(
    plan_key: &str,
    capacities: &[ResourceCapacity],
    units: &[ExecutionPlanUnitView<'_>],
) -> String {
    let mut sorted_caps: Vec<&ResourceCapacity> = capacities.iter().collect();
    sorted_caps.sort_by(|a, b| a.key.cmp(&b.key));

    let mut s = String::new();
    s.push_str("plan_key=");
    s.push_str(plan_key);
    s.push('|');

    for cap in &sorted_caps {
        s.push_str("cap[");
        s.push_str(&cap.key);
        s.push_str("]=");
        s.push_str(&cap.capacity.to_string());
        s.push('|');
    }

    for unit in units {
        s.push_str("unit[");
        s.push_str(unit.id);
        s.push_str("]|target_branch=");
        s.push_str(unit.target_branch);
        s.push('|');
        s.push_str("depends_on=");
        let mut deps: Vec<String> = unit.depends_on.to_vec();
        deps.sort();
        s.push_str(&deps.join(","));
        s.push('|');

        let mut sorted_claims: Vec<&ResourceClaim> = unit.resource_claims.iter().collect();
        sorted_claims.sort_by(|a, b| a.key.cmp(&b.key));
        for claim in sorted_claims {
            s.push_str("claim[");
            s.push_str(unit.id);
            s.push(':');
            s.push_str(&claim.key);
            s.push_str("]=");
            s.push_str(&claim.permits.to_string());
            s.push('|');
        }
    }

    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Borrowed view over the fields of an [`ExecutionPlanUnit`] that participate
/// in the resource-aware canonical digest. Keeping the digest function in
/// terms of a borrowed view lets callers pass parsed units without cloning.
#[derive(Debug, Clone)]
pub struct ExecutionPlanUnitView<'a> {
    pub id: &'a str,
    pub target_branch: &'a str,
    pub depends_on: &'a [String],
    pub resource_claims: &'a [ResourceClaim],
}

/// Validate and normalize a parsed v2 plan in place.
///
/// Normalization:
/// - Sorts `plan.resource_capacities` by `key` (stable digest order).
/// - Sorts each `unit.resource_claims` by `key` (stable digest order).
///
/// Validation (fail-closed, all errors returned, not just the first):
/// - No duplicate `resource_capacities[].key`.
/// - No duplicate `resource_claims[].key` within the same unit.
/// - Every `resource_claims[].key` references a declared capacity.
/// - No `permits == 0`.
/// - No `capacity == 0`.
/// - Every unit has a non-empty, `git check-ref-format --branch`-safe
///   `target_branch`.
// `ExecutionPlanArtifact` is intentionally crate-private; this validator is
// exposed so callers can re-validate an already-parsed plan (e.g. tests).
#[allow(private_interfaces)]
pub fn validate_and_normalize_plan(plan: &mut ExecutionPlanArtifact) -> Result<(), PlanV2Error> {
    let mut seen_capacity_keys: HashSet<String> = HashSet::new();
    for cap in &plan.resource_capacities {
        if cap.capacity == 0 {
            return Err(PlanV2Error::ZeroCapacity {
                resource_key: cap.key.clone(),
            });
        }
        if !seen_capacity_keys.insert(cap.key.clone()) {
            return Err(PlanV2Error::DuplicateCapacityKey {
                key: cap.key.clone(),
            });
        }
    }
    plan.resource_capacities.sort_by(|a, b| a.key.cmp(&b.key));

    let declared_keys: HashSet<&str> = plan
        .resource_capacities
        .iter()
        .map(|cap| cap.key.as_str())
        .collect();

    for unit in &mut plan.units {
        if unit.target_branch.is_empty() {
            return Err(PlanV2Error::EmptyTargetBranch {
                unit_id: unit.id.clone(),
            });
        }
        validate_target_branch(&unit.target_branch).map_err(|reason| {
            PlanV2Error::UnsafeTargetBranch {
                unit_id: unit.id.clone(),
                branch: unit.target_branch.clone(),
                reason,
            }
        })?;

        let mut seen_claim_keys: HashSet<String> = HashSet::new();
        for claim in &unit.resource_claims {
            if claim.permits == 0 {
                return Err(PlanV2Error::ZeroPermits {
                    unit_id: unit.id.clone(),
                    resource_key: claim.key.clone(),
                });
            }
            if !declared_keys.contains(claim.key.as_str()) {
                return Err(PlanV2Error::UnknownResource {
                    unit_id: unit.id.clone(),
                    resource_key: claim.key.clone(),
                });
            }
            if !seen_claim_keys.insert(claim.key.clone()) {
                return Err(PlanV2Error::DuplicateClaimKey {
                    unit_id: unit.id.clone(),
                    resource_key: claim.key.clone(),
                });
            }
        }
        unit.resource_claims.sort_by(|a, b| a.key.cmp(&b.key));
    }
    Ok(())
}

/// Error from plan handoff verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandoffError {
    /// The artifact path escapes the workspace root or contains a parent
    /// traversal component.
    PathEscape { path: String },
    /// The artifact path is not a regular file.
    NotRegularFile { path: String },
    /// The artifact changed while it was being read.
    ChangedDuringRead { path: String },
    /// The artifact could not be read.
    Io { path: String, source: String },
    /// The digest in the event payload did not match the canonical digest.
    DigestMismatch {
        /// The digest the event payload claimed.
        expected: String,
        /// The digest computed from the artifact bytes.
        actual: String,
    },
    /// The raw artifact exceeded the 1 MiB size bound.
    ArtifactTooLarge {
        /// Observed size in bytes.
        size: usize,
        /// The configured limit (1 MiB).
        limit: usize,
    },
    /// The artifact declared too many Units.
    TooManyUnits {
        /// Observed Unit count.
        count: usize,
        /// The configured limit.
        limit: usize,
    },
    /// The artifact declared too many dependency edges.
    TooManyEdges {
        /// Observed edge count.
        count: usize,
        /// The configured limit.
        limit: usize,
    },
    /// The Parallel Forge artifact contains no actual fan-out wave.
    NoParallelWave {
        /// Largest number of Units assigned to one execution wave.
        max_wave_size: usize,
    },
    /// The artifact was not valid YAML.
    ParseError {
        /// The underlying parser message.
        source: String,
    },
}

impl std::fmt::Display for HandoffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandoffError::PathEscape { path } => {
                write!(f, "artifact path escapes workspace: {path}")
            }
            HandoffError::NotRegularFile { path } => {
                write!(f, "artifact is not a regular file: {path}")
            }
            HandoffError::ChangedDuringRead { path } => {
                write!(f, "artifact changed during read: {path}")
            }
            HandoffError::Io { path, source } => {
                write!(f, "failed to read artifact {path}: {source}")
            }
            HandoffError::DigestMismatch { expected, actual } => write!(
                f,
                "plan digest mismatch: event claimed {expected}, artifact canonicalized to {actual}"
            ),
            HandoffError::ArtifactTooLarge { size, limit } => write!(
                f,
                "artifact is {size} bytes, exceeding the {limit}-byte (1 MiB) limit"
            ),
            HandoffError::TooManyUnits { count, limit } => {
                write!(
                    f,
                    "artifact declares {count} units, exceeding the limit of {limit}"
                )
            }
            HandoffError::TooManyEdges { count, limit } => write!(
                f,
                "artifact declares {count} dependency edges, exceeding the limit of {limit}"
            ),
            HandoffError::NoParallelWave { max_wave_size } => write!(
                f,
                "parallel-forge execution plan must contain a wave with at least 2 Units; largest wave has {max_wave_size}"
            ),
            HandoffError::ParseError { source } => {
                write!(f, "failed to parse artifact YAML: {source}")
            }
        }
    }
}

impl std::error::Error for HandoffError {}

impl From<ArtifactError> for HandoffError {
    fn from(e: ArtifactError) -> Self {
        match e {
            ArtifactError::TooLarge { size, limit } => {
                HandoffError::ArtifactTooLarge { size, limit }
            }
            ArtifactError::TooManyUnits { count, limit } => {
                HandoffError::TooManyUnits { count, limit }
            }
            ArtifactError::TooManyEdges { count, limit } => {
                HandoffError::TooManyEdges { count, limit }
            }
            ArtifactError::ParseError { source } => HandoffError::ParseError { source },
            ArtifactError::AliasesForbidden => HandoffError::ParseError {
                source: "artifact uses YAML anchors/aliases, which are forbidden".to_string(),
            },
        }
    }
}

/// Verify a `forge.plan.ready` event payload against the raw artifact bytes.
///
/// Pipeline:
/// 1. Canonicalize the artifact via U9 (bounds enforcement + deterministic
///    digest).
/// 2. Extract `plan_digest` from the event payload.
/// 3. Compare the canonical digest against the payload's claimed digest.
/// 4. Return `Ok(canonical)` on match, `Err(DigestMismatch)` on mismatch.
///
/// # Errors
///
/// Returns [`HandoffError::DigestMismatch`] when the payload's `plan_digest`
/// does not match the canonical digest, or any [`HandoffError`] variant
/// propagated from U9 canonicalization (size / unit / edge / parse errors).
pub fn verify_plan_handoff(
    payload: &Value,
    artifact_bytes: &[u8],
) -> Result<crate::artifact_canonicalizer::CanonicalArtifact, HandoffError> {
    let canonical = canonicalize(artifact_bytes)?;

    let expected_digest = payload
        .get("plan_digest")
        .and_then(Value::as_str)
        .unwrap_or("");

    if !expected_digest.is_empty() && canonical.digest != expected_digest {
        return Err(HandoffError::DigestMismatch {
            expected: expected_digest.to_string(),
            actual: canonical.digest.clone(),
        });
    }

    Ok(canonical)
}

/// Read an artifact once from a workspace-bounded regular file.
fn read_artifact_bytes(workspace: &Path, artifact_path: &Path) -> Result<Vec<u8>, HandoffError> {
    if artifact_path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(HandoffError::PathEscape {
            path: artifact_path.display().to_string(),
        });
    }
    let root = workspace.canonicalize().map_err(|error| HandoffError::Io {
        path: workspace.display().to_string(),
        source: error.to_string(),
    })?;
    let candidate = if artifact_path.is_absolute() {
        artifact_path.to_path_buf()
    } else {
        workspace.join(artifact_path)
    };
    let resolved = candidate.canonicalize().map_err(|error| HandoffError::Io {
        path: candidate.display().to_string(),
        source: error.to_string(),
    })?;
    if !resolved.starts_with(&root) {
        return Err(HandoffError::PathEscape {
            path: artifact_path.display().to_string(),
        });
    }
    let before = std::fs::metadata(&resolved).map_err(|error| HandoffError::Io {
        path: resolved.display().to_string(),
        source: error.to_string(),
    })?;
    if !before.is_file() {
        return Err(HandoffError::NotRegularFile {
            path: resolved.display().to_string(),
        });
    }
    let bytes = std::fs::read(&resolved).map_err(|error| HandoffError::Io {
        path: resolved.display().to_string(),
        source: error.to_string(),
    })?;
    let after = std::fs::metadata(&resolved).map_err(|error| HandoffError::Io {
        path: resolved.display().to_string(),
        source: error.to_string(),
    })?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(HandoffError::ChangedDuringRead {
            path: resolved.display().to_string(),
        });
    }
    Ok(bytes)
}

/// Read and verify an artifact from a contract-bounded workspace path.
/// The file is opened once, checked as a regular file, read completely, and
/// checked again before canonicalization to detect replacement/TOCTOU races.
pub fn verify_plan_handoff_path(
    payload: &Value,
    workspace: &Path,
    artifact_path: &Path,
) -> Result<crate::artifact_canonicalizer::CanonicalArtifact, HandoffError> {
    let bytes = read_artifact_bytes(workspace, artifact_path)?;
    verify_plan_handoff(payload, &bytes)
}

/// One runtime-derived task specification from `execution-plan.yml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CanonicalTaskSpec {
    pub unit_id: String,
    pub task_key: String,
    pub title: String,
    pub depends_on_task_keys: Vec<String>,
    pub execution_wave: u32,
    pub integration_order: u32,
}

/// Verified Parallel Forge plan handoff. Derived task data never comes from
/// the event payload; the execution-plan artifact is the only authority.
#[derive(Debug, Clone)]
pub struct CanonicalPlanHandoff {
    pub artifact: CanonicalArtifact,
    pub plan_key: String,
    pub tasks: Vec<CanonicalTaskSpec>,
    pub wave_total: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct ExecutionPlanArtifact {
    plan_key: String,
    #[serde(default)]
    resource_capacities: Vec<ResourceCapacity>,
    units: Vec<ExecutionPlanUnit>,
}

#[derive(Debug, Clone, Deserialize)]
struct ExecutionPlanUnit {
    id: String,
    title: String,
    #[serde(default)]
    depends_on: Vec<String>,
    #[serde(rename = "execution_wave")]
    _execution_wave: u32,
    integration_order: u32,
    #[serde(default)]
    target_branch: String,
    #[serde(default)]
    resource_claims: Vec<ResourceClaim>,
}

/// Read, verify, and derive the canonical Parallel Forge task schedule.
///
/// The event payload may only identify the artifact (`execution_plan_path`,
/// `plan_key`, `plan_digest`). Derived fields such as `unit_tasks` are rejected
/// so the planner cannot override the on-disk DAG.
pub fn load_plan_handoff(
    payload: &Value,
    workspace: &Path,
) -> Result<CanonicalPlanHandoff, HandoffError> {
    if payload.get("unit_tasks").is_some() {
        return Err(HandoffError::ParseError {
            source: "forge.plan.ready must not contain derived field 'unit_tasks'".to_string(),
        });
    }

    let artifact_path = payload
        .get("execution_plan_path")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| HandoffError::ParseError {
            source: "forge.plan.ready requires non-empty execution_plan_path".to_string(),
        })?;
    let artifact = verify_plan_handoff_path(payload, workspace, Path::new(artifact_path))?;
    derive_plan_handoff(payload, artifact)
}

fn derive_plan_handoff(
    payload: &Value,
    artifact: CanonicalArtifact,
) -> Result<CanonicalPlanHandoff, HandoffError> {
    let mut plan: ExecutionPlanArtifact = serde_yaml::from_slice(&artifact.canonical_bytes)
        .map_err(|error| HandoffError::ParseError {
            source: format!("invalid Parallel Forge execution plan: {error}"),
        })?;

    validate_and_normalize_plan(&mut plan).map_err(|error| HandoffError::ParseError {
        source: error.to_string(),
    })?;

    let payload_plan_key = payload
        .get("plan_key")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| HandoffError::ParseError {
            source: "forge.plan.ready requires non-empty plan_key".to_string(),
        })?;
    if plan.plan_key != payload_plan_key {
        return Err(HandoffError::ParseError {
            source: format!(
                "plan_key mismatch: payload declares '{payload_plan_key}', artifact declares '{}'",
                plan.plan_key
            ),
        });
    }
    if plan.units.is_empty() {
        return Err(HandoffError::ParseError {
            source: "execution plan units must be non-empty".to_string(),
        });
    }

    let mut unit_ids = HashSet::with_capacity(plan.units.len());
    let mut task_keys = HashMap::with_capacity(plan.units.len());
    for unit in &plan.units {
        if unit.id.trim().is_empty() || !unit_ids.insert(unit.id.clone()) {
            return Err(HandoffError::ParseError {
                source: format!(
                    "execution plan has empty or duplicate unit id '{}'",
                    unit.id
                ),
            });
        }
        task_keys.insert(
            unit.id.clone(),
            format!("forge:{}:{}", plan.plan_key, unit.id),
        );
    }

    // `execution_wave` is an authoring hint, not an authority over
    // concurrency. Recompute the earliest safe wave from the dependency DAG
    // so a plan that lists independent Units in serial waves still fans them
    // out. `integration_order` remains the separate deterministic merge order.
    let execution_waves = derive_execution_waves(&plan.units)?;
    let mut tasks = Vec::with_capacity(plan.units.len());
    let mut wave_sizes: HashMap<u32, usize> = HashMap::new();
    for unit in plan.units {
        let execution_wave = execution_waves[&unit.id];
        let depends_on_task_keys = unit
            .depends_on
            .iter()
            .map(|dependency| {
                task_keys
                    .get(dependency)
                    .cloned()
                    .ok_or_else(|| HandoffError::ParseError {
                        source: format!(
                            "unit '{}' depends on unknown unit '{dependency}'",
                            unit.id
                        ),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        tasks.push(CanonicalTaskSpec {
            unit_id: unit.id.clone(),
            task_key: task_keys[&unit.id].clone(),
            title: unit.title,
            depends_on_task_keys,
            execution_wave,
            integration_order: unit.integration_order,
        });
        *wave_sizes.entry(execution_wave).or_default() += 1;
    }
    let max_wave_size = wave_sizes.values().copied().max().unwrap_or(0);
    if max_wave_size < 2 {
        return Err(HandoffError::NoParallelWave { max_wave_size });
    }
    let wave_total = tasks
        .iter()
        .map(|task| task.execution_wave)
        .max()
        .unwrap_or(0);

    Ok(CanonicalPlanHandoff {
        artifact,
        plan_key: plan.plan_key,
        tasks,
        wave_total,
    })
}

/// Compute the earliest safe execution wave from dependency edges.
///
/// This deliberately ignores the artifact's declared `execution_wave` value:
/// the DAG is the hard ordering contract, while the declared value is only a
/// planning hint and must not suppress available parallelism.
fn derive_execution_waves(
    units: &[ExecutionPlanUnit],
) -> Result<HashMap<String, u32>, HandoffError> {
    let known_ids: HashSet<&str> = units.iter().map(|unit| unit.id.as_str()).collect();
    for unit in units {
        for dependency in &unit.depends_on {
            if !known_ids.contains(dependency.as_str()) {
                return Err(HandoffError::ParseError {
                    source: format!("unit '{}' depends on unknown unit '{dependency}'", unit.id),
                });
            }
        }
    }

    let mut waves = HashMap::with_capacity(units.len());
    while waves.len() < units.len() {
        let mut progressed = false;
        for unit in units {
            if waves.contains_key(&unit.id) {
                continue;
            }
            let Some(max_dependency_wave) = unit
                .depends_on
                .iter()
                .map(|dependency| waves.get(dependency).copied())
                .collect::<Option<Vec<_>>>()
                .map(|dependency_waves| dependency_waves.into_iter().max().unwrap_or(0))
            else {
                continue;
            };
            waves.insert(unit.id.clone(), max_dependency_wave + 1);
            progressed = true;
        }
        if !progressed {
            return Err(HandoffError::ParseError {
                source: "execution plan dependency graph contains a cycle".to_string(),
            });
        }
    }
    Ok(waves)
}

/// Normalize a `forge.plan.ready` payload into the runtime-owned summary.
///
/// `plan_digest`, `unit_count`, and `wave_total` are always overwritten from
/// the verified artifact. This function is shared by CLI precheck/apply and
/// EventLoop ingress so both paths accept the same bytes and produce the same
/// canonical payload.
pub fn canonicalize_plan_ready_payload(
    payload_text: &str,
    workspace: &Path,
) -> Result<String, HandoffError> {
    let mut payload: Value =
        serde_json::from_str(payload_text).map_err(|error| HandoffError::ParseError {
            source: format!("forge.plan.ready payload must be a JSON object: {error}"),
        })?;
    let object = payload
        .as_object_mut()
        .ok_or_else(|| HandoffError::ParseError {
            source: "forge.plan.ready payload must be a JSON object".to_string(),
        })?;
    if object.contains_key("unit_tasks") {
        return Err(HandoffError::ParseError {
            source: "forge.plan.ready must not contain derived field 'unit_tasks'".to_string(),
        });
    }

    let artifact_path = object
        .get("execution_plan_path")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| HandoffError::ParseError {
            source: "forge.plan.ready requires non-empty execution_plan_path".to_string(),
        })?
        .to_string();
    let bytes = read_artifact_bytes(workspace, Path::new(&artifact_path))?;
    let computed = canonicalize(&bytes)?;
    if let Some(claimed) = object.get("plan_digest").and_then(Value::as_str)
        && !claimed.is_empty()
        && claimed != computed.digest
    {
        return Err(HandoffError::DigestMismatch {
            expected: claimed.to_string(),
            actual: computed.digest.clone(),
        });
    }
    object.insert(
        "plan_digest".to_string(),
        Value::String(computed.digest.clone()),
    );

    let handoff = derive_plan_handoff(&payload, computed)?;
    let object = payload.as_object_mut().expect("validated object");
    object.insert(
        "unit_count".to_string(),
        Value::from(u64::try_from(handoff.tasks.len()).unwrap_or(u64::MAX)),
    );
    object.insert("wave_total".to_string(), Value::from(handoff.wave_total));
    serde_json::to_string(&payload).map_err(|error| HandoffError::ParseError {
        source: format!("failed to serialize canonical forge.plan.ready payload: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Minimal valid artifact: 1 Unit, 0 edges.
    const MINIMAL_ARTIFACT: &[u8] = b"units:\n  - id: u0\nedges: []\n";

    const PLAN_ARTIFACT: &[u8] = br#"version: 1
plan_key: pf-test
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1-foundation
  - id: U2
    title: Feature
    depends_on: []
    execution_wave: 1
    integration_order: 2
    target_branch: feat/u2-feature
"#;

    const SERIAL_PLAN_ARTIFACT: &[u8] = br#"version: 1
plan_key: pf-test
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1-foundation
  - id: U2
    title: Feature
    depends_on: [U1]
    execution_wave: 2
    integration_order: 2
    target_branch: feat/u2-feature
"#;

    const SERIAL_LAYOUT_ARTIFACT: &[u8] = br#"version: 1
plan_key: pf-test
units:
  - id: U1
    title: Feature A
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1-a
  - id: U2
    title: Feature B
    depends_on: []
    execution_wave: 2
    integration_order: 2
    target_branch: feat/u2-b
"#;

    const UNKNOWN_DEPENDENCY_ARTIFACT: &[u8] = br#"version: 1
plan_key: pf-test
units:
  - id: U1
    title: Feature A
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1-a
  - id: U2
    title: Feature B
    depends_on: [U9]
    execution_wave: 2
    integration_order: 2
    target_branch: feat/u2-b
"#;

    const DEPENDENCY_CYCLE_ARTIFACT: &[u8] = br#"version: 1
plan_key: pf-test
units:
  - id: U1
    title: Feature A
    depends_on: [U2]
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1-a
  - id: U2
    title: Feature B
    depends_on: [U1]
    execution_wave: 2
    integration_order: 2
    target_branch: feat/u2-b
"#;

    #[test]
    fn artifact_first_handoff_derives_tasks_and_schedule() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("execution-plan.yml");
        std::fs::write(&path, PLAN_ARTIFACT).expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-test",
        });

        let handoff = load_plan_handoff(&payload, temp.path()).expect("valid handoff");
        assert_eq!(handoff.tasks.len(), 2);
        assert_eq!(handoff.wave_total, 1);
        assert_eq!(handoff.tasks[0].task_key, "forge:pf-test:U1");
        assert_eq!(handoff.tasks[1].depends_on_task_keys, Vec::<String>::new());
    }

    #[test]
    fn artifact_first_handoff_rejects_serial_only_plan() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("execution-plan.yml"), SERIAL_PLAN_ARTIFACT)
            .expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-test",
        });

        let error = load_plan_handoff(&payload, temp.path())
            .expect_err("parallel-forge must reject serial-only plans");
        assert!(matches!(
            error,
            HandoffError::NoParallelWave { max_wave_size: 1 }
        ));
    }

    #[test]
    fn artifact_first_handoff_rewrites_serial_layout_for_independent_units() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("execution-plan.yml"),
            SERIAL_LAYOUT_ARTIFACT,
        )
        .expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-test",
        });

        let handoff = load_plan_handoff(&payload, temp.path())
            .expect("independent Units must be parallelized");
        assert_eq!(handoff.wave_total, 1);
        assert_eq!(
            handoff
                .tasks
                .iter()
                .map(|task| task.execution_wave)
                .collect::<Vec<_>>(),
            vec![1, 1]
        );
    }

    #[test]
    fn artifact_first_handoff_rejects_payload_derived_tasks() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("execution-plan.yml"), PLAN_ARTIFACT).expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-test",
            "unit_tasks": [],
        });

        let error = load_plan_handoff(&payload, temp.path())
            .expect_err("derived payload tasks must be rejected");
        assert!(
            error
                .to_string()
                .contains("must not contain derived field 'unit_tasks'")
        );
    }

    #[test]
    fn cli_payload_normalization_adds_runtime_summary() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("execution-plan.yml"), PLAN_ARTIFACT).expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-test",
        })
        .to_string();

        let normalized =
            canonicalize_plan_ready_payload(&payload, temp.path()).expect("payload must normalize");
        let value: Value = serde_json::from_str(&normalized).expect("normalized JSON");
        assert_eq!(value["unit_count"], 2);
        assert_eq!(value["wave_total"], 1);
        assert!(
            value["plan_digest"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
        assert!(value.get("unit_tasks").is_none());
    }

    #[test]
    fn u10_forge_plan_ready_carries_canonical_digest() {
        let canonical = crate::artifact_canonicalizer::canonicalize(MINIMAL_ARTIFACT)
            .expect("must canonicalize");

        let payload = json!({
            "plan_digest": canonical.digest,
            "execution_wave": 1,
        });

        let result = verify_plan_handoff(&payload, MINIMAL_ARTIFACT);
        assert!(result.is_ok(), "matching digest must succeed: {result:?}");

        let verified = result.unwrap();
        assert_eq!(verified.digest, canonical.digest);
        assert_eq!(verified.unit_count, 1);
        assert_eq!(verified.edge_count, 0);
    }

    #[test]
    fn u10_forge_plan_ready_rejects_digest_mismatch() {
        let canonical = crate::artifact_canonicalizer::canonicalize(MINIMAL_ARTIFACT)
            .expect("must canonicalize");

        let payload = json!({
            "plan_digest": "wrong_digest",
        });

        let err = verify_plan_handoff(&payload, MINIMAL_ARTIFACT)
            .expect_err("mismatched digest must be rejected");

        assert!(
            matches!(
                &err,
                HandoffError::DigestMismatch { expected, actual }
                    if expected == "wrong_digest" && actual == &canonical.digest
            ),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn u10_forge_plan_ready_rejects_oversized_artifact() {
        // 1 MiB + 1 byte — exceeds the U9 size bound.
        let oversized = vec![b'a'; crate::artifact_canonicalizer::MAX_ARTIFACT_BYTES + 1];

        let payload = json!({ "plan_digest": "any" });

        let err = verify_plan_handoff(&payload, &oversized)
            .expect_err("oversized artifact must be rejected");

        assert!(
            matches!(
                err,
                HandoffError::ArtifactTooLarge { size, limit }
                    if size == crate::artifact_canonicalizer::MAX_ARTIFACT_BYTES + 1
                    && limit == crate::artifact_canonicalizer::MAX_ARTIFACT_BYTES
            ),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn artifact_first_handoff_rejects_unknown_dependency() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("execution-plan.yml"),
            UNKNOWN_DEPENDENCY_ARTIFACT,
        )
        .expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-test",
        });

        let error = load_plan_handoff(&payload, temp.path())
            .expect_err("a dependency on an unknown unit must be rejected");
        assert!(
            matches!(&error, HandoffError::ParseError { .. }),
            "unexpected error: {error:?}"
        );
        assert!(error.to_string().contains("unknown unit"));
    }

    #[test]
    fn artifact_first_handoff_rejects_dependency_cycle() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            temp.path().join("execution-plan.yml"),
            DEPENDENCY_CYCLE_ARTIFACT,
        )
        .expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-test",
        });

        let error = load_plan_handoff(&payload, temp.path())
            .expect_err("a dependency cycle must be rejected");
        assert!(
            matches!(&error, HandoffError::ParseError { .. }),
            "unexpected error: {error:?}"
        );
        assert!(error.to_string().contains("cycle"));
    }

    // ── U2 (plan 2026-09-03-0959) resources + canonical digest v2 ──────

    /// v2 happy-path plan: two independent Units, one capacity, one claim.
    const V2_HAPPY_ARTIFACT: &[u8] = br#"version: 1
plan_key: pf-v2
resource_capacities:
  - key: gpu
    capacity: 1
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1
    resource_claims:
      - key: gpu
        permits: 1
  - id: U2
    title: Feature
    depends_on: []
    execution_wave: 1
    integration_order: 2
    target_branch: feat/u2
"#;

    /// v1-shaped artifact (no resource_capacities, no target_branch, no claims):
    /// shadow migration must parse to empty resources.
    const V1_ABSENCE_ARTIFACT: &[u8] = br#"version: 1
plan_key: pf-v1
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
  - id: U2
    title: Feature
    depends_on: []
    execution_wave: 1
    integration_order: 2
"#;

    #[test]
    fn u2_happy_path_roundtrips_and_digest_is_stable() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("execution-plan.yml"), V2_HAPPY_ARTIFACT)
            .expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-v2",
        });

        let first = load_plan_handoff(&payload, temp.path()).expect("v2 plan must load");
        assert_eq!(first.tasks.len(), 2);

        // Canonicalize the raw bytes a second time and confirm the digest
        // matches the canonical artifact's digest.
        let bytes = std::fs::read(temp.path().join("execution-plan.yml")).expect("read plan");
        let canonical = crate::artifact_canonicalizer::canonicalize(&bytes).expect("canon");
        assert_eq!(first.artifact.digest, canonical.digest);
    }

    #[test]
    fn u2_unknown_resource_claim_is_rejected() {
        let raw = br#"version: 1
plan_key: pf-v2
resource_capacities:
  - key: cpu
    capacity: 4
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1
    resource_claims:
      - key: gpu
        permits: 1
"#;
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("execution-plan.yml"), raw).expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-v2",
        });
        let error = load_plan_handoff(&payload, temp.path())
            .expect_err("unknown resource claim must be rejected");
        assert!(matches!(error, HandoffError::ParseError { .. }));
        assert!(
            error.to_string().contains("claims resource 'gpu'"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn u2_duplicate_capacity_key_is_rejected() {
        let raw = br#"version: 1
plan_key: pf-v2
resource_capacities:
  - key: cpu
    capacity: 1
  - key: cpu
    capacity: 2
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1
"#;
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("execution-plan.yml"), raw).expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-v2",
        });
        let error = load_plan_handoff(&payload, temp.path())
            .expect_err("duplicate capacity key must be rejected");
        assert!(matches!(error, HandoffError::ParseError { .. }));
        assert!(
            error.to_string().contains("duplicate key 'cpu'"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn u2_duplicate_claim_key_within_one_unit_is_rejected() {
        let raw = br#"version: 1
plan_key: pf-v2
resource_capacities:
  - key: cpu
    capacity: 4
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1
    resource_claims:
      - key: cpu
        permits: 1
      - key: cpu
        permits: 2
"#;
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("execution-plan.yml"), raw).expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-v2",
        });
        let error = load_plan_handoff(&payload, temp.path())
            .expect_err("duplicate claim key must be rejected");
        assert!(matches!(error, HandoffError::ParseError { .. }));
        assert!(
            error
                .to_string()
                .contains("duplicate resource_claims entry for key 'cpu'"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn u2_zero_permits_claim_is_rejected() {
        let raw = br#"version: 1
plan_key: pf-v2
resource_capacities:
  - key: cpu
    capacity: 1
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1
    resource_claims:
      - key: cpu
        permits: 0
"#;
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("execution-plan.yml"), raw).expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-v2",
        });
        let error =
            load_plan_handoff(&payload, temp.path()).expect_err("permits=0 must be rejected");
        assert!(matches!(error, HandoffError::ParseError { .. }));
        assert!(
            error.to_string().contains("permits=0"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn u2_zero_capacity_is_rejected() {
        let raw = br#"version: 1
plan_key: pf-v2
resource_capacities:
  - key: cpu
    capacity: 0
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1
"#;
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("execution-plan.yml"), raw).expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-v2",
        });
        let error =
            load_plan_handoff(&payload, temp.path()).expect_err("capacity=0 must be rejected");
        assert!(matches!(error, HandoffError::ParseError { .. }));
        assert!(
            error.to_string().contains("capacity=0"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn u2_claim_exceeding_capacity_is_accepted_at_parse() {
        // U2 only canonicalizes — single claim > capacity is U4's job to
        // enforce at admission time. This test pins that contract.
        // Two independent Units keep the wave size ≥ 2 so we don't trip
        // parallel-forge's `NoParallelWave` gate before reaching validation.
        let raw = br#"version: 1
plan_key: pf-v2
resource_capacities:
  - key: cpu
    capacity: 1
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1
    resource_claims:
      - key: cpu
        permits: 4294967295
  - id: U2
    title: Feature
    depends_on: []
    execution_wave: 1
    integration_order: 2
    target_branch: feat/u2
"#;
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("execution-plan.yml"), raw).expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-v2",
        });
        let handoff = load_plan_handoff(&payload, temp.path())
            .expect("U2 must accept permits > capacity; U4 enforces");
        assert_eq!(handoff.tasks.len(), 2);
    }

    #[test]
    fn u2_empty_target_branch_is_rejected() {
        let raw = br#"version: 1
plan_key: pf-v2
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: ""
"#;
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("execution-plan.yml"), raw).expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-v2",
        });
        let error = load_plan_handoff(&payload, temp.path())
            .expect_err("empty target_branch must be rejected");
        assert!(matches!(error, HandoffError::ParseError { .. }));
        assert!(
            error.to_string().contains("empty target_branch"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn u2_unsafe_target_branches_are_rejected() {
        let unsafe_branches: &[&str] = &[
            "..",
            "-foo",
            "foo/",
            "foo//bar",
            "foo/.lock",
            "foo@{0}",
            "foo bar",
            "foo~bar",
            "foo^bar",
            "foo:bar",
            "foo?bar",
            "foo*bar",
            "foo[bar",
            "foo\\bar",
            "foo.",
        ];
        for branch in unsafe_branches {
            let yaml = format!(
                "version: 1\nplan_key: pf-v2\nunits:\n  - id: U1\n    title: Foundation\n    depends_on: []\n    execution_wave: 1\n    integration_order: 1\n    target_branch: \"{branch}\"\n"
            );
            let temp = tempfile::tempdir().expect("tempdir");
            std::fs::write(temp.path().join("execution-plan.yml"), yaml.as_bytes())
                .expect("write plan");
            let payload = json!({
                "execution_plan_path": "execution-plan.yml",
                "plan_key": "pf-v2",
            });
            match load_plan_handoff(&payload, temp.path()) {
                Ok(_) => {
                    panic!("unsafe target_branch '{branch}' was accepted; validator must reject it")
                }
                Err(error) => {
                    assert!(
                        matches!(error, HandoffError::ParseError { .. }),
                        "unsafe target_branch '{branch}' produced wrong error variant: {error:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn u2_resource_aware_digest_is_stable_across_reorder() {
        // Same logical plan, expressed twice with `resource_capacities` in
        // a different Vec order, must produce the same resource-aware
        // digest (digest normalizes by sorting capacities + per-unit claims).
        let plan_alpha_yaml: &[u8] = br#"version: 1
plan_key: pf-v2
resource_capacities:
  - key: gpu
    capacity: 1
  - key: cpu
    capacity: 4
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1
    resource_claims:
      - key: gpu
        permits: 1
  - id: U2
    title: Feature
    depends_on: []
    execution_wave: 1
    integration_order: 2
    target_branch: feat/u2
    resource_claims:
      - key: cpu
        permits: 2
"#;
        let plan_bravo_yaml: &[u8] = br#"version: 1
plan_key: pf-v2
resource_capacities:
  - key: cpu
    capacity: 4
  - key: gpu
    capacity: 1
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
    target_branch: feat/u1
    resource_claims:
      - key: gpu
        permits: 1
  - id: U2
    title: Feature
    depends_on: []
    execution_wave: 1
    integration_order: 2
    target_branch: feat/u2
    resource_claims:
      - key: cpu
        permits: 2
"#;

        let mut plan_alpha: ExecutionPlanArtifact =
            serde_yaml::from_slice(plan_alpha_yaml).expect("alpha parses");
        let mut plan_bravo: ExecutionPlanArtifact =
            serde_yaml::from_slice(plan_bravo_yaml).expect("bravo parses");
        validate_and_normalize_plan(&mut plan_alpha).expect("alpha validates");
        validate_and_normalize_plan(&mut plan_bravo).expect("bravo validates");

        let units_alpha: Vec<ExecutionPlanUnitView> = plan_alpha
            .units
            .iter()
            .map(|u| ExecutionPlanUnitView {
                id: &u.id,
                target_branch: &u.target_branch,
                depends_on: &u.depends_on,
                resource_claims: &u.resource_claims,
            })
            .collect();
        let units_bravo: Vec<ExecutionPlanUnitView> = plan_bravo
            .units
            .iter()
            .map(|u| ExecutionPlanUnitView {
                id: &u.id,
                target_branch: &u.target_branch,
                depends_on: &u.depends_on,
                resource_claims: &u.resource_claims,
            })
            .collect();

        let digest_alpha = compute_resource_aware_digest(
            &plan_alpha.plan_key,
            &plan_alpha.resource_capacities,
            &units_alpha,
        );
        let digest_bravo = compute_resource_aware_digest(
            &plan_bravo.plan_key,
            &plan_bravo.resource_capacities,
            &units_bravo,
        );
        assert_eq!(
            digest_alpha, digest_bravo,
            "reorder of resource_capacities must not change the resource-aware digest"
        );

        // Stability: same plan twice produces the same digest.
        let digest_alpha2 = compute_resource_aware_digest(
            &plan_alpha.plan_key,
            &plan_alpha.resource_capacities,
            &units_alpha,
        );
        assert_eq!(digest_alpha, digest_alpha2);
    }

    #[test]
    fn u2_v1_absence_parses_to_empty_resources() {
        let raw = V1_ABSENCE_ARTIFACT;
        let plan: ExecutionPlanArtifact = serde_yaml::from_slice(raw)
            .expect("v1-shaped plan without resource fields still parses");
        assert!(plan.resource_capacities.is_empty());
        for unit in &plan.units {
            assert!(unit.resource_claims.is_empty());
            assert_eq!(unit.target_branch, "");
        }

        // Note: validation REJECTS the v1 plan because target_branch is
        // empty. This test only asserts the parser defaults, per the U2
        // spec ("v1 absence maps empty resources for shadow migration
        // only"). The handoff path must therefore refuse v1 plans without
        // target_branch — see u2_empty_target_branch_is_rejected.
    }
}
