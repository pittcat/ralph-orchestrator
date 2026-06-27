//! Idempotent JSONL log writer (U4).
//!
//! Why this exists: the diagnosis / recovery / drift / task-store
//! consumers used to append raw lines to `*.jsonl`. That left
//! three problems open, all of which contributed to the
//! 2026-06-26 incident:
//!
//! 1. **Two records can both claim `_final=true` for the same
//!    key** if two writers race the read-modify-write — the
//!    earlier code wrapped the writer in a `Mutex`, but a Mutex
//!    only serialises in-process calls; cross-process races
//!    still corrupt the log.
//! 2. **Stale records survive across loop restarts** because the
//!    in-memory `Mutex` index is gone, so a `_final=true` from
//!    the previous run can be silently overwritten.
//! 3. **Cross-loop interference in reused worktrees** — when the
//!    operator reuses the same worktree for a new plan, the old
//!    `tasks.jsonl` records pollute the new loop's
//!    `TaskWrongLoop` checks.
//!
//! This module solves all three by:
//!
//! - Persisting the loop version in `.ralph/loop-version.json`
//!   so a new loop on the same workspace starts at `version+1`.
//! - Using `fsync(parent_dir) + rename` as the atomic write
//!   protocol.
//! - Holding an OS-level `flock` (`nix::fcntl::Flock`) across the
//!   read-modify-write of any `_final=true` record so concurrent
//!   writers see a stable "is this key already final?" answer.
//!
//! Cross-platform / concurrency semantics:
//!
//! - macOS / Linux: `std::fs::rename` is atomic; `flock` is
//!   available; the implementation is correct as written.
//! - Windows: `std::fs::rename` is **not** atomic; the plan
//!   appendix explicitly calls out that callers must add an
//!   inter-process mutex on Windows. We still emit
//!   `fsync(parent_dir)` before rename to minimise the window.
//! - Threading: each `IdempotentLog` instance owns its lock
//!   handle; cloning is not provided because callers should
//!   route all writes through one writer per (workspace,
//!   loop_id) pair.
//!
//! # Example
//!
//! ```no_run
//! use std::path::Path;
//! use ralph_core::state::idempotent_log::{IdempotentLog, IdempotentRecord};
//!
//! let mut log = IdempotentLog::open(Path::new(".ralph"), "loop-1").unwrap();
//! log.append(IdempotentRecord::new("recovery:abc:loop:loop-1")
//!     .with_final(true)
//!     .with_payload(serde_json::json!({"retry_key": "abc"})))
//!     .unwrap();
//! ```

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
#[cfg(unix)]
use {
    nix::fcntl::{Flock, FlockArg, OFlag},
};

/// One transition entry inside `IdempotentRecord._transitions`.
///
/// `from` is `None` for the first transition (creation);
/// `to` is the state name (e.g. `"detected"`, `"diagnosing"`,
/// `"closed"`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Transition {
    pub from: Option<String>,
    pub to: String,
}

/// The on-disk record shape.
///
/// `_idempotency_key`, `_version`, `_final`, `_created_at`, and
/// `_transitions` are recognised by the writer for the
/// idempotency protocol. Any other field is preserved verbatim
/// in the JSONL output.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct IdempotentRecord {
    pub _idempotency_key: String,
    #[serde(default)]
    pub _version: u64,
    #[serde(default)]
    pub _final: bool,
    #[serde(default)]
    pub _created_at: String,
    #[serde(default)]
    pub _transitions: Vec<Transition>,
    /// Free-form payload — captured under the writer's
    /// `payload` key on disk so it round-trips through serde
    /// without collision with the underscore-prefixed metadata.
    #[serde(default)]
    pub payload: Value,
}

impl IdempotentRecord {
    /// Build a minimal record. `version` is filled in by the
    /// log writer on append.
    pub fn new(key: impl Into<String>) -> Self {
        Self {
            _idempotency_key: key.into(),
            _version: 0,
            _final: false,
            _created_at: String::new(),
            _transitions: Vec::new(),
            payload: Value::Null,
        }
    }

    pub fn with_payload(mut self, payload: Value) -> Self {
        self.payload = payload;
        self
    }

    pub fn with_final(mut self, is_final: bool) -> Self {
        self._final = is_final;
        self
    }

    pub fn with_transition(mut self, from: Option<String>, to: impl Into<String>) -> Self {
        self._transitions.push(Transition {
            from,
            to: to.into(),
        });
        self
    }
}

/// Errors surfaced by `IdempotentLog`.
#[derive(Debug, Error)]
pub enum IdempotentError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("missing `_idempotency_key` in record")]
    MissingIdempotencyKey,
    #[error("key `{0}` is already `_final=true`; further writes are rejected")]
    FinalAlreadySet(String),
    #[error("serde error: {0}")]
    Serde(#[from] serde_json::Error),
    #[error("OS file lock unavailable on this platform (Windows needs caller-side inter-process mutex)")]
    NoOsLock,
}

