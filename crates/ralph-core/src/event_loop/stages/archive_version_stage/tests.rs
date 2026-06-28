use super::archive_state_for_loop;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

#[test]
fn archive_noop_on_first_run() {
    let dir = TempDir::new().unwrap();
    let archive = archive_state_for_loop(dir.path(), "loop-a").unwrap();
    assert!(archive.is_none());
    assert!(!dir.path().join("loop-version.json").exists());
}

#[test]
fn archive_noop_when_loop_id_unchanged() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("loop-version.json"),
        r#"{"loop_id":"loop-a","version":1}"#,
    )
    .unwrap();
    fs::write(dir.path().join("tasks.jsonl"), "{}\n").unwrap();

    let archive = archive_state_for_loop(dir.path(), "loop-a").unwrap();
    assert!(archive.is_none());
    assert!(dir.path().join("tasks.jsonl").exists());
}

#[test]
fn archive_moves_jsonl_when_loop_id_changes() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("loop-version.json"),
        r#"{"loop_id":"loop-a","version":1}"#,
    )
    .unwrap();
    fs::write(dir.path().join("tasks.jsonl"), "{}\n").unwrap();
    fs::write(dir.path().join("recovery.jsonl"), "{}\n").unwrap();

    let archive = archive_state_for_loop(dir.path(), "loop-b").unwrap();
    let archive_dir = archive.expect("archive dir must be returned");
    assert!(archive_dir.exists());
    assert!(!dir.path().join("tasks.jsonl").exists());
    assert!(!dir.path().join("recovery.jsonl").exists());
    assert!(archive_dir.join("tasks.jsonl").exists());
    assert!(archive_dir.join("recovery.jsonl").exists());
}

#[test]
fn archive_keeps_loop_version_json_in_place() {
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("loop-version.json"),
        r#"{"loop_id":"loop-a","version":1}"#,
    )
    .unwrap();
    fs::write(dir.path().join("tasks.jsonl"), "{}\n").unwrap();

    archive_state_for_loop(dir.path(), "loop-b").unwrap();
    assert!(dir.path().join("loop-version.json").exists());
}

#[test]
fn archive_errors_on_relative_path() {
    let result = archive_state_for_loop(Path::new("relative/.ralph"), "loop-a");
    assert!(result.is_err());
}

#[test]
fn archive_subdirectories_also_moved() {
    // P1-8 (2026-06-27 adversarial review): the
    // archive now walks every JSONL file under
    // the workspace, including files in
    // subdirectories (e.g. `.ralph/agent/`). The
    // archive directory mirrors the source
    // structure so the relative path is
    // preserved.
    let dir = TempDir::new().unwrap();
    fs::write(
        dir.path().join("loop-version.json"),
        r#"{"loop_id":"loop-a","version":1}"#,
    )
    .unwrap();
    fs::create_dir(dir.path().join("subdir")).unwrap();
    fs::write(dir.path().join("subdir/tasks.jsonl"), "{}\n").unwrap();
    fs::write(dir.path().join("tasks.jsonl"), "{}\n").unwrap();

    let archive = archive_state_for_loop(dir.path(), "loop-b")
        .unwrap()
        .expect("archive directory created");
    assert!(!dir.path().join("tasks.jsonl").exists());
    assert!(!dir.path().join("subdir/tasks.jsonl").exists());
    // The archive directory mirrors the source
    // structure: `subdir/tasks.jsonl` is moved
    // into `archive/<id>/subdir/tasks.jsonl`.
    assert!(archive.join("tasks.jsonl").exists());
    assert!(archive.join("subdir").join("tasks.jsonl").exists());
}