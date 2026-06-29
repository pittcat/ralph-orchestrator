//! Tests for the `migrate_state` module. The plan
//! pins three scenarios:
//!
//! 1. Roundtrip: a legacy tasks.jsonl migrates to the
//!    new shape on first call.
//! 2. Idempotency: a second call reports
//!    `already_current` for every record.
//! 3. Backwards compatibility: a freshly-written file
//!    (with valid `loop_id`) roundtrips without
//!    rewriting.

use super::*;
use std::io::Write;

fn write_jsonl(path: &std::path::Path, lines: &[&str]) {
    let mut f = std::fs::File::create(path).expect("create");
    for line in lines {
        writeln!(f, "{line}").unwrap();
    }
}

#[test]
fn migrate_roundtrip_assigns_loop_id() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("tasks.jsonl");
    write_jsonl(
        &path,
        &[
            r#"{"task_key":"a","loop_id":null}"#,
            r#"{"task_key":"b","loop_id":""}"#,
        ],
    );
    let report = migrate_tasks_file(&path, "loop-mig-1").expect("migrate");
    assert_eq!(report.processed, 2);
    assert_eq!(report.migrated, 2);
    assert_eq!(report.already_current, 0);
    // Re-read the file: both records must now carry the
    // new `loop_id`.
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains(r#""loop_id":"loop-mig-1""#));
}

#[test]
fn migrate_is_idempotent() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("tasks.jsonl");
    write_jsonl(&path, &[r#"{"task_key":"a","loop_id":null}"#]);
    let first = migrate_tasks_file(&path, "loop-mig-2").unwrap();
    assert_eq!(first.migrated, 1);
    let second = migrate_tasks_file(&path, "loop-mig-2").unwrap();
    assert_eq!(second.already_current, 1);
    assert_eq!(second.migrated, 0);
}

#[test]
fn migrate_preserves_fresh_records_unchanged() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("tasks.jsonl");
    write_jsonl(&path, &[r#"{"task_key":"a","loop_id":"existing"}"#]);
    let report = migrate_tasks_file(&path, "loop-mig-3").unwrap();
    assert_eq!(report.already_current, 1);
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains(r#""loop_id":"existing""#));
    assert!(!content.contains(r#""loop_id":"loop-mig-3""#));
}

#[test]
fn migrate_missing_file_returns_empty_report() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("nonexistent.jsonl");
    let report = migrate_tasks_file(&path, "loop-x").expect("missing file is OK");
    assert_eq!(report, MigrationReport::default());
}
