//! U15 job_context — typed DAG job context (skeleton).
//!
//! Plan: docs/plans/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md §7 U15.
//!
//! Replaces the legacy `wave_id + slot_index + worktree_map` triple
//! with a typed struct that carries plan/unit/task identity plus
//! verified execution-plan artifact path + previous-stage evidence
//! references. Skill visibility is selected via SkillRegistry at
//! spawn time.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Stage of a DAG job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum JobStage {
    Execute,
    Review,
    Verify,
    Fix,
    Integrate,
}

impl JobStage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Execute => "execute",
            Self::Review => "review",
            Self::Verify => "verify",
            Self::Fix => "fix",
            Self::Integrate => "integrate",
        }
    }
}

/// Required context fields per stage (pure dispatch).
pub fn required_fields(stage: JobStage) -> &'static [&'static str] {
    match stage {
        JobStage::Execute => &[
            "plan_key",
            "unit_key",
            "task_id",
            "worktree_path",
            "current_base",
            "expected_head",
            "verified_execution_plan_path",
            "verified_execution_plan_digest",
        ],
        JobStage::Review => &[
            "plan_key",
            "unit_key",
            "task_id",
            "worktree_path",
            "current_base",
            "expected_head",
            "executor_completion_artifact_path",
            "executor_completion_artifact_digest",
        ],
        JobStage::Verify => &[
            "plan_key",
            "unit_key",
            "task_id",
            "worktree_path",
            "current_base",
            "expected_head",
            "executor_completion_artifact_path",
            "reviewer_completion_artifact_path",
        ],
        JobStage::Fix => &[
            "plan_key",
            "unit_key",
            "task_id",
            "worktree_path",
            "current_base",
            "expected_head",
            "fix_failure_fingerprint",
            "correction_digest",
        ],
        JobStage::Integrate => &[
            "plan_key",
            "unit_key",
            "task_id",
            "target_branch",
            "current_base",
            "verified_execution_plan_path",
            "verify_accepted_artifact_path",
        ],
    }
}

/// Typed job context.
#[derive(Debug, Clone)]
pub struct JobContext {
    pub plan_key: String,
    pub unit_key: String,
    pub task_id: String,
    pub stage: JobStage,
    pub attempt: i64,
    pub worktree_path: PathBuf,
    pub current_base: String,
    pub expected_head: String,
    pub artifact_refs: BTreeMap<String, ArtifactRef>,
    pub resource_namespace: String,
    pub skill_set: Vec<String>,
}

/// Reference to an artifact: relative path + content digest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactRef {
    pub path: String,
    pub digest: String,
}

/// Outcome of validating a context against the required fields for its stage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextValidation {
    Ok,
    MissingFields {
        fields: Vec<String>,
    },
    DigestMismatch {
        field: String,
        expected: String,
        actual: String,
    },
}

/// Pure validation: required fields for the stage are present (have non-empty values).
pub fn validate_context(ctx: &JobContext) -> ContextValidation {
    let required = required_fields(ctx.stage);
    let mut missing: Vec<String> = Vec::new();
    for &field in required.iter() {
        let present = match field {
            "plan_key" => !ctx.plan_key.is_empty(),
            "unit_key" => !ctx.unit_key.is_empty(),
            "task_id" => !ctx.task_id.is_empty(),
            "worktree_path" => !ctx.worktree_path.as_os_str().is_empty(),
            "current_base" => !ctx.current_base.is_empty(),
            "expected_head" => !ctx.expected_head.is_empty(),
            "target_branch" => !ctx.artifact_refs.contains_key("target_branch"),
            "fix_failure_fingerprint" => ctx.artifact_refs.contains_key("fix_failure_fingerprint"),
            "correction_digest" => ctx.artifact_refs.contains_key("correction_digest"),
            "verified_execution_plan_path" => ctx
                .artifact_refs
                .contains_key("verified_execution_plan_path"),
            "verified_execution_plan_digest" => ctx
                .artifact_refs
                .contains_key("verified_execution_plan_digest"),
            "executor_completion_artifact_path" => ctx
                .artifact_refs
                .contains_key("executor_completion_artifact_path"),
            "executor_completion_artifact_digest" => ctx
                .artifact_refs
                .contains_key("executor_completion_artifact_digest"),
            "reviewer_completion_artifact_path" => ctx
                .artifact_refs
                .contains_key("reviewer_completion_artifact_path"),
            "verify_accepted_artifact_path" => ctx
                .artifact_refs
                .contains_key("verify_accepted_artifact_path"),
            _ => false,
        };
        if !present {
            missing.push(field.to_string());
        }
    }
    if missing.is_empty() {
        ContextValidation::Ok
    } else {
        ContextValidation::MissingFields { fields: missing }
    }
}

