//! `monitor-descriptor/v1` sidecar resolver (Plan2-U1).
//!
//! A monitor descriptor maps a preset's topic stream onto UI stages,
//! progress denominator, artifacts and DAG hints. It is a *render
//! contract*, never an execution input — `inspect run` resolves it
//! read-only and emits `preset_descriptor` (or `null`) in the
//! observation snapshot.
//!
//! Resolution order:
//!   1. builtin sidecar embedded from `presets/monitor/<name>.yml`;
//!   2. workspace `.ralph/monitor/<name>.yml` custom override;
//!   3. `resolved_from: "none"` (no descriptor).
//!
//! Fail-closed validation lives here so a hostile or broken sidecar
//! can only degrade rendering, never corrupt observation:
//!   * schema_version must equal `monitor-descriptor/v1`
//!     (`unsupported_version` diagnostic otherwise);
//!   * malformed YAML → `resolved_from: "invalid"` + diagnostic;
//!   * `path_glob` rejects absolute paths and `..` components —
//!     the descriptor itself is the artifact-channel security
//!     boundary for Plan2-U3;
//!   * unknown `progress.denominator` → `invalid`.

use std::path::Path;

use serde::Deserialize;

/// Descriptor schema tag; bumped only on breaking changes.
pub const DESCRIPTOR_SCHEMA_VERSION: &str = "monitor-descriptor/v1";

/// Default `dag.unit_topic_prefix` when a DAG descriptor omits it.
pub const DEFAULT_UNIT_TOPIC_PREFIX: &str = "forge.unit.";

/// Builtin sidecars embedded at compile time (copied into `$OUT_DIR`
/// by `build.rs` from the canonical `presets/monitor/` tree).
const BUILTIN_DESCRIPTORS: &[(&str, &str)] = &[
    (
        "ce-executor-pipeline",
        include_str!(concat!(
            env!("OUT_DIR"),
            "/monitor-descriptors/ce-executor-pipeline.yml"
        )),
    ),
    (
        "parallel-forge",
        include_str!(concat!(
            env!("OUT_DIR"),
            "/monitor-descriptors/parallel-forge.yml"
        )),
    ),
    (
        "autoresearch",
        include_str!(concat!(
            env!("OUT_DIR"),
            "/monitor-descriptors/autoresearch.yml"
        )),
    ),
];

