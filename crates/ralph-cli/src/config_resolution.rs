use anyhow::{Context, Result};
use ralph_core::RalphConfig;
use serde_yaml::Value;
use std::path::{Path, PathBuf};

use crate::ConfigSource;

pub(crate) fn default_user_config_path() -> Option<PathBuf> {
    user_config_path_from_home(home_dir_from_env().as_deref())
}

pub(crate) fn user_config_label_if_exists() -> Option<String> {
    let path = default_user_config_path()?;
    path.exists().then(|| path.display().to_string())
}

pub(crate) fn load_optional_user_config_value() -> Result<Option<(Value, String)>> {
    let path = default_user_config_path();
    load_optional_user_config_value_from(path.as_deref())
}

pub(crate) fn load_optional_user_config_value_from(
    path: Option<&Path>,
) -> Result<Option<(Value, String)>> {
    let Some(path) = path else {
        return Ok(None);
    };

    if !path.exists() {
        return Ok(None);
    }

    let label = path.display().to_string();
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to load config from {}", label))?;
    let value = parse_yaml_value(&content, &label)?;
    Ok(Some((value, label)))
}

pub(crate) fn parse_yaml_value(content: &str, label: &str) -> Result<Value> {
    serde_yaml::from_str(content).with_context(|| format!("Failed to parse YAML from {}", label))
}

pub(crate) fn default_core_value() -> Result<Value> {
    let mut value = serde_yaml::to_value(RalphConfig::default())
        .context("Failed to build default core config")?;

    if let Some(mapping) = value.as_mapping_mut() {
        let hats_key = Value::String("hats".to_string());
        let events_key = Value::String("events".to_string());
        mapping.remove(&hats_key);
        mapping.remove(&events_key);

        // 2026-06-20 fix: strip opt-in key placeholders under
        // `event_loop` so `merge_hats_overlay` can detect "operator
        // omitted" via `contains_key`. Without this, the default
        // placeholders (e.g. `state_projection: {enabled: false,
        // actions: {}}`, `suppress_human_guidance: false`) survive
        // `default_core_value()` and make the `!contains_key` guard
        // in `merge_hats_overlay` always true on those keys, silently
        // dropping the preset opt-in (perky-maple + bold-heron
        // regressions). Operator's explicit declaration still wins
        // because `merge_yaml_values` keeps the operator's value
        // when declared; only the default placeholder is stripped.
        const PRESET_OPT_IN_KEYS: &[&str] = &[
            "state_projection",
            "workflow_contract",
            "ephemeral_isolation",
            "enforce_current_unit",
            // 2026-06-24 plan U2: max_residuals is u32-typed
            // (default 8). Strip the operator-default placeholder so
            // the preset opt-in (8 for ce-executor-serial) survives
            // `merge_hats_overlay`.
            "max_residuals",
            // 2026-07-03-001 plan U1: supervisor is opt-in.
            // The framework default is fully populated
            // (`enabled: false`, db_path=".ralph/supervisor.db",
            // ...), so without this strip the
            // `merge_hats_overlay()` `!contains_key` guard
            // always sees the key as present and silently keeps
            // the framework default `enabled: false`, blocking
            // the preset opt-in (e.g. ce-executor-supervisor's
            // `supervisor.enabled: true`).
            "supervisor",
        ];
        if let Some(event_loop) = mapping
            .get_mut(&Value::String("event_loop".to_string()))
            .and_then(|v| v.as_mapping_mut())
        {
            for key in PRESET_OPT_IN_KEYS {
                event_loop.remove(&Value::String((*key).to_string()));
            }
        }

        // 2026-06-24: also strip
        // `telemetry.runtime_diagnosis.drift.coord_join_mode`. Unlike
        // the event_loop-level keys above (which are Option-typed and
        // serialise to Value::Null), `coord_join_mode` is concrete-typed
        // (default = CoordJoinMode::Parallel) so it serialises to a
        // real value, and the `!contains_key` guard in
        // `merge_hats_overlay` always sees the key as present and
        // silently swallows the preset's `coord_join_mode: serial`
        // opt-in. KTD-Drift e2e guard
        // `merge_hats_overlay_preserves_coord_join_mode_via_default_core_value`
        // pins this contract.
        // 2026-07-02: strip `tasks.enabled` so hat-only presets can opt
        // out of the runtime task system via `tasks.enabled: false`
        // without tripping `lint.preset.coordinator_missing` when the
        // operator ralph.yml omits the `tasks` subtree.
        if let Some(tasks) = mapping
            .get_mut(&Value::String("tasks".to_string()))
            .and_then(|v| v.as_mapping_mut())
        {
            tasks.remove(&Value::String("enabled".to_string()));
        }

        if let Some(telemetry) = mapping
            .get_mut(&Value::String("telemetry".to_string()))
            .and_then(|v| v.as_mapping_mut())
        {
            if let Some(runtime_diagnosis) = telemetry
                .get_mut(&Value::String("runtime_diagnosis".to_string()))
                .and_then(|v| v.as_mapping_mut())
            {
                if let Some(drift) = runtime_diagnosis
                    .get_mut(&Value::String("drift".to_string()))
                    .and_then(|v| v.as_mapping_mut())
                {
                    drift.remove(&Value::String("coord_join_mode".to_string()));
                }
            }
        }
    }

    Ok(value)
}

