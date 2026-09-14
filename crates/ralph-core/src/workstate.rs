//! Loop-scoped workstate storage.
//!
//! Workstate is a per-worktree, loop-scoped key-value store persisted as
//! append-only JSONL at `.ralph/agent/workstate.jsonl`. Writes append one
//! record line per mutation under an exclusive `FileLock`; reads take a
//! shared lock and fold the lines into a live view where the last line per
//! `(loop_id, key)` wins and `deleted: true` tombstone lines remove the
//! entry. Entries are scoped by loop id: the human CLI operates on the
//! loop-less (`None`) scope, in-loop agents on their current loop.

use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::file_lock::FileLock;

/// Default path for the workstate file relative to the workspace root.
pub const DEFAULT_WORKSTATE_PATH: &str = ".ralph/agent/workstate.jsonl";

/// Maximum characters per workstate value (rejected on set).
pub const MAX_WORKSTATE_VALUE_CHARS: usize = 10_000;

/// Errors produced by workstate validation and IO.
#[derive(Debug, thiserror::Error)]
pub enum WorkstateError {
    #[error("workstate key must not be empty")]
    EmptyKey,
    #[error("workstate key must not contain whitespace or control characters: {0:?}")]
    InvalidKey(String),
    #[error("workstate value exceeds {max} characters (got {got})")]
    ValueTooLong { max: usize, got: usize },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// One workstate record line in `workstate.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkstateEntry {
    pub loop_id: Option<String>,
    pub key: String,
    pub value: String,
    pub updated_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hat: Option<String>,
    #[serde(default)]
    pub deleted: bool,
}

/// Validates a workstate key shape.
pub fn validate_key(key: &str) -> Result<(), WorkstateError> {
    if key.is_empty() {
        return Err(WorkstateError::EmptyKey);
    }
    if key.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(WorkstateError::InvalidKey(key.to_string()));
    }
    Ok(())
}

/// Validates a workstate value shape.
pub fn validate_value(value: &str) -> Result<(), WorkstateError> {
    let got = value.chars().count();
    if got > MAX_WORKSTATE_VALUE_CHARS {
        return Err(WorkstateError::ValueTooLong {
            max: MAX_WORKSTATE_VALUE_CHARS,
            got,
        });
    }
    Ok(())
}

/// Folds record lines into the live view: last line wins per
/// `(loop_id, key)`, tombstones remove the entry.
pub fn fold_entries(
    entries: impl IntoIterator<Item = WorkstateEntry>,
) -> BTreeMap<(Option<String>, String), WorkstateEntry> {
    let mut folded = BTreeMap::new();
    for entry in entries {
        let key = (entry.loop_id.clone(), entry.key.clone());
        if entry.deleted {
            folded.remove(&key);
        } else {
            folded.insert(key, entry);
        }
    }
    folded
}

/// Append-only JSONL store for loop-scoped workstate.
#[derive(Debug, Clone)]
pub struct WorkstateStore {
    path: PathBuf,
}

impl WorkstateStore {
    /// Creates a store at the given JSONL path.
    #[must_use]
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    /// Creates a store at `.ralph/agent/workstate.jsonl` under `root`.
    #[must_use]
    pub fn with_default_path(root: impl AsRef<Path>) -> Self {
        Self::new(root.as_ref().join(DEFAULT_WORKSTATE_PATH))
    }

    /// Returns the backing file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Returns true when the backing file exists.
    #[must_use]
    pub fn exists(&self) -> bool {
        self.path.exists()
    }