#[derive(Debug, Clone, Deserialize)]
pub struct DescriptorStage {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub topics: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DescriptorProgress {
    /// `stages` is the only supported denominator in v1.
    pub denominator: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DescriptorArtifact {
    pub id: String,
    /// Repo-relative glob; must not be absolute or contain `..`.
    pub path_glob: String,
    pub kind: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DescriptorDag {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_unit_topic_prefix")]
    pub unit_topic_prefix: String,
}

fn default_unit_topic_prefix() -> String {
    DEFAULT_UNIT_TOPIC_PREFIX.to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub struct MonitorDescriptor {
    pub schema_version: String,
    pub preset: String,
    #[serde(default)]
    pub stages: Vec<DescriptorStage>,
    pub progress: DescriptorProgress,
    #[serde(default)]
    pub artifacts: Vec<DescriptorArtifact>,
    pub dag: DescriptorDag,
}

/// Resolution outcome — `descriptor` is `None` for every non-success
/// case; `diagnostic` explains why for operators.
#[derive(Debug, Clone)]
pub struct DescriptorResolution {
    pub resolved_from: &'static str,
    pub descriptor: Option<MonitorDescriptor>,
    pub diagnostic: Option<String>,
}

/// Strip `builtin:`/`directory:` wrappers down to the sidecar stem.
fn preset_stem(preset: &str) -> &str {
    preset
        .strip_prefix("builtin:")
        .or_else(|| preset.strip_prefix("directory:"))
        .unwrap_or(preset)
}

/// Reject names that could escape `.ralph/monitor/` via traversal.
fn safe_stem(name: &str) -> bool {
    !name.is_empty() && !name.contains('/') && !name.contains('\\') && !name.contains("..")
}

fn glob_is_safe(glob: &str) -> bool {
    if glob.is_empty() || Path::new(glob).is_absolute() {
        return false;
    }
    !glob.split('/').any(|seg| seg == "..")
}

/// Parse + validate a descriptor document. Returns a diagnostic on
/// failure instead of panicking — a bad sidecar degrades to
/// `resolved_from: "invalid"`/`"unsupported_version"`, never a crash.
fn parse_descriptor(text: &str) -> Result<MonitorDescriptor, String> {
    let doc: serde_yaml::Value =
        serde_yaml::from_str(text).map_err(|e| format!("yaml parse: {e}"))?;
    let version = doc
        .get("schema_version")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if version != DESCRIPTOR_SCHEMA_VERSION {
        return Err(format!(
            "unsupported_version: expected {DESCRIPTOR_SCHEMA_VERSION}, found `{version}`"
        ));
    }
    let d: MonitorDescriptor = serde_yaml::from_value(doc).map_err(|e| format!("schema: {e}"))?;
    if d.progress.denominator != "stages" {
        return Err(format!(
            "progress.denominator must be `stages`, found `{}`",
            d.progress.denominator
        ));
    }
    for a in &d.artifacts {
        if !glob_is_safe(&a.path_glob) {
            return Err(format!(
                "artifacts[{}].path_glob must be repo-relative without `..`: `{}`",
                a.id, a.path_glob
            ));
        }
    }
    // stages.topics dedup warning → diagnostic, not an error.
    let mut seen = std::collections::BTreeSet::new();
    let mut dups = Vec::new();
    for s in &d.stages {
        for t in &s.topics {
            if !seen.insert(t.as_str()) {
                dups.push(t.clone());
            }
        }
    }
    if !dups.is_empty() {
        return Err(format!(
            "duplicate topics across stages: {}",
            dups.join(", ")
        ));
    }
    Ok(d)
}

/// Resolve a preset reference (`builtin:<name>` / `directory:<stem>` /
/// bare name) to a monitor descriptor for `inspect run`.
pub fn resolve(preset_ref: Option<&str>, workspace: &Path) -> DescriptorResolution {
    let Some(raw) = preset_ref else {
        return DescriptorResolution {
            resolved_from: "none",
            descriptor: None,
            diagnostic: None,
        };
    };
    let stem = preset_stem(raw);

    if let Some((_, text)) = BUILTIN_DESCRIPTORS.iter().find(|(n, _)| *n == stem) {
        return match parse_descriptor(text) {
            Ok(d) => DescriptorResolution {
                resolved_from: "builtin",
                descriptor: Some(d),
                diagnostic: None,
            },
            Err(e) => DescriptorResolution {
                resolved_from: "invalid",
                descriptor: None,
                diagnostic: Some(format!("builtin sidecar `{stem}`: {e}")),
            },
        };
    }

    // Workspace custom override — `.ralph/monitor/<stem>.yml`.
    if safe_stem(stem) {
        let path = workspace
            .join(".ralph")
            .join("monitor")
            .join(format!("{stem}.yml"));
        if path.is_file() {
            return match std::fs::read_to_string(&path) {
                Ok(text) => match parse_descriptor(&text) {
                    Ok(d) => DescriptorResolution {
                        resolved_from: "workspace",
                        descriptor: Some(d),
                        diagnostic: None,
                    },
                    Err(e) => DescriptorResolution {
                        resolved_from: if e.starts_with("unsupported_version") {
                            "unsupported_version"
                        } else {
                            "invalid"
                        },
                        descriptor: None,
                        diagnostic: Some(format!("{}: {e}", path.display())),
                    },
                },
                Err(e) => DescriptorResolution {
                    resolved_from: "invalid",
                    descriptor: None,
                    diagnostic: Some(format!("{}: read: {e}", path.display())),
                },
            };
        }
    }

    DescriptorResolution {
        resolved_from: "none",
        descriptor: None,
        diagnostic: None,
    }
}

/// Convert a resolution into the serialized JSON block (or `None`
/// when no descriptor resolved — `resolved_from` is then still
/// emitted for observability).
pub fn resolution_json(res: &DescriptorResolution) -> Option<serde_json::Value> {
    res.descriptor.as_ref().map(|d| {
        serde_json::json!({
            "schema_version": d.schema_version.clone(),
            "resolved_from": res.resolved_from,
            "preset": d.preset,
            "stages": d.stages.iter().map(|s| serde_json::json!({
                "id": s.id,
                "label": s.label,
                "topics": s.topics,
            })).collect::<Vec<_>>(),
            "progress": { "denominator": d.progress.denominator.clone() },
            "artifacts": d.artifacts.iter().map(|a| serde_json::json!({
                "id": a.id,
                "path_glob": a.path_glob,
                "kind": a.kind,
            })).collect::<Vec<_>>(),
            "dag": {
                "enabled": d.dag.enabled,
                "unit_topic_prefix": d.dag.unit_topic_prefix.clone(),
            },
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_descriptor_resolves_for_pipeline() {
        let res = resolve(Some("ce-executor-pipeline"), Path::new("/nonexistent"));
        assert_eq!(res.resolved_from, "builtin");
        let d = res.descriptor.expect("descriptor");
        assert_eq!(d.preset, "ce-executor-pipeline");
        assert_eq!(d.stages.len(), 8);
        assert_eq!(d.stages[0].id, "plan");
        assert!(!d.dag.enabled);
        assert_eq!(d.progress.denominator, "stages");
    }

    #[test]
    fn builtin_descriptor_parallel_forge_has_dag_defaults() {
        let res = resolve(Some("builtin:parallel-forge"), Path::new("/nonexistent"));
        let d = res.descriptor.expect("descriptor");
        assert!(d.dag.enabled);
        assert_eq!(d.dag.unit_topic_prefix, "forge.unit.");
    }

    #[test]
    fn unknown_preset_resolves_none() {
        let res = resolve(Some("unknown-x"), Path::new("/nonexistent"));
        assert_eq!(res.resolved_from, "none");
        assert!(res.descriptor.is_none());
    }

    #[test]
    fn no_preset_resolves_none() {
        let res = resolve(None, Path::new("/nonexistent"));
        assert_eq!(res.resolved_from, "none");
    }

    #[test]
    fn workspace_custom_descriptor_resolves() {
        let ws = tempfile::tempdir().unwrap();
        let dir = ws.path().join(".ralph/monitor");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("custom.yml"),
            "schema_version: monitor-descriptor/v1\npreset: custom\nstages:\n  - id: a\n    label: A\n    topics: [t1]\nprogress: {denominator: stages}\nartifacts: []\ndag: {enabled: false}\n",
        )
        .unwrap();
        let res = resolve(Some("custom"), ws.path());
        assert_eq!(res.resolved_from, "workspace");
        assert_eq!(res.descriptor.unwrap().stages[0].id, "a");
    }

    /// Plan 2026-10-30-001 U1: `ce-executor-pipeline-loop` builtin removal.
    /// 退役 preset 不得再出现在 builtin descriptor 表里,且 `inspect run`
    /// 的 descriptor 查找必须回退(不 panic、不静默命中其它 builtin)。
    #[test]
    fn removed_pipeline_loop_preset_has_no_builtin_descriptor() {
        assert!(
            !BUILTIN_DESCRIPTORS
                .iter()
                .any(|(n, _)| *n == "ce-executor-pipeline-loop"),
            "Plan 2026-10-30-001 U1: retired preset must NOT keep a builtin descriptor entry"
        );
        let res = resolve(
            Some("builtin:ce-executor-pipeline-loop"),
            Path::new("/nonexistent"),
        );
        assert_eq!(res.resolved_from, "none");
        assert!(res.descriptor.is_none());
        assert!(res.diagnostic.is_none());
    }

    #[test]
    fn unsupported_schema_version_diagnoses() {
        let ws = tempfile::tempdir().unwrap();
        let dir = ws.path().join(".ralph/monitor");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("v2.yml"),
            "schema_version: monitor-descriptor/v2\npreset: v2\nprogress: {denominator: stages}\ndag: {enabled: false}\n",
        )
        .unwrap();
        let res = resolve(Some("v2"), ws.path());
        assert_eq!(res.resolved_from, "unsupported_version");
        assert!(res.diagnostic.unwrap().contains("v2"));
    }

    #[test]
    fn bad_yaml_is_invalid() {
        let ws = tempfile::tempdir().unwrap();
        let dir = ws.path().join(".ralph/monitor");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("broken.yml"), "not: [valid: yaml: [[[").unwrap();
        let res = resolve(Some("broken"), ws.path());
        assert_eq!(res.resolved_from, "invalid");
        assert!(res.diagnostic.is_some());
    }

    #[test]
    fn absolute_path_glob_rejected() {
        let text = "schema_version: monitor-descriptor/v1\npreset: x\nstages: []\nprogress: {denominator: stages}\nartifacts:\n  - id: a\n    path_glob: /etc/passwd\n    kind: file\ndag: {enabled: false}\n";
        assert!(parse_descriptor(text).unwrap_err().contains("path_glob"));
        let dotdot = text.replace("/etc/passwd", "../secret");
        assert!(parse_descriptor(&dotdot).unwrap_err().contains("path_glob"));
    }

    #[test]
    fn traversal_stem_does_not_escape() {
        // `..` stems are rejected before filesystem lookup.
        let res = resolve(Some("../etc/passwd"), Path::new("/nonexistent"));
        assert_eq!(res.resolved_from, "none");
    }

    #[test]
    fn descriptor_topics_subset_of_preset_publishes() {
        // Self-check (plan §12): every sidecar topic must appear in
        // the corresponding en preset's hat publishes. Runtime-
        // emitted entry/terminal topics (injected by
        // `HatRegistry::from_runtime_config` and the loop runner)
        // are also allowed: LOOP_COMPLETE/work.start/loop.cancel/
        // forge.start.
        let pairs = [
            (
                "ce-executor-pipeline",
                "../../presets/en/ce-executor-pipeline.yml",
            ),
            ("parallel-forge", "../../presets/en/parallel-forge.yml"),
            ("autoresearch", "../../presets/en/autoresearch.yml"),
        ];
        for (stem, preset_rel) in pairs {
            let res = resolve(Some(stem), Path::new("/nonexistent"));
            let d = res
                .descriptor
                .unwrap_or_else(|| panic!("descriptor {stem}"));
            let preset_path = Path::new(env!("CARGO_MANIFEST_DIR")).join(preset_rel);
            let preset: serde_yaml::Value = serde_yaml::from_str(
                &std::fs::read_to_string(&preset_path)
                    .unwrap_or_else(|e| panic!("read {}: {e}", preset_path.display())),
            )
            .unwrap();
            let mut declared: std::collections::BTreeSet<String> =
                ["LOOP_COMPLETE", "work.start", "loop.cancel", "forge.start"]
                    .iter()
                    .map(|s| (*s).to_string())
                    .collect();
            if let Some(hats) = preset.get("hats").and_then(|h| h.as_mapping()) {
                for (_name, hat) in hats {
                    for key in ["publishes", "triggers"] {
                        if let Some(list) = hat.get(key).and_then(|p| p.as_sequence()) {
                            for t in list {
                                if let Some(s) = t.as_str() {
                                    declared.insert(s.to_string());
                                }
                            }
                        }
                    }
                }
            }
            // DAG descriptors additionally tolerate runtime-emitted
            // coordination topics: anything under
            // `dag.unit_topic_prefix` plus the development-completion
            // fence `forge.exec.development.done` (both owned by the
            // DAG runtime, not hats).
            for stage in &d.stages {
                for topic in &stage.topics {
                    let allowed = declared.contains(topic.as_str())
                        || (d.dag.enabled
                            && (topic.starts_with(&d.dag.unit_topic_prefix)
                                || topic == "forge.exec.development.done"));
                    assert!(
                        allowed,
                        "descriptor {stem} stage {} topic `{topic}` not in preset publishes",
                        stage.id
                    );
                }
            }
        }
    }
}