pub(crate) fn merge_yaml_values(base: Value, overlay: Value) -> Result<Value> {
    match (base, overlay) {
        (Value::Mapping(mut base_map), Value::Mapping(overlay_map)) => {
            for (key, overlay_value) in overlay_map {
                let merged_value = if let Some(base_value) = base_map.remove(&key) {
                    merge_yaml_values(base_value, overlay_value)?
                } else {
                    overlay_value
                };
                base_map.insert(key, merged_value);
            }
            Ok(Value::Mapping(base_map))
        }
        (_, overlay) => Ok(overlay),
    }
}

pub(crate) fn compose_core_label(
    user_label: Option<&str>,
    primary_label: &str,
    primary_uses_defaults: bool,
) -> String {
    match user_label {
        Some(user) if primary_uses_defaults => format!("{user} + defaults"),
        Some(user) => format!("{user} + {primary_label}"),
        None => primary_label.to_string(),
    }
}

pub(crate) fn split_config_sources(
    config_sources: &[ConfigSource],
) -> (Vec<ConfigSource>, Vec<ConfigSource>) {
    config_sources
        .iter()
        .cloned()
        .partition(|source| !matches!(source, ConfigSource::Override { .. }))
}

pub(crate) fn find_workspace_config_path(root: &Path) -> Option<PathBuf> {
    ["ralph.yml", "ralph.yaml"]
        .iter()
        .map(|candidate| root.join(candidate))
        .find(|path| path.exists())
}

fn user_config_path_from_home(home: Option<&Path>) -> Option<PathBuf> {
    Some(home?.join(".ralph").join("config.yml"))
}

