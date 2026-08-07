#![cfg(test)]

// ─────────────────────────────────────────────────────────────────────────
// U7 (2026-07-04-003 plan): `load_coordinator_hats` typed error tests.
//
// Each test feeds a temporary workspace ralph.yml and asserts the
// typed `CoordinatorHatsError` variant. The tests do NOT touch the
// global `execute()` path — that integration is Unit 2.
// ─────────────────────────────────────────────────────────────────────────

use super::CoordinatorHatsError;
use super::load_coordinator_hats;

use tempfile::TempDir;

#[test]
fn test_missing_ralph_yml_returns_missing_ralph_yml() {
    let temp_dir = TempDir::new().expect("temp dir");
    let err = load_coordinator_hats(temp_dir.path(), &[])
        .expect_err("empty workspace must surface MissingRalphYml");
    assert_eq!(err, CoordinatorHatsError::MissingRalphYml);
}

#[test]
fn test_invalid_yaml_returns_invalid_yaml_variant() {
    let temp_dir = tempfile::TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    std::fs::write(root.join("ralph.yml"), "tasks: [").expect("write broken yaml");
    let err = load_coordinator_hats(root, &[]).expect_err("broken yaml must surface InvalidYaml");
    match err {
        CoordinatorHatsError::InvalidYaml { path, source } => {
            assert_eq!(path, root.join("ralph.yml"));
            assert!(
                !source.is_empty(),
                "InvalidYaml must carry the parse error text"
            );
        }
        other => panic!("expected InvalidYaml, got {other:?}"),
    }
}

#[test]
fn test_missing_tasks_section_returns_missing_tasks_section() {
    let temp_dir = tempfile::TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    std::fs::write(
        root.join("ralph.yml"),
        "event_loop:\n  execution_mode: isolated\n",
    )
    .expect("write ralph.yml");
    let err = load_coordinator_hats(root, &[])
        .expect_err("ralph.yml without tasks: must surface MissingTasksSection");
    assert_eq!(err, CoordinatorHatsError::MissingTasksSection);
}

#[test]
fn test_missing_coordinator_hats_key_returns_missing_key() {
    let temp_dir = tempfile::TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    std::fs::write(root.join("ralph.yml"), "tasks:\n  enabled: true\n").expect("write yaml");
    let err = load_coordinator_hats(root, &[])
        .expect_err("tasks without coordinator_hats must surface MissingCoordinatorHatsKey");
    assert_eq!(err, CoordinatorHatsError::MissingCoordinatorHatsKey);
}

#[test]
fn test_empty_coordinator_hats_returns_empty_variant() {
    let temp_dir = tempfile::TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    std::fs::write(
        root.join("ralph.yml"),
        "tasks:\n  enabled: true\n  coordinator_hats: []\n",
    )
    .expect("write yaml");
    let err = load_coordinator_hats(root, &[])
        .expect_err("coordinator_hats: [] must surface CoordinatorHatsEmpty");
    assert_eq!(err, CoordinatorHatsError::CoordinatorHatsEmpty);
}

#[test]
fn test_valid_yaml_returns_hats_vec() {
    let temp_dir = tempfile::TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    std::fs::write(
        root.join("ralph.yml"),
        "tasks:\n  enabled: true\n  coordinator_hats:\n    - coordinator\n    - executor\n",
    )
    .expect("write yaml");
    let hats = load_coordinator_hats(root, &[]).expect("valid yaml must parse");
    assert_eq!(
        hats,
        vec!["coordinator".to_string(), "executor".to_string()]
    );
}

#[test]
fn test_load_coordinator_hats_falls_back_to_ralph_yaml_extension() {
    let temp_dir = tempfile::TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    // No ralph.yml, only ralph.yaml — should still load.
    std::fs::write(
        root.join("ralph.yaml"),
        "tasks:\n  coordinator_hats: [only]\n",
    )
    .expect("write yaml");
    let hats = load_coordinator_hats(root, &[]).expect("ralph.yaml fallback must work");
    assert_eq!(hats, vec!["only".to_string()]);
}

#[test]
fn test_invalid_yaml_error_message_mentions_path() {
    let temp_dir = tempfile::TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    std::fs::write(root.join("ralph.yml"), ":\n - broken").expect("write yaml");
    let err = load_coordinator_hats(root, &[]).expect_err("must error");
    match err {
        CoordinatorHatsError::InvalidYaml { path, .. } => {
            assert_eq!(path, root.join("ralph.yml"));
        }
        other => panic!("expected InvalidYaml, got {other:?}"),
    }
}