    /// Upserts `(loop_id, key)` by appending a record line.
    pub fn set(
        &self,
        loop_id: Option<&str>,
        key: &str,
        value: &str,
        hat: Option<&str>,
    ) -> Result<(), WorkstateError> {
        validate_key(key)?;
        validate_value(value)?;
        self.append_line(&WorkstateEntry {
            loop_id: loop_id.map(str::to_string),
            key: key.to_string(),
            value: value.to_string(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
            hat: hat.map(str::to_string),
            deleted: false,
        })
    }

    /// Returns the live entry for `(loop_id, key)`, if any.
    pub fn get(
        &self,
        loop_id: Option<&str>,
        key: &str,
    ) -> Result<Option<WorkstateEntry>, WorkstateError> {
        let lookup = (loop_id.map(str::to_string), key.to_string());
        Ok(self.load_folded()?.remove(&lookup))
    }

    /// Lists live entries for `loop_id`, sorted by key.
    pub fn list(&self, loop_id: Option<&str>) -> Result<Vec<WorkstateEntry>, WorkstateError> {
        Ok(self
            .load_folded()?
            .into_values()
            .filter(|e| e.loop_id.as_deref() == loop_id)
            .collect())
    }

    /// Appends a tombstone for `(loop_id, key)`. Returns whether a live
    /// entry existed.
    pub fn delete(
        &self,
        loop_id: Option<&str>,
        key: &str,
        hat: Option<&str>,
    ) -> Result<bool, WorkstateError> {
        validate_key(key)?;
        if self.get(loop_id, key)?.is_none() {
            return Ok(false);
        }
        self.append_line(&WorkstateEntry {
            loop_id: loop_id.map(str::to_string),
            key: key.to_string(),
            value: String::new(),
            updated_at_ms: chrono::Utc::now().timestamp_millis(),
            hat: hat.map(str::to_string),
            deleted: true,
        })?;
        Ok(true)
    }

    /// Appends one serialized record line under an exclusive lock.
    fn append_line(&self, entry: &WorkstateEntry) -> Result<(), WorkstateError> {
        let lock = FileLock::new(&self.path)?;
        let _guard = lock.exclusive()?;
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut line = serde_json::to_string(entry)?;
        line.push('\n');
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(line.as_bytes())?;
        Ok(())
    }

    /// Reads all lines under a shared lock and folds them into the live
    /// view. Malformed lines are skipped with a warning.
    fn load_folded(
        &self,
    ) -> Result<BTreeMap<(Option<String>, String), WorkstateEntry>, WorkstateError> {
        if !self.exists() {
            return Ok(BTreeMap::new());
        }
        let lock = FileLock::new(&self.path)?;
        let _guard = lock.shared()?;
        let content = fs::read_to_string(&self.path)?;
        let entries = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .enumerate()
            .filter_map(
                |(idx, line)| match serde_json::from_str::<WorkstateEntry>(line) {
                    Ok(entry) => Some(entry),
                    Err(err) => {
                        tracing::warn!(
                            path = %self.path.display(),
                            line = idx + 1,
                            error = %err,
                            "skipping malformed workstate line"
                        );
                        None
                    }
                },
            );
        Ok(fold_entries(entries))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_store() -> (TempDir, WorkstateStore) {
        let temp_dir = TempDir::new().expect("temp dir");
        let store = WorkstateStore::with_default_path(temp_dir.path());
        (temp_dir, store)
    }

    fn entry(loop_id: Option<&str>, key: &str, value: &str, ms: i64) -> WorkstateEntry {
        WorkstateEntry {
            loop_id: loop_id.map(str::to_string),
            key: key.to_string(),
            value: value.to_string(),
            updated_at_ms: ms,
            hat: None,
            deleted: false,
        }
    }

    fn tombstone(loop_id: Option<&str>, key: &str, ms: i64) -> WorkstateEntry {
        WorkstateEntry {
            deleted: true,
            ..entry(loop_id, key, "", ms)
        }
    }

    // ---- shape validation ----

    #[test]
    fn validate_key_accepts_simple_key() {
        assert!(validate_key("draft-conclusion").is_ok());
        assert!(validate_key("unit:U1:status").is_ok());
    }

    #[test]
    fn validate_key_rejects_empty() {
        assert!(matches!(validate_key(""), Err(WorkstateError::EmptyKey)));
        assert!(matches!(
            validate_key("   "),
            Err(WorkstateError::InvalidKey(_))
        ));
    }

    #[test]
    fn validate_key_rejects_whitespace_and_control() {
        for key in [
            "with space",
            "with\ttab",
            "with\nnewline",
            "with\rcr",
            "with\u{7}bell",
        ] {
            assert!(
                matches!(validate_key(key), Err(WorkstateError::InvalidKey(_))),
                "key {key:?} must be rejected"
            );
        }
    }

    #[test]
    fn validate_value_accepts_limit_boundary() {
        let ok = "x".repeat(MAX_WORKSTATE_VALUE_CHARS);
        assert!(validate_value(&ok).is_ok());
    }

    #[test]
    fn validate_value_rejects_over_limit() {
        let too_long = "x".repeat(MAX_WORKSTATE_VALUE_CHARS + 1);
        assert!(matches!(
            validate_value(&too_long),
            Err(WorkstateError::ValueTooLong { .. })
        ));
        // Multi-byte chars count as chars, not bytes.
        let too_long_chars = "汉".repeat(MAX_WORKSTATE_VALUE_CHARS + 1);
        assert!(validate_value(&too_long_chars).is_err());
    }

    // ---- fold (last-wins per (loop_id, key), tombstones removed) ----

    #[test]
    fn fold_entries_last_write_wins_by_line_order() {
        // Line order wins even when the later line has an older timestamp.
        let folded = fold_entries(vec![
            entry(Some("l1"), "k", "first", 200),
            entry(Some("l1"), "k", "second", 100),
        ]);
        assert_eq!(folded.len(), 1);
        assert_eq!(
            folded[&(Some("l1".to_string()), "k".to_string())].value,
            "second"
        );
    }

    #[test]
    fn fold_entries_removes_tombstones() {
        let folded = fold_entries(vec![
            entry(Some("l1"), "k", "v", 1),
            tombstone(Some("l1"), "k", 2),
        ]);
        assert!(folded.is_empty());
    }

    #[test]
    fn fold_entries_set_after_delete_revives() {
        let folded = fold_entries(vec![
            entry(Some("l1"), "k", "v1", 1),
            tombstone(Some("l1"), "k", 2),
            entry(Some("l1"), "k", "v2", 3),
        ]);
        assert_eq!(
            folded[&(Some("l1".to_string()), "k".to_string())].value,
            "v2"
        );
    }

    #[test]
    fn fold_entries_scopes_by_loop_and_none() {
        let folded = fold_entries(vec![
            entry(Some("l1"), "k", "l1-value", 1),
            entry(Some("l2"), "k", "l2-value", 2),
            entry(None, "k", "human-value", 3),
        ]);
        assert_eq!(folded.len(), 3);
        assert_eq!(
            folded[&(Some("l1".to_string()), "k".to_string())].value,
            "l1-value"
        );
        assert_eq!(
            folded[&(Some("l2".to_string()), "k".to_string())].value,
            "l2-value"
        );
        assert_eq!(folded[&(None, "k".to_string())].value, "human-value");
    }

    // ---- store behavior (real temp files, real FileLock) ----

    #[test]
    fn store_set_get_roundtrip() {
        let (_tmp, store) = temp_store();
        store
            .set(Some("l1"), "k", "方案A待验证", Some("executor"))
            .expect("set");
        let got = store.get(Some("l1"), "k").expect("get").expect("entry");
        assert_eq!(got.value, "方案A待验证");
        assert_eq!(got.loop_id.as_deref(), Some("l1"));
        assert_eq!(got.hat.as_deref(), Some("executor"));
        assert!(got.updated_at_ms > 0);
    }

    #[test]
    fn store_upsert_single_effective_value() {
        let (_tmp, store) = temp_store();
        store.set(Some("l1"), "k", "first", None).expect("set 1");
        store.set(Some("l1"), "k", "second", None).expect("set 2");
        assert_eq!(
            store
                .get(Some("l1"), "k")
                .expect("get")
                .expect("entry")
                .value,
            "second"
        );
        let listed = store.list(Some("l1")).expect("list");
        assert_eq!(listed.len(), 1);
    }

    #[test]
    fn store_delete_then_get_none() {
        let (_tmp, store) = temp_store();
        store.set(Some("l1"), "k", "v", None).expect("set");
        assert!(store.delete(Some("l1"), "k", None).expect("delete"));
        assert!(store.get(Some("l1"), "k").expect("get").is_none());
        assert!(store.list(Some("l1")).expect("list").is_empty());
    }

    #[test]
    fn store_delete_absent_returns_false() {
        let (_tmp, store) = temp_store();
        assert!(!store.delete(Some("l1"), "missing", None).expect("delete"));
        assert!(!store.exists(), "deleting an absent key writes nothing");
    }

    #[test]
    fn store_loop_scope_isolation() {
        let (_tmp, store) = temp_store();
        store.set(Some("l1"), "k", "l1", None).expect("set l1");
        store.set(Some("l2"), "k", "l2", None).expect("set l2");
        store.set(None, "k", "human", None).expect("set human");

        assert_eq!(
            store
                .get(Some("l1"), "k")
                .expect("get")
                .expect("entry")
                .value,
            "l1"
        );
        assert_eq!(
            store
                .get(Some("l2"), "k")
                .expect("get")
                .expect("entry")
                .value,
            "l2"
        );
        assert_eq!(
            store.get(None, "k").expect("get").expect("entry").value,
            "human"
        );
        assert_eq!(store.list(Some("l1")).expect("list").len(), 1);
        assert_eq!(store.list(None).expect("list").len(), 1);
    }

    #[test]
    fn store_skips_malformed_lines() {
        let (_tmp, store) = temp_store();
        store.set(Some("l1"), "good", "v", None).expect("set");
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(store.path())
            .expect("open");
        use std::io::Write;
        writeln!(file, "{{not json").expect("write garbage");
        writeln!(file).expect("write blank");
        drop(file);

        let listed = store.list(Some("l1")).expect("list tolerates bad lines");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].key, "good");
    }

