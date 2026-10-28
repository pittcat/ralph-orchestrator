//! Cross-mode acceptance for run analysis (plan 2026-10-29-0104 U10).
//!
//! Spawns the compiled `ralph` binary against a hand-built
//! `.ralph/diagnostics/<session>/` tree and verifies the deterministic
//! performance path end-to-end: JSON/HTML/markdown output, coverage
//! honesty (`not_collected`, censored spans stay visible), and offline
//! self-containment of the HTML report.

mod common;

use std::fs;
use std::path::Path;

use tempfile::TempDir;

/// Minimal valid manifest (schema `run-diagnosis-input/v2`) with a
/// timing_capture field the analysis surfaces as coverage.
fn write_manifest(dir: &Path, timing_capture: &str) {
    let manifest = format!(
        r#"{{"schema_version":"run-diagnosis-input/v2","manifest_status":"finalized",
"run":{{}},"code_baseline":{{}},"execution_capabilities":[],"artifacts":[],
"boundary_coverage":[],"created_at":"2026-10-29T01:00:00Z","updated_at":"2026-10-29T01:05:00Z",
"timing_capture":"{timing_capture}"}}"#
    );
    fs::write(dir.join("diagnosis-input.json"), manifest).unwrap();
}

fn span_row(span_id: &str, status: &str, extra: &str) -> String {
    // One JSON object per line — timing.jsonl is line-delimited.
    format!(
        r#"{{"schema_version":"run-analysis-timing/v1","span_id":"{span_id}","identity":{{"loop_id":"l1","hat":"executor","activation_id":"l1:executor:1"}},"kind":"agent_exec","status":"{status}","ts_start_utc":"2026-10-29T01:00:00Z","clock_epoch":"ep-1","precision":"exact"{extra}}}"#
    )
}

fn write_timing(dir: &Path) {
    let mut rows = String::new();
    // sp-1: open + completed, 8s monotonic + 8s UTC.
    rows.push_str(&span_row(
        "sp-1",
        "open",
        r#","mono_start_ms":0,"sequence":0"#,
    ));
    rows.push('\n');
    rows.push_str(&span_row(
        "sp-1",
        "completed",
        r#","ts_end_utc":"2026-10-29T01:00:08Z","mono_start_ms":0,"mono_end_ms":8000,"sequence":1"#,
    ));
    rows.push('\n');
    // sp-2: censored — open row only, no close.
    rows.push_str(&span_row(
        "sp-2",
        "open",
        r#","mono_start_ms":100,"sequence":2"#,
    ));
    rows.push('\n');
    fs::write(dir.join("timing.jsonl"), rows).unwrap();
}

