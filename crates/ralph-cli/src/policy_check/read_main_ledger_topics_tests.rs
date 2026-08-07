use super::*;
use std::io::Write;
use tempfile::TempDir;

fn write_events(dir: &TempDir, lines: &[&str]) -> std::path::PathBuf {
    let path = dir.path().join(".ralph").join("events.jsonl");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut f = std::fs::File::create(&path).unwrap();
    for line in lines {
        writeln!(f, "{line}").unwrap();
    }
    path
}

#[test]
fn drops_system_injected_fallback_topics() {
    // Plan 2026-07-31-001 regression: a runtime-injected
    // `scope.blocked` fallback (system_injected=true) must NOT
    // be replayed as an accepted topic. Otherwise
    // `recover_current_plan_step` advances the recovered step
    // to `finalize` and the next loop's first `scope.ready`
    // emit is rejected with `flow_unknown_emit`.
    let dir = TempDir::new().unwrap();
    let path = write_events(
        &dir,
        &[
            r#"{"topic":"review.start"}"#,
            r#"{"topic":"scope.blocked","system_injected":true}"#,
            r#"{"topic":"LOOP_COMPLETE"}"#,
        ],
    );
    let topics = read_main_ledger_topics(dir.path(), Some(path.as_path()));
    assert_eq!(
        topics,
        vec!["review.start".to_string(), "LOOP_COMPLETE".to_string()],
        "system_injected=true entries must be filtered out"
    );
}

#[test]
fn keeps_normal_topics_and_absent_flag() {
    let dir = TempDir::new().unwrap();
    let path = write_events(
        &dir,
        &[
            r#"{"topic":"review.start"}"#,
            r#"{"topic":"scope.ready","system_injected":false}"#,
        ],
    );
    let topics = read_main_ledger_topics(dir.path(), Some(path.as_path()));
    assert_eq!(
        topics,
        vec!["review.start".to_string(), "scope.ready".to_string()],
        "system_injected=false / absent must be retained"
    );
}
