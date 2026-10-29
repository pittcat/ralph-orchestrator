//! Integration tests for `ralph loops stop|retry|merge --format
//! json` typed receipts (feat-monitor control-delivery plan
//! 2026-10-28-1949, Unit 1 / D3). Human output must stay byte-level
//! unchanged — the JSON receipt is additive.

mod common;

use ralph_core::MergeQueue;
use serde_json::Value;
use std::fs;
use std::process::Output;
use tempfile::TempDir;

fn ralph(temp_path: &std::path::Path, args: &[&str]) -> Output {
    common::ralph_bin()
        .args(args)
        .current_dir(temp_path)
        .output()
        .expect("execute ralph")
}

/// Fake `ralph` on PATH so `spawn_merge_ralph` succeeds without a
/// real merge run.
fn fake_ralph_path_dir(home: &std::path::Path) -> std::path::PathBuf {
    let bin = home.join("fakebin");
    fs::create_dir_all(&bin).unwrap();
    let script = bin.join("ralph");
    fs::write(&script, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut p = fs::metadata(&script).unwrap().permissions();
        p.set_mode(0o755);
        fs::set_permissions(&script, p).unwrap();
    }
    bin
}

fn last_stdout_line_json(out: &Output) -> Value {
    let text = String::from_utf8_lossy(&out.stdout);
    let line = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .expect("non-empty stdout");
    serde_json::from_str(line)
        .unwrap_or_else(|_| panic!("last line is JSON: {line:?} (full: {text:?})"))
}

/// stop: human output keeps the legacy text; --format json appends
/// a typed receipt.
#[test]
fn test_stop_json_receipt_and_human_unchanged() {
    let temp = TempDir::new().unwrap();

    // A live "loop" = a long-running child whose pid sits in the
    // primary loop.lock.
    let mut child = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .unwrap();
    fs::create_dir_all(temp.path().join(".ralph")).unwrap();
    fs::write(
        temp.path().join(".ralph/loop.lock"),
        format!(
            "{{\"pid\":{},\"prompt\":\"p\",\"started\":\"2026-10-29T00:00:00Z\"}}",
            child.id()
        ),
    )
    .unwrap();

    // Human: text output only, no JSON trailing line.
    let human = ralph(temp.path(), &["loops", "stop"]);
    assert!(
        human.status.success(),
        "{}",
        String::from_utf8_lossy(&human.stderr)
    );
    let human_out = String::from_utf8_lossy(&human.stdout);
    assert!(human_out.contains("Stop requested"), "{human_out}");
    assert!(!human_out.trim().starts_with('{'));

    // Recreate lock (stop wrote stop-requested; that's fine) then
    // json receipt — the loop is still alive.
    let json = ralph(temp.path(), &["loops", "stop", "--format", "json"]);
    assert!(
        json.status.success(),
        "{}",
        String::from_utf8_lossy(&json.stderr)
    );
    let r = last_stdout_line_json(&json);
    assert_eq!(r["ok"], true);
    assert_eq!(r["action"], "stop");
    assert!(r["loop_id"].is_string());
    assert_eq!(r["detail"], "stop-requested written");
    assert!(r["revision_after"].as_str().unwrap().len() == 64);

    let _ = child.kill();
    let _ = child.wait();
}

/// retry: needs_review queue entry + human caller + stub `ralph` on
/// PATH → receipt with action=retry.
#[test]
fn test_retry_json_receipt() {
    let temp = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    fs::create_dir_all(temp.path().join(".ralph")).unwrap();

    let queue = MergeQueue::new(temp.path());
    queue.enqueue("loop-rev-1", "prompt").unwrap();
    queue.mark_merging("loop-rev-1", 4242).unwrap();
    queue.mark_needs_review("loop-rev-1", "conflict").unwrap();

    let bin = fake_ralph_path_dir(home.path());
    let out = common::ralph_bin()
        .args(["loops", "retry", "loop-rev-1", "--format", "json"])
        .current_dir(temp.path())
        .env("HOME", home.path())
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r = last_stdout_line_json(&out);
    assert_eq!(r["ok"], true);
    assert_eq!(r["action"], "retry");
    assert_eq!(r["loop_id"], "loop-rev-1");
}

/// merge: queued entry + stub `ralph` on PATH → receipt action=merge.
#[test]
fn test_merge_json_receipt() {
    let temp = TempDir::new().unwrap();
    let home = TempDir::new().unwrap();
    fs::create_dir_all(temp.path().join(".ralph")).unwrap();

    let queue = MergeQueue::new(temp.path());
    queue.enqueue("loop-mg-1", "prompt").unwrap();

    let bin = fake_ralph_path_dir(home.path());
    let out = common::ralph_bin()
        .args(["loops", "merge", "loop-mg-1", "--format", "json"])
        .current_dir(temp.path())
        .env("HOME", home.path())
        .env(
            "PATH",
            format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
        )
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let r = last_stdout_line_json(&out);
    assert_eq!(r["ok"], true);
    assert_eq!(r["action"], "merge");
    assert_eq!(r["loop_id"], "loop-mg-1");
}