fn session(tmp: &TempDir, name: &str) -> std::path::PathBuf {
    let dir = tmp.path().join(".ralph").join("diagnostics").join(name);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn diagnose(tmp: &TempDir, args: &[&str]) -> std::process::Output {
    common::ralph_bin()
        .arg("diagnose")
        .arg("--diagnostics-root")
        .arg(tmp.path().join(".ralph/diagnostics"))
        .args(args)
        .current_dir(tmp.path())
        .output()
        .unwrap()
}

#[test]
fn performance_json_reports_facts_and_censored() {
    let tmp = TempDir::new().unwrap();
    let dir = session(&tmp, "2026-10-29T01-00-00");
    write_manifest(&dir, "collected");
    write_timing(&dir);

    let output = diagnose(&tmp, &["--performance", "--format", "json"]);
    assert!(
        output.status.success(),
        "diagnose --performance failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["schema_version"], "run-analysis-performance/v1");
    assert_eq!(v["capture"]["status"], "collected");
    // sp-1 merged to a completed span with a real duration; sp-2
    // stays censored — never zero-filled.
    assert_eq!(v["capture"]["censored_spans"], 1);
    let spans = v["spans"].as_array().unwrap();
    let sp1 = spans.iter().find(|s| s["span_id"] == "sp-1").unwrap();
    assert_eq!(sp1["duration_ms"], 8000);
    let sp2 = spans.iter().find(|s| s["span_id"] == "sp-2").unwrap();
    assert!(sp2["duration_ms"].is_null());
}

#[test]
fn performance_json_missing_sidecar_stays_not_collected() {
    let tmp = TempDir::new().unwrap();
    let dir = session(&tmp, "2026-10-29T01-00-00");
    write_manifest(&dir, "not_collected");

    let output = diagnose(&tmp, &["--performance", "--format", "json"]);
    assert!(output.status.success());
    let v: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(v["capture"]["status"], "not_collected");
    // A coverage-gap candidate exists — the report surfaces the
    // missing observation instead of guessing.
    let gaps = v["capture"]["gaps"].as_array().unwrap();
    assert!(!gaps.is_empty());
    let bottlenecks = v["bottlenecks"].as_array().unwrap();
    assert!(bottlenecks.iter().any(|b| b["kind"] == "coverage_gap"));
}

#[test]
fn performance_html_is_standalone_and_offline() {
    let tmp = TempDir::new().unwrap();
    let dir = session(&tmp, "2026-10-29T01-00-00");
    write_manifest(&dir, "collected");
    write_timing(&dir);
    let out = tmp.path().join("report.html");

    let output = diagnose(
        &tmp,
        &[
            "--performance",
            "--format",
            "html",
            "--output",
            out.to_str().unwrap(),
        ],
    );
    assert!(
        output.status.success(),
        "html emit failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let html = fs::read_to_string(&out).unwrap();
    assert!(html.contains("<style>"));
    assert!(html.contains("<script>"));
    // Offline: no external resource loads of any kind.
    for needle in ["src=\"http", "href=\"http", "@import", "fetch(", "eval("] {
        assert!(!html.contains(needle), "report must not contain {needle}");
    }
    // Facts rendered, unknown/censored states visible.
    assert!(html.contains("sp-1"));
    assert!(html.contains("censored"));
}

#[test]
fn performance_markdown_lists_bottleneck_evidence_or_gaps() {
    let tmp = TempDir::new().unwrap();
    let dir = session(&tmp, "2026-10-29T01-00-00");
    write_manifest(&dir, "collected");
    write_timing(&dir);

    let output = diagnose(&tmp, &["--performance", "--format", "markdown"]);
    assert!(output.status.success());
    let md = String::from_utf8_lossy(&output.stdout);
    assert!(md.contains("Bottleneck candidates"));
    // A slow exec span without tool observations must surface a gap,
    // never an invented cause.
    assert!(md.contains("gap:") || md.contains("Direction:"));
}

#[test]
fn memory_update_lifecycle_and_prime_query_roundtrip() {
    let tmp = TempDir::new().unwrap();
    // Seed two memories via the real CLI.
    for args in [
        vec!["add", "alpha content about retries", "--tags", "retry"],
        vec!["add", "beta unrelated note"],
    ] {
        let out = common::ralph_bin()
            .args(["tools", "memory"])
            .args(&args)
            .arg("--root")
            .arg(tmp.path())
            .output()
            .unwrap();
        assert!(out.status.success(), "add failed: {:?}", out);
    }
    // Extract the first memory's id from the store file.
    let store = fs::read_to_string(tmp.path().join(".ralph/agent/memories.md")).unwrap();
    let id = store
        .lines()
        .find(|l| l.starts_with("### mem-"))
        .unwrap()
        .trim_start_matches("### ")
        .trim()
        .to_string();

    // `memory update` marks it superseded by the other entry.
    let out = common::ralph_bin()
        .args(["tools", "memory", "update", &id, "--invalidate"])
        .arg("--root")
        .arg(tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "update failed: {:?}", out);

    // The invalidated entry stays readable in the file…
    let store2 = fs::read_to_string(tmp.path().join(".ralph/agent/memories.md")).unwrap();
    assert!(store2.contains("invalidated: true"));
    // …but `prime` must never auto-inject it.
    let out = common::ralph_bin()
        .args(["tools", "memory", "prime", "--query", "retry", "--format", "json"])
        .arg("--root")
        .arg(tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let arr = v.as_array().unwrap();
    assert!(
        arr.iter().all(|m| m["id"] != serde_json::json!(id)),
        "invalidated memory must not be primed"
    );

    // Receipt records the exclusion without leaking content.
    let receipt = tmp.path().join("receipt.json");
    let out = common::ralph_bin()
        .args([
            "tools", "memory", "prime", "--query", "retry", "--receipt",
        ])
        .arg(&receipt)
        .arg("--root")
        .arg(tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success(), "prime --receipt failed: {:?}", out);
    let r: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&receipt).unwrap()).unwrap();
    assert_eq!(r["schema_version"], "memory-selection-receipt/v1");
    assert!(!fs::read_to_string(&receipt)
        .unwrap()
        .contains("alpha content about retries"));
}
