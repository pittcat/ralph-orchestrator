use super::relocate_legacy_tasks;
use std::fs;
use tempfile::TempDir;

fn write_lines(dir: &TempDir, lines: &[&str]) -> std::path::PathBuf {
    let path = dir.path().join("tasks.jsonl");
    let content = lines.join("\n");
    fs::write(&path, content).unwrap();
    path
}

fn read_jsonl_lines(path: &std::path::Path) -> Vec<serde_json::Value> {
    let content = fs::read_to_string(path).unwrap();
    content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn legacy_task_relocate_backfills_missing_loop_id() {
    let dir = TempDir::new().unwrap();
    let path = write_lines(
        &dir,
        &[
            r#"{"task_id":"a","loop_id":null,"status":"open"}"#,
            r#"{"task_id":"b","status":"open"}"#,
            r#"{"task_id":"c","loop_id":"loop-existing","status":"closed"}"#,
        ],
    );

    let n = relocate_legacy_tasks(&path, "loop-current").unwrap();
    assert_eq!(n, 2);

    let lines = read_jsonl_lines(&path);
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["loop_id"], "loop-current");
    assert_eq!(lines[1]["loop_id"], "loop-current");
    assert_eq!(lines[2]["loop_id"], "loop-existing");
}

#[test]
fn legacy_task_relocate_treats_empty_string_as_missing() {
    let dir = TempDir::new().unwrap();
    let path = write_lines(
        &dir,
        &[
            r#"{"task_id":"a","loop_id":"","status":"open"}"#,
            r#"{"task_id":"b","loop_id":"   ","status":"open"}"#,
        ],
    );

    let n = relocate_legacy_tasks(&path, "loop-x").unwrap();
    // Empty string is treated as missing; whitespace-only is
    // currently treated as "present" because we only check
    // `is_empty`. This documents the current behaviour so a
    // future change to also trim is intentional.
    assert_eq!(n, 1);

    let lines = read_jsonl_lines(&path);
    assert_eq!(lines[0]["loop_id"], "loop-x");
    // whitespace-only is preserved
    assert_eq!(lines[1]["loop_id"], "   ");
}

#[test]
fn legacy_task_relocate_returns_zero_when_nothing_to_do() {
    let dir = TempDir::new().unwrap();
    let original = r#"{"task_id":"a","loop_id":"loop-existing"}"#;
    let path = write_lines(&dir, &[original]);

    let n = relocate_legacy_tasks(&path, "loop-current").unwrap();
    assert_eq!(n, 0);

    // File is byte-for-byte unchanged — no rewrite happens when
    // backfilled == 0.
    let content = fs::read_to_string(&path).unwrap();
    assert_eq!(content, original);
}

#[test]
fn legacy_task_relocate_handles_empty_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("tasks.jsonl");
    fs::write(&path, "").unwrap();

    let n = relocate_legacy_tasks(&path, "loop-current").unwrap();
    assert_eq!(n, 0);
    let content = fs::read_to_string(&path).unwrap();
    assert_eq!(content, "");
}

#[test]
fn legacy_task_relocate_errors_on_missing_file() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("does-not-exist.jsonl");

    let result = relocate_legacy_tasks(&path, "loop-current");
    assert!(result.is_err());
}

#[test]
fn legacy_task_relocate_is_idempotent() {
    let dir = TempDir::new().unwrap();
    let path = write_lines(
        &dir,
        &[
            r#"{"task_id":"a","loop_id":null}"#,
            r#"{"task_id":"b","status":"open"}"#,
        ],
    );

    let first = relocate_legacy_tasks(&path, "loop-current").unwrap();
    assert_eq!(first, 2);
    let after_first = fs::read_to_string(&path).unwrap();

    let second = relocate_legacy_tasks(&path, "loop-current").unwrap();
    assert_eq!(second, 0);
    let after_second = fs::read_to_string(&path).unwrap();
    assert_eq!(after_first, after_second);
}

#[test]
fn legacy_task_relocate_errors_on_malformed_json() {
    let dir = TempDir::new().unwrap();
    let path = write_lines(
        &dir,
        &[
            r#"{"task_id":"a","loop_id":null}"#,
            r#"not-valid-json"#,
        ],
    );

    let result = relocate_legacy_tasks(&path, "loop-current");
    assert!(matches!(
        result,
        Err(super::RelocateError::MalformedJson { .. })
    ));
}