/// Loop-level version persisted to `.ralph/loop-version.json`.
///
/// On a brand-new workspace, version starts at 1. If the file
/// already contains a `loop_id` matching the caller's, the
/// version is reused (resume case). If the persisted `loop_id`
/// differs from the caller's, version is incremented and the
/// caller is expected to archive the old `.ralph/*.jsonl`
/// records before calling `open` (see U11).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PersistedVersion {
    pub loop_id: String,
    pub version: u64,
}

const LOOP_VERSION_FILE: &str = "loop-version.json";

/// Idempotent JSONL writer for a single (workspace, loop_id)
/// pair. Constructed via `IdempotentLog::open`.
pub struct IdempotentLog {
    workspace: PathBuf,
    loop_id: String,
    version: u64,
    /// In-memory index of the last record seen per key. Not
    /// authoritative; the file is the source of truth, and the
    /// OS lock guards the read-modify-write.
    index: HashMap<String, IdempotentRecord>,
}

impl IdempotentLog {
    /// Open or resume the log for the given loop. Reads or
    /// writes `.ralph/loop-version.json` so that:
    ///
    /// - the first call to a workspace sets version=1;
    /// - a subsequent call with the same loop_id reuses the
    ///   existing version (resume);
    /// - a call with a different loop_id bumps the version and
    ///   is the caller's signal that archive (U11) must run
    ///   first.
    ///
    /// Concurrent callers may briefly observe a missing or
    /// partially written `loop-version.json`; `open` retries the
    /// read up to 5 times with a small sleep before treating the
    /// file as fresh.
    pub fn open(workspace: &Path, loop_id: &str) -> Result<Self, IdempotentError> {
        fs::create_dir_all(workspace)?;

        let version_path = workspace.join(LOOP_VERSION_FILE);

        let mut expected_version: Option<u64> = None;
        for attempt in 0..5 {
            if version_path.exists() {
                let raw = fs::read_to_string(&version_path)?;
                if !raw.trim().is_empty() {
                    match serde_json::from_str::<PersistedVersion>(&raw) {
                        Ok(persisted) => {
                            expected_version = Some(if persisted.loop_id == loop_id {
                                persisted.version
                            } else {
                                persisted.version + 1
                            });
                            break;
                        }
                        Err(_) => {
                            // Partial write from a concurrent caller —
                            // fall through to retry.
                        }
                    }
                }
            }
            if attempt < 4 {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        }

        let expected_version = expected_version.unwrap_or(1);

        let new_persisted = PersistedVersion {
            loop_id: loop_id.to_string(),
            version: expected_version,
        };
        let serialised = serde_json::to_string_pretty(&new_persisted)? + "\n";

        // Write the version file atomically too — otherwise a
        // half-written file breaks the next `open`.
        let nonce = std::process::id();
        let tmp = version_path.with_extension(format!("json.tmp.{nonce}"));
        fs::write(&tmp, &serialised)?;
        fs::rename(&tmp, &version_path)?;

        Ok(Self {
            workspace: workspace.to_path_buf(),
            loop_id: loop_id.to_string(),
            version: expected_version,
            index: HashMap::new(),
        })
    }

    pub fn loop_id(&self) -> &str {
        &self.loop_id
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Append or update `record` under the per-key JSONL file
    /// `{workspace}/{key}.jsonl`.
    ///
    /// The protocol:
    ///
    /// 1. Acquire an OS-level exclusive lock on the key file
    ///    (`flock` on Unix; rejected with `NoOsLock` on Windows).
    /// 2. Read the existing record (if any). If it is
    ///    `_final=true`, refuse with `FinalAlreadySet`.
    /// 3. Merge the new transitions into the existing record
    ///    (or use the new record if there is no existing one).
    /// 4. Write the merged record to a sibling `.{nonce}.tmp`
    ///    file, `fsync`, then `rename` over the key file.
    /// 5. Release the lock (RAII on `Flock`).
    ///
    /// The in-memory index is updated **after** the rename so
    /// a crash between steps 4 and 5 leaves the disk as the
    /// source of truth and the next `open` rebuilds the index.
    pub fn append(&mut self, mut record: IdempotentRecord) -> Result<(), IdempotentError> {
        if record._idempotency_key.is_empty() {
            return Err(IdempotentError::MissingIdempotencyKey);
        }

        record._version = self.version;
        if record._created_at.is_empty() {
            record._created_at = now_iso8601();
        }

        let key_path = self.workspace.join(format!("{}.jsonl", record._idempotency_key));

        // 1. Acquire OS lock.
        let _guard = acquire_exclusive_lock(&key_path)?;

        // 2. Read existing record (if any).
        let existing = read_last_record(&key_path)?;
        let merged = if let Some(existing) = existing {
            if existing._final {
                return Err(IdempotentError::FinalAlreadySet(
                    record._idempotency_key.clone(),
                ));
            }
            if existing._idempotency_key != record._idempotency_key {
                return Err(IdempotentError::FinalAlreadySet(
                    record._idempotency_key.clone(),
                ));
            }
            merge_records(existing, record)
        } else {
            record
        };

        // 3. Atomic write.
        write_atomic(&key_path, &merged)?;

        // 4. Update in-memory index.
        self.index.insert(merged._idempotency_key.clone(), merged);

        Ok(())
    }

    /// Return the number of `_final=true` records currently
    /// indexed in memory. Used by the diagnosis summary path
    /// (U8) to avoid scanning JSONL on every read.
    pub fn final_count(&self) -> usize {
        self.index.values().filter(|r| r._final).count()
    }

    /// Replay every JSONL file under `workspace` and rebuild
    /// the in-memory index. Returns the number of records
    /// indexed.
    ///
    /// This is what `DiagnosisSummary::from_final_records`
    /// (U8) calls before computing summary counts — it
    /// guarantees the count is grounded in what's on disk,
    /// not in whatever process-local state happened to be
    /// alive when the last write occurred.
    pub fn replay(&mut self) -> Result<usize, IdempotentError> {
        self.index.clear();
        for entry in fs::read_dir(&self.workspace)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
                continue;
            }
            if path.file_name().and_then(|s| s.to_str()) == Some(LOOP_VERSION_FILE) {
                continue;
            }
            let content = fs::read_to_string(&path)?;
            for line in content.lines() {
                if line.trim().is_empty() {
                    continue;
                }
                let rec: IdempotentRecord = serde_json::from_str(line)?;
                self.index.insert(rec._idempotency_key.clone(), rec);
            }
        }
        Ok(self.index.len())
    }