    #[test]
    fn store_rejects_invalid_input_without_writing() {
        let (_tmp, store) = temp_store();
        assert!(store.set(Some("l1"), "", "v", None).is_err());
        assert!(store.set(Some("l1"), "bad key", "v", None).is_err());
        assert!(
            store
                .set(
                    Some("l1"),
                    "k",
                    &"x".repeat(MAX_WORKSTATE_VALUE_CHARS + 1),
                    None
                )
                .is_err()
        );
        assert!(!store.exists(), "rejected writes must not create the file");
    }

    #[test]
    fn store_list_sorted_by_key() {
        let (_tmp, store) = temp_store();
        store.set(Some("l1"), "b-key", "1", None).expect("set");
        store.set(Some("l1"), "a-key", "2", None).expect("set");
        let listed = store.list(Some("l1")).expect("list");
        let keys: Vec<_> = listed.iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["a-key", "b-key"]);
    }

    #[test]
    fn store_concurrent_appends_lose_no_lines() {
        let (_tmp, store) = temp_store();
        let store_a = store.clone();
        let store_b = store.clone();

        let handle_a = std::thread::spawn(move || {
            for i in 0..10 {
                store_a
                    .set(Some("l1"), &format!("a-{i}"), "v", None)
                    .expect("set a");
            }
        });
        let handle_b = std::thread::spawn(move || {
            for i in 0..10 {
                store_b
                    .set(Some("l1"), &format!("b-{i}"), "v", None)
                    .expect("set b");
            }
        });
        handle_a.join().expect("join a");
        handle_b.join().expect("join b");

        let raw = std::fs::read_to_string(store.path()).expect("read raw");
        assert_eq!(
            raw.lines().filter(|l| !l.trim().is_empty()).count(),
            20,
            "append-only + FileLock must not lose or interleave lines: {raw}"
        );
        assert_eq!(store.list(Some("l1")).expect("list").len(), 20);
    }
}
