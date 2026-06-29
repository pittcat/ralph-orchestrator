use super::archive_state_for_loop;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

#[test]
fn archive_noop_on_first_run() {
    let dir = TempDir::new().unwrap();
    let archive = archive_state_for_loop(dir.path(), "loop-a").unwrap();
    // 2026-06-28-002 U4: first run now writes the initial
    // `loop-version.json` marker (no archive directory is
    // created, but the marker file must be on disk). The
    // `archive_noop_on_first_run` semantics shift from "do
    // nothing" to "do nothing besides write the initial marker".
    assert!(archive.is_none());
    assert!(dir.path().join("loop-version.json").exists());
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
#[test]
fn u4_fresh_workspace_writes_initial_loop_version_json() {
    // 2026-06-28-002 U4: fresh workspace (no loop-version.json)
    // must now write the initial version marker so downstream
    // U11/U13 stages can verify archive correctness.
    let dir = TempDir::new().unwrap();
    // Sanity: workspace is empty.
    assert!(!dir.path().join("loop-version.json").exists());

    let result = archive_state_for_loop(dir.path(), "loop-fresh").unwrap();
    // No archive is created on first run, but the marker file
    // must be on disk.
    assert!(result.is_none(), "first run returns no archive directory");
    let version_path = dir.path().join("loop-version.json");
    assert!(
        version_path.exists(),
        "U4: fresh workspace must now write loop-version.json on first run"
    );
    let persisted: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&version_path).unwrap()).unwrap();
    assert_eq!(persisted["loop_id"], "loop-fresh");
    assert_eq!(persisted["version"], 1);
}

#[test]
fn u4_repeat_first_loop_id_does_not_rewrite_version() {
    // Calling archive twice on the same fresh loop_id must NOT
    // overwrite the marker with a higher version — `archive_state_for_loop`
    // is a no-op when the persisted loop_id matches.
    let dir = TempDir::new().unwrap();
    let _ = archive_state_for_loop(dir.path(), "loop-stable").unwrap();
    let first_content = fs::read_to_string(dir.path().join("loop-version.json")).unwrap();
    let _ = archive_state_for_loop(dir.path(), "loop-stable").unwrap();
    let second_content = fs::read_to_string(dir.path().join("loop-version.json")).unwrap();
    assert_eq!(first_content, second_content);
}