fn home_dir_from_env() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
        .or_else(|| {
            let drive = std::env::var_os("HOMEDRIVE")?;
            let path = std::env::var_os("HOMEPATH")?;
            let mut joined = PathBuf::from(drive);
            joined.push(path);
            Some(joined)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml::Value;

    #[test]
    fn user_config_path_uses_ralph_home_convention() {
        let path = user_config_path_from_home(Some(Path::new("/tmp/test-home")))
            .expect("path should exist");
        assert_eq!(path, PathBuf::from("/tmp/test-home/.ralph/config.yml"));
    }

    #[test]
    fn merge_yaml_values_recursively_merges_maps_and_replaces_arrays() {
        let base: Value = serde_yaml::from_str(
            r"
hooks:
  events:
    pre.loop.start:
      - name: user-hook
        command: [./user.sh]
event_loop:
  max_iterations: 10
  tags: [one, two]
",
        )
        .unwrap();
        let overlay: Value = serde_yaml::from_str(
            r"
hooks:
  events:
    pre.loop.start:
      - name: local-hook
        command: [./local.sh]
event_loop:
  completion_promise: LOOP_COMPLETE
  tags: [three]
",
        )
        .unwrap();

        let merged = merge_yaml_values(base, overlay).unwrap();
        let hooks = merged["hooks"]["events"]["pre.loop.start"]
            .as_sequence()
            .expect("hook sequence");
        assert_eq!(hooks.len(), 1);
        assert_eq!(hooks[0]["name"].as_str(), Some("local-hook"));
        assert_eq!(merged["event_loop"]["max_iterations"].as_i64(), Some(10));
        assert_eq!(
            merged["event_loop"]["completion_promise"].as_str(),
            Some("LOOP_COMPLETE")
        );
        let tags = merged["event_loop"]["tags"].as_sequence().expect("tags");
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].as_str(), Some("three"));
    }

    #[test]
    fn compose_core_label_uses_defaults_suffix_only_for_user_only_resolution() {
        assert_eq!(
            compose_core_label(Some("/home/test/.ralph/config.yml"), "ralph.yml", true,),
            "/home/test/.ralph/config.yml + defaults"
        );
        assert_eq!(
            compose_core_label(
                Some("/home/test/.ralph/config.yml"),
                "repo/ralph.yml",
                false,
            ),
            "/home/test/.ralph/config.yml + repo/ralph.yml"
        );
        assert_eq!(compose_core_label(None, "ralph.yml", true), "ralph.yml");
    }

    /// 2026-06-24 fix: `default_core_value()` MUST strip all preset
    /// opt-in keys from the `event_loop` mapping so that
    /// `merge_hats_overlay`'s `!contains_key` guard correctly
    /// detects "operator omitted the key". Without this strip,
    /// `Option<...>`-typed fields default to `Value::Null` in the
    /// serialised core value, the `contains_key` check is always
    /// true, and the preset opt-in is silently dropped
    /// (perky-maple + bold-heron pattern). This test pins the
    /// post-strip contract: the key is absent from the default
    /// core value, and a preset overlay correctly inserts it.
    ///
    /// 2026-06-24 follow-up: also covers the nested
    /// `telemetry.runtime_diagnosis.drift.coord_join_mode` opt-in.
    /// Unlike the event_loop-level keys (which are Option-typed and
    /// serialise to Value::Null), `coord_join_mode` is
    /// concrete-typed (default = CoordJoinMode::Parallel) so it
    /// serialises to a real value; the `!contains_key` guard sees
    /// the key as present and silently swallows the preset's
    /// `coord_join_mode: serial` opt-in. KTD-Drift e2e guard
    /// `merge_hats_overlay_preserves_coord_join_mode_via_default_core_value`
    /// in preflight.rs pins this contract.
    #[test]
    fn default_core_value_strips_preset_opt_in_keys_from_event_loop() {
        let default_value = default_core_value().expect("default must build");

        let event_loop = default_value
            .get("event_loop")
            .and_then(|v| v.as_mapping())
            .expect("event_loop must be a mapping in default core value");

        for key in [
            "state_projection",
            "workflow_contract",
            "ephemeral_isolation",
            "enforce_current_unit",
            // 2026-07-03-001 plan U1: supervisor opt-in. The
            // framework default is fully populated
            // (`enabled: false` + concrete defaults), so the
            // strip is required for the preset opt-in
            // (`supervisor.enabled: true`) to survive operator
            // omission.
            "supervisor",
        ] {
            let key_value = Value::String(key.to_string());
            assert!(
                !event_loop.contains_key(&key_value),
                "default core value must NOT contain event_loop.{} \
                 (it would block preset opt-in via `contains_key` guard \
                 in `merge_hats_overlay`)",
                key
            );
        }

        // The nested `coord_join_mode` opt-in must also be stripped
        // from the default telemetry.runtime_diagnosis.drift mapping.
        // Concrete-typed default values still need to be removed so
        // the preset's serial opt-in can land.
        let coord_join_mode = default_value
            .get("telemetry")
            .and_then(|v| v.get("runtime_diagnosis"))
            .and_then(|v| v.get("drift"))
            .and_then(|v| v.get("coord_join_mode"));
        assert!(
            coord_join_mode.is_none(),
            "default core value must NOT contain \
             telemetry.runtime_diagnosis.drift.coord_join_mode \
             (concrete-typed default CoordJoinMode::Parallel would \
             block preset opt-in via `contains_key` guard in \
             `merge_hats_overlay`); got {:?}",
            coord_join_mode
        );

        let tasks_enabled = default_value.get("tasks").and_then(|v| v.get("enabled"));
        assert!(
            tasks_enabled.is_none(),
            "default core value must NOT contain tasks.enabled \
             (framework default `true` would block preset opt-in via \
             `contains_key` guard in `merge_hats_overlay`); got {:?}",
            tasks_enabled
        );

        // 2026-07-03-001 plan U1: the supervisor opt-in must
        // round-trip through `merge_yaml_values` so a preset
        // declaring `supervisor.enabled: true` reaches the
        // dispatcher branch (U12) even when the operator's
        // ralph.yml omits the `event_loop.supervisor` block.
        // This pins the end-to-end strip + merge contract.
        let preset_value: Value = serde_yaml::from_str(
            r#"
event_loop:
  supervisor:
    enabled: true
    max_concurrent_workers: 16
"#,
        )
        .expect("preset snippet must parse");
        let merged = merge_yaml_values(default_value, preset_value).expect("merge must succeed");
        let enabled = merged
            .get("event_loop")
            .and_then(|v| v.get("supervisor"))
            .and_then(|v| v.get("enabled"))
            .and_then(|v| v.as_bool());
        assert_eq!(
            enabled,
            Some(true),
            "preset supervisor.enabled must survive merge_yaml_values; got {:?}",
            enabled
        );
        let max_workers = merged
            .get("event_loop")
            .and_then(|v| v.get("supervisor"))
            .and_then(|v| v.get("max_concurrent_workers"))
            .and_then(|v| v.as_u64());
        assert_eq!(
            max_workers,
            Some(16),
            "preset supervisor.max_concurrent_workers must reach merged value; got {:?}",
            max_workers
        );
    }
}