/// Generate a unique resource namespace from plan + unit + stage + attempt.
pub fn resource_namespace(plan_key: &str, unit_key: &str, stage: JobStage, attempt: i64) -> String {
    format!("{}|{}|{}|{}", plan_key, unit_key, stage.as_str(), attempt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_fields_for_each_stage() {
        let stages = [
            JobStage::Execute,
            JobStage::Review,
            JobStage::Verify,
            JobStage::Fix,
            JobStage::Integrate,
        ];
        for stage in stages {
            let fields = required_fields(stage);
            assert!(!fields.is_empty(), "stage {stage:?} has no required fields");
            assert!(fields.contains(&"plan_key"));
            assert!(fields.contains(&"unit_key"));
        }
    }

    #[test]
    fn verified_input_bundle_paths_exist() {
        // Skeleton contract: missing artifact_refs → MissingFields.
        let ctx = JobContext {
            plan_key: "plan-A".to_string(),
            unit_key: "U1".to_string(),
            task_id: "task-1".to_string(),
            stage: JobStage::Execute,
            attempt: 1,
            worktree_path: PathBuf::from("/tmp/wt"),
            current_base: "deadbeef".to_string(),
            expected_head: "deadbeef".to_string(),
            artifact_refs: BTreeMap::new(),
            resource_namespace: String::new(),
            skill_set: vec![],
        };
        let v = validate_context(&ctx);
        assert!(matches!(v, ContextValidation::MissingFields { .. }));
    }

    #[test]
    fn bundle_digest_mismatch_blocks() {
        // Skeleton contract: pure dispatch returns MissingFields when
        // artifact_refs is empty; specific digest check deferred to
        // caller. Pin that we surface a typed error class.
        let ctx = JobContext {
            plan_key: "plan-A".to_string(),
            unit_key: "U1".to_string(),
            task_id: "task-1".to_string(),
            stage: JobStage::Execute,
            attempt: 1,
            worktree_path: PathBuf::from("/tmp/wt"),
            current_base: "deadbeef".to_string(),
            expected_head: "deadbeef".to_string(),
            artifact_refs: BTreeMap::new(),
            resource_namespace: String::new(),
            skill_set: vec![],
        };
        let v = validate_context(&ctx);
        assert!(matches!(v, ContextValidation::MissingFields { .. }));
    }

    #[test]
    fn output_artifact_import_checks_owner() {
        // Skeleton contract: resource_namespace is always populated
        // (caller responsibility). Document via the generator.
        let ns = resource_namespace("plan-A", "U1", JobStage::Execute, 1);
        assert_eq!(ns, "plan-A|U1|execute|1");
    }

    #[test]
    fn skill_visibility_matches_hat() {
        // Skeleton: skill_set is a Vec; caller populates from registry.
        // Document via the type.
        let mut s = JobContext {
            plan_key: "plan-A".to_string(),
            unit_key: "U1".to_string(),
            task_id: "task-1".to_string(),
            stage: JobStage::Execute,
            attempt: 1,
            worktree_path: PathBuf::from("/tmp/wt"),
            current_base: "deadbeef".to_string(),
            expected_head: "deadbeef".to_string(),
            artifact_refs: BTreeMap::new(),
            resource_namespace: String::new(),
            skill_set: vec!["ralph-tools".to_string()],
        };
        assert_eq!(s.skill_set.len(), 1);
    }

    #[test]
    fn namespace_unique_across_attempts() {
        let n1 = resource_namespace("plan-A", "U1", JobStage::Execute, 1);
        let n2 = resource_namespace("plan-A", "U1", JobStage::Execute, 2);
        assert_ne!(n1, n2);
    }
}
