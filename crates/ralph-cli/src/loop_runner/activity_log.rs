//! Per-run activity ledger (Plan3-U1): every backend (model CLI)
//! call appends a `start`/`end` line pair to
//! `.ralph/runs/<run_id>.activity.jsonl`.
//!
//! Guarantees:
//! - **Always on** — no `RALPH_DIAGNOSTICS` gate.
//! - **Best-effort** — a write failure logs to stderr/tracing and
//!   never aborts the execution path (same contract as the
//!   notifications sinks, E8).
//! - **Single-line atomicity** — one `write_all` per record under
//!   `OpenOptions::append`, so concurrent writers never interleave
//!   bytes within a line.
//! - **Crash-safe** — fsync per line; a SIGKILL mid-run leaves all
//!   previously written lines consumable.
//! - **Bounded line** — stderr content never lands verbatim; only a
//!   `stderr_tail_hint` classification.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use serde::Serialize;
use tracing::warn;

/// Cap a single record at 4 KiB — fields are small and fixed; the
/// bound is a tripwire, payloads this size never occur.
const MAX_LINE_BYTES: usize = 4096;

#[derive(Debug, Serialize)]
struct ActivityRecord<'a> {
    ts: String,
    seq_in_run: u64,
    hat: &'a str,
    backend: &'a str,
    phase: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timed_out: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stderr_tail_hint: Option<&'a str>,
}

/// Appender for one run's activity ledger. Cheap to clone — seq is
/// shared via the atomic counter.
#[derive(Debug)]
pub struct ActivityLog {
    path: PathBuf,
    seq: AtomicU64,
}

impl ActivityLog {
    /// `.ralph/runs/<run_id>.activity.jsonl` for `run_id`.
    pub fn path_for(ralph_dir: &Path, run_id: &str) -> PathBuf {
        ralph_dir
            .join("runs")
            .join(format!("{run_id}.activity.jsonl"))
    }

    /// Open (create if needed) the ledger for `run_id`. `seq_in_run`
    /// resumes at line_count+1 so a `--continue`/resume appends
    /// without reusing sequence numbers.
    pub fn open(ralph_dir: &Path, run_id: &str) -> Self {
        let path = Self::path_for(ralph_dir, run_id);
        let existing =
            std::fs::read(&path).map(|b| b.iter().filter(|&&c| c == b'\n').count() as u64);
        Self {
            path,
            seq: AtomicU64::new(existing.unwrap_or(0)),
        }
    }

    fn append(&self, rec: &ActivityRecord<'_>) {
        let Ok(mut line) = serde_json::to_string(rec) else {
            return;
        };
        if line.len() + 1 > MAX_LINE_BYTES {
            warn!(target: "ralph_cli::activity_log", "activity record exceeds 4KiB; dropping");
            return;
        }
        line.push('\n');
        let write = || -> std::io::Result<()> {
            let mut f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?;
            f.write_all(line.as_bytes())?;
            f.sync_data()
        };
        if let Err(e) = write() {
            warn!(
                target: "ralph_cli::activity_log",
                path = %self.path.display(),
                error = %e,
                "activity ledger append failed (best-effort; execution continues)"
            );
        }
    }

    pub fn record_start(&self, hat: &str, backend: &str) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        self.append(&ActivityRecord {
            ts: Utc::now().to_rfc3339(),
            seq_in_run: seq,
            hat,
            backend,
            phase: "start",
            exit_code: None,
            timed_out: None,
            is_error: None,
            duration_ms: None,
            stderr_tail_hint: None,
        });
    }

    pub fn record_end(
        &self,
        hat: &str,
        backend: &str,
        duration_ms: u64,
        exit_code: Option<i32>,
        timed_out: bool,
        is_error: bool,
        stderr_tail_hint: Option<&str>,
    ) {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        self.append(&ActivityRecord {
            ts: Utc::now().to_rfc3339(),
            seq_in_run: seq,
            hat,
            backend,
            phase: "end",
            exit_code,
            timed_out: Some(timed_out),
            is_error: Some(is_error),
            duration_ms: Some(duration_ms),
            stderr_tail_hint,
        });
    }
}