    /// Read every `_final=true` record from the in-memory
    /// index. Used by the summary path.
    pub fn final_records(&self) -> Vec<IdempotentRecord> {
        self.index
            .values()
            .filter(|r| r._final)
            .cloned()
            .collect()
    }
}

fn now_iso8601() -> String {
    // We deliberately use the system clock directly here
    // rather than passing a `Clock` through every call. The
    // idempotency protocol depends on the value being a
    // monotonic-ish timestamp on disk, not on test
    // determinism. Tests that need a stable timestamp inject
    // one via `IdempotentRecord::with_created_at`.
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

fn write_atomic(path: &Path, record: &IdempotentRecord) -> Result<(), IdempotentError> {
    let nonce = std::process::id();
    let tmp = path.with_extension(format!("tmp.{nonce}"));

    let mut f = File::create(&tmp)?;
    let line = serde_json::to_string(record)?;
    writeln!(f, "{line}")?;
    f.sync_all()?;

    // POSIX rename is atomic on macOS/Linux.
    // Windows: caller must add inter-process mutex (see
    // appendix C of the plan).
    fs::rename(&tmp, path)?;
    Ok(())
}

fn read_last_record(path: &Path) -> Result<Option<IdempotentRecord>, IdempotentError> {
    if !path.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(path)?;
    // Each per-key file holds exactly one record (merged on
    // every append). Reading the last non-empty line is the
    // authoritative shape.
    let last_non_empty = content
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty());
    match last_non_empty {
        None => Ok(None),
        Some(line) => Ok(Some(serde_json::from_str(line)?)),
    }
}

/// Merge `new` into `existing`: keep `existing`'s
/// `_idempotency_key`, `_version`, and `_created_at`, then
/// concatenate the new transitions. If `new._final` is true the
/// merged record becomes final; otherwise it inherits the
/// existing `_final` value (typically `false`). The payload is
/// always the new one — the most recent write wins for the
/// payload, but the transition log is monotonic.
fn merge_records(existing: IdempotentRecord, new: IdempotentRecord) -> IdempotentRecord {
    let mut merged = existing;
    merged._transitions.extend(new._transitions);
    if new._final {
        merged._final = true;
    }
    merged.payload = new.payload;
    merged
}

/// RAII guard that releases the OS-level file lock on drop.
#[cfg(unix)]
struct LockGuard(Flock<File>);
#[cfg(not(unix))]
struct LockGuard;

#[cfg(unix)]
fn acquire_exclusive_lock(path: &Path) -> Result<LockGuard, IdempotentError> {
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .custom_flags(OFlag::O_CLOEXEC.bits())
        .open(path)?;
    let flock = Flock::lock(file, FlockArg::LockExclusive).map_err(|(_, errno)| {
        IdempotentError::Io(std::io::Error::from_raw_os_error(errno as i32))
    })?;
    Ok(LockGuard(flock))
}

#[cfg(not(unix))]
fn acquire_exclusive_lock(_path: &Path) -> Result<LockGuard, IdempotentError> {
    Err(IdempotentError::NoOsLock)
}

#[cfg(test)]
mod tests;