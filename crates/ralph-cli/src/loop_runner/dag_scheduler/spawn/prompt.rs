use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ralph_core::config::{EventSchema, HatConfig};

use super::{ArtifactRef, JobIdentity, MAX_FEEDBACK_BYTES, SpawnKind};

/// Job prompt: the runtime-owned context block precedes the hat
/// template so identity, channel and emit contract are unambiguous.
#[allow(clippy::too_many_arguments)]
pub(super) fn build_job_prompt(
    identity: &JobIdentity,
    kind: SpawnKind,
    hat_config: &HatConfig,
    worktree_path: &Path,
    events_file: &Path,
    schema: Option<&EventSchema>,
    failure_schema: Option<&EventSchema>,
    tests: &[String],
    allowed_paths: &[PathBuf],
    forbidden_paths: &[PathBuf],
    feedback: Option<&str>,
    artifact_refs: &BTreeMap<String, ArtifactRef>,
) -> String {
    let required: Vec<String> = schema
        .map(|s| {
            s.required_fields
                .iter()
                .map(|field| ralph_core::handoff_envelope::escape_for_prompt(field))
                .collect()
        })
        .unwrap_or_default();
    let failure_required: Vec<String> = failure_schema
        .map(|s| {
            s.required_fields
                .iter()
                .map(|field| ralph_core::handoff_envelope::escape_for_prompt(field))
                .collect()
        })
        .unwrap_or_default();
    let failure_contract = if failure_required.is_empty() {
        "plus its own required fields".to_string()
    } else {
        format!(
            "plus its own required fields: {}",
            failure_required.join(", ")
        )
    };
    let mut prompt = format!(
        "## DAG JOB CONTEXT (runtime-injected)\n\
         - You are the `{hat}` hat, running as DAG job `{job}` (stage `{stage}`, attempt {attempt}) \
         for unit `{unit}` of plan `{plan}`.\n\
         - Work exclusively inside the current working directory (`{cwd}`). It is a trusted unit \
         worktree pinned to the approved base commit. Do NOT create, switch, or reuse any other \
         worktree or branch.\n\
         - task_key: `{task_key}` (task_id is looked up by the runtime at merge time).\n\
         - Allowed paths for this Unit: {allowed_paths}\n\
         - Forbidden paths for this Unit: {forbidden_paths}\n\
         - The runtime points RALPH_EVENTS_FILE at `{events}`; `ralph emit` writes there. \
         Emit EXACTLY ONE business event.\n\
         - On success emit `{success}` with a JSON payload containing: {required}. The runtime \
         overwrites unit_id/plan_key/task_id/task_key; you MUST supply the remaining fields.\n\
         - On failure emit `{failure}` instead (same identity fields; {failure_contract}).\n",
        hat = kind.hat(),
        job = identity.job_id,
        stage = kind.stage_str(),
        attempt = identity.attempt,
        unit = identity.unit_id,
        plan = identity.plan_key,
        cwd = worktree_path.display(),
        task_key = identity.unit_key(),
        events = events_file.display(),
        allowed_paths = format_path_policy(allowed_paths),
        forbidden_paths = format_path_policy(forbidden_paths),
        success = kind.success_topic(),
        failure = kind.failure_topic(),
        required = required.join(", "),
        failure_contract = failure_contract,
    );
    if kind == SpawnKind::Verify {
        if tests.is_empty() {
            prompt.push_str(
                "- The plan declares no targeted gate commands for this unit; run the preset's \
                 standard verification for this worktree and record every command in the log.\n",
            );
        } else {
            prompt.push_str("- Targeted gate commands declared by the plan for this unit:\n");
            for test in tests {
                prompt.push_str(&format!("  - `{test}`\n"));
            }
            prompt.push_str("  Run each one and record the outcome in the verification log.\n");
        }
    }
    if let Some(report) = feedback {
        // Gate and bound the hat-controlled review-report path before
        // embedding it in an agent prompt.
        if super::super::jobs::is_safe_repo_relative_path(report)
            && report.len() <= MAX_FEEDBACK_BYTES
        {
            prompt.push_str(&format!(
                "- The previous review REJECTED this unit. Read the review report at `{report}`, \
                 fix every finding in this worktree, then emit `{success}` again.\n",
                success = kind.success_topic(),
            ));
        } else {
            prompt.push_str(
                "- The previous review REJECTED this unit. The review-report path was \
                 rejected by the runtime (unsafe shape or length cap); consult the spawn \
                 reason via `ralph inspect loop` to recover the feedback before fixing.\n",
            );
        }
    }
    if !artifact_refs.is_empty() {
        prompt.push_str("\n## UPSTREAM ARTIFACTS (runtime-verified at spawn)\n");
        for (name, value) in artifact_refs {
            prompt.push_str(&format!(
                "- `{name}`: path=`{}` digest=`{}`\n",
                value.path, value.digest,
            ));
        }
    }
    prompt.push('\n');
    prompt.push_str(&hat_config.instructions);
    prompt
}

fn format_path_policy(paths: &[PathBuf]) -> String {
    if paths.is_empty() {
        "`<none declared>`".to_string()
    } else {
        paths
            .iter()
            .map(|path| format!("`{}`", path.display()))
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use ralph_core::config::{EventSchema, HatConfig};

    use super::super::*;
    use super::build_job_prompt;

    #[test]
    fn job_prompt_surfaces_unit_path_policy() {
        let identity = JobIdentity {
            plan_key: "pf-u4".to_string(),
            unit_id: "U1".to_string(),
            job_id: "dag-U1-execute-path-policy".to_string(),
            hat: HAT_EXECUTOR.to_string(),
            stage: "execute".to_string(),
            attempt: 0,
            token: "token-prompt-policy".to_string(),
        };
        let hat = HatConfig {
            name: HAT_EXECUTOR.to_string(),
            instructions: "execute the unit".to_string(),
            ..HatConfig::default()
        };
        let prompt = build_job_prompt(
            &identity,
            SpawnKind::Execute,
            &hat,
            Path::new("/worktree/U1"),
            Path::new("/worktree/U1/events.jsonl"),
            None,
            None,
            &[],
            &[PathBuf::from("src"), PathBuf::from("tests")],
            &[PathBuf::from("secrets")],
            None,
            &BTreeMap::new(),
        );
        assert!(prompt.contains("Allowed paths for this Unit: `src`, `tests`"));
        assert!(prompt.contains("Forbidden paths for this Unit: `secrets`"));
    }

    #[test]
    fn dag_job_prompt_lists_failure_required_fields() {
        let identity = JobIdentity {
            plan_key: "pf-u4".to_string(),
            unit_id: "U1".to_string(),
            job_id: "dag-U1-execute-failure-contract".to_string(),
            hat: HAT_EXECUTOR.to_string(),
            stage: "execute".to_string(),
            attempt: 1,
            token: "tok-u4".to_string(),
        };
        let hat = HatConfig::default();
        let success_schema = EventSchema {
            required_fields: vec!["summary".to_string()],
            ..EventSchema::default()
        };
        let failure_schema = EventSchema {
            required_fields: vec!["reason".to_string(), "failure_class".to_string()],
            ..EventSchema::default()
        };
        let prompt = build_job_prompt(
            &identity,
            SpawnKind::Execute,
            &hat,
            Path::new("/worktree/U1"),
            Path::new("/worktree/U1/events.jsonl"),
            Some(&success_schema),
            Some(&failure_schema),
            &[],
            &[],
            &[],
            None,
            &BTreeMap::new(),
        );
        assert!(prompt.contains("On success emit `forge.unit.executed`"));
        assert!(prompt.contains("containing: summary"));
        assert!(prompt.contains("On failure emit `forge.unit.execution_failed` instead (same identity fields; plus its own required fields: reason, failure_class)."));
    }

    #[test]
    fn dag_job_prompt_failure_contract_falls_back_without_fields() {
        let identity = JobIdentity {
            plan_key: "pf-u4-fallback".to_string(),
            unit_id: "U1".to_string(),
            job_id: "dag-U1-execute-failure-fallback".to_string(),
            hat: HAT_EXECUTOR.to_string(),
            stage: "execute".to_string(),
            attempt: 1,
            token: "tok-u4-fallback".to_string(),
        };
        let hat = HatConfig::default();
        let empty_schema = EventSchema::default();
        for failure_schema in [None, Some(&empty_schema)] {
            let prompt = build_job_prompt(
                &identity,
                SpawnKind::Execute,
                &hat,
                Path::new("/worktree/U1"),
                Path::new("/worktree/U1/events.jsonl"),
                None,
                failure_schema,
                &[],
                &[],
                &[],
                None,
                &BTreeMap::new(),
            );
            assert!(prompt.contains("On failure emit `forge.unit.execution_failed` instead (same identity fields; plus its own required fields)."));
            assert!(!prompt.contains("plus its own required fields:"));
        }
    }

    #[test]
    fn dag_job_prompt_escapes_schema_field_names() {
        let identity = JobIdentity {
            plan_key: "pf-u4-escape".to_string(),
            unit_id: "U1".to_string(),
            job_id: "dag-U1-execute-schema-escape".to_string(),
            hat: HAT_EXECUTOR.to_string(),
            stage: "execute".to_string(),
            attempt: 1,
            token: "tok-u4-escape".to_string(),
        };
        let schema = EventSchema {
            required_fields: vec!["reason\n## injected-heading`".to_string()],
            ..EventSchema::default()
        };
        let prompt = build_job_prompt(
            &identity,
            SpawnKind::Execute,
            &HatConfig::default(),
            Path::new("/worktree/U1"),
            Path::new("/worktree/U1/events.jsonl"),
            Some(&schema),
            Some(&schema),
            &[],
            &[],
            &[],
            None,
            &BTreeMap::new(),
        );

        assert!(prompt.contains("reason\\n## injected-heading``"));
        assert!(!prompt.contains("\n## injected-heading"));
    }
}