/// Classify the tail of stderr/output into a coarse hint. Never
/// returns raw text — privacy + bounded line (plan §5).
/// Pattern list is deliberately small and conservative.
pub fn classify_stderr_tail(output: &str) -> Option<&'static str> {
    // Scan only the tail — a 32KB suffix is plenty for fatal banners.
    let tail_start = output.len().saturating_sub(32 * 1024);
    let tail = &output[tail_start..].to_lowercase();
    for (needle, hint) in [
        ("rate limit", "rate_limited"),
        ("rate_limit", "rate_limited"),
        ("429", "rate_limited"),
        ("unauthorized", "unauthorized"),
        ("401", "unauthorized"),
        ("forbidden", "unauthorized"),
        ("eai_again", "dns"),
        ("enotfound", "dns"),
        ("dns", "dns"),
        ("etimedout", "timeout"),
        ("timed out", "timeout"),
        ("timeout", "timeout"),
    ] {
        if tail.contains(needle) {
            return Some(hint);
        }
    }
    if tail.trim().is_empty() {
        None
    } else {
        Some("unknown_pattern")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn lines(path: &Path) -> Vec<String> {
        std::fs::read_to_string(path)
            .unwrap()
            .lines()
            .map(String::from)
            .collect()
    }

    #[test]
    fn start_and_end_write_parseable_lines_with_increasing_seq() {
        let dir = TempDir::new().unwrap();
        let ralph = dir.path().join(".ralph");
        std::fs::create_dir_all(ralph.join("runs")).unwrap();
        let log = ActivityLog::open(&ralph, "run-1");
        log.record_start("executor", "claude");
        log.record_end("executor", "claude", 1234, Some(0), false, false, None);
        let ls = lines(&ActivityLog::path_for(&ralph, "run-1"));
        assert_eq!(ls.len(), 2);
        let a: serde_json::Value = serde_json::from_str(&ls[0]).unwrap();
        let b: serde_json::Value = serde_json::from_str(&ls[1]).unwrap();
        assert_eq!(a["phase"], "start");
        assert_eq!(a["seq_in_run"], 1);
        assert_eq!(a["hat"], "executor");
        assert_eq!(b["phase"], "end");
        assert_eq!(b["seq_in_run"], 2);
        assert_eq!(b["exit_code"], 0);
        assert_eq!(b["duration_ms"], 1234);
        assert_eq!(b["is_error"], false);
    }

    #[test]
    fn seq_resumes_after_reopen() {
        let dir = TempDir::new().unwrap();
        let ralph = dir.path().join(".ralph");
        std::fs::create_dir_all(ralph.join("runs")).unwrap();
        ActivityLog::open(&ralph, "r").record_start("h", "b");
        ActivityLog::open(&ralph, "r").record_start("h", "b");
        let ls = lines(&ActivityLog::path_for(&ralph, "r"));
        assert_eq!(ls.len(), 2);
        let b: serde_json::Value = serde_json::from_str(&ls[1]).unwrap();
        assert_eq!(b["seq_in_run"], 2);
    }

    #[test]
    fn write_failure_does_not_panic() {
        // Path parent is a *file* → every open fails; must not panic.
        let dir = TempDir::new().unwrap();
        let ralph = dir.path().join("not-a-dir");
        std::fs::write(&ralph, "file").unwrap();
        let log = ActivityLog::open(&ralph, "r");
        log.record_start("h", "b");
        log.record_end("h", "b", 1, Some(1), false, true, Some("timeout"));
    }

    #[test]
    fn concurrent_appends_keep_lines_whole() {
        let dir = TempDir::new().unwrap();
        let ralph = dir.path().join(".ralph");
        std::fs::create_dir_all(ralph.join("runs")).unwrap();
        let mut handles = Vec::new();
        for i in 0..8 {
            let ralph = ralph.clone();
            handles.push(std::thread::spawn(move || {
                // each thread owns a log to stress the atomic-append claim
                let log = ActivityLog::open(&ralph, "r");
                for j in 0..25 {
                    log.record_start(&format!("hat{i}"), &format!("b{j}"));
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let ls = lines(&ActivityLog::path_for(&ralph, "r"));
        assert_eq!(ls.len(), 200);
        for l in ls {
            let v: serde_json::Value = serde_json::from_str(&l).unwrap();
            assert!(v["seq_in_run"].as_u64().unwrap() >= 1);
        }
    }

    #[test]
    fn stderr_hint_classification() {
        assert_eq!(classify_stderr_tail(""), None);
        assert_eq!(
            classify_stderr_tail("boom: 429 too many"),
            Some("rate_limited")
        );
        assert_eq!(
            classify_stderr_tail("Error: rate limit hit"),
            Some("rate_limited")
        );
        assert_eq!(
            classify_stderr_tail("401 Unauthorized"),
            Some("unauthorized")
        );
        assert_eq!(classify_stderr_tail("ENOTFOUND x"), Some("dns"));
        assert_eq!(classify_stderr_tail("weird crash"), Some("unknown_pattern"));
        // only the tail is scanned — a banner pushed out of the
        // 32KB window falls back to the generic hint, not None
        let pad = "x".repeat(40 * 1024);
        assert_eq!(
            classify_stderr_tail(&format!("rate limit {pad}")),
            Some("unknown_pattern")
        );
    }
}
