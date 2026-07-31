//! U11 (plan 2026-07-30-004): durable Recovery Intent + persistent budget.
//!
//! Recovery routing decisions (which hat should fix what) are persisted to
//! `<workspace>/.ralph/agent/recovery-intents.jsonl`. The retry budget
//! **survives loop restarts** — it continues from where it left off and never
//! resets. This is the durable counterpart to the in-memory recovery routing:
//! a loop that crashes and is restarted must not get a fresh retry budget, or
//! a permanently failing recovery route could loop forever.
//!
//! # Budget persistence guarantee
//!
//! Every mutation ([`RecoveryIntentStore::record`] and
//! [`RecoveryIntentStore::increment_attempt`]) is flushed to disk under an
//! exclusive file lock **before** it returns. Reopening the store from the
//! same workspace therefore observes the exact `attempt_count` and `exhausted`
//! flag that were in effect when the previous instance stopped — the budget
//! continues, it does not reset.
//!
//! # Concurrency
//!
//! Uses the same cross-process [`FileLock`] pattern as
//! [`crate::execution_contract::ActivationRegistry`]: mutations acquire an
//! exclusive lock and reload the on-disk state before applying the change, so
//! concurrent Ralph processes serialise on the lock and never clobber each
//! other's budget increments.
//!
//! # Fail-closed corruption handling
//!
//! A corrupt or unparseable line fails the load with
//! [`RecoveryError::Corrupt`] rather than being silently dropped. Silently
//! discarding a budget line would reset a budget — exactly the failure mode
//! this store exists to prevent — so callers must handle the error explicitly.

use crate::file_lock::FileLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Workspace-relative path (under `.ralph/`) of the recovery intent store.
pub const RECOVERY_INTENTS_RELATIVE_PATH: &str = "agent/recovery-intents.jsonl";

/// A durable recovery intent: a routing decision plus its retry budget state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveryIntent {
    /// Unique intent identifier.
    pub intent_id: String,
    /// The hat that should handle the recovery.
    pub target_hat: String,
    /// Human-readable reason for the recovery.
    pub reason: String,
    /// Number of attempts made so far.
    pub attempt_count: u32,
    /// Maximum allowed attempts.
    pub budget: u32,
    /// Whether the budget has been exhausted.
    pub exhausted: bool,
}

/// Error returned by recovery intent store operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryError {
    /// The retry budget has been exceeded.
    BudgetExhausted {
        /// The attempt count that breached the budget (always `> budget`).
        attempt: u32,
        /// The configured budget ceiling.
        budget: u32,
    },
    /// No intent with the given id exists in the store.
    NotFound {
        /// The id that was looked up.
        intent_id: String,
    },
    /// The store file exists but contains an unparseable line.
    Corrupt {
        /// Underlying parse error detail.
        source: String,
    },
    /// An IO error occurred while reading or writing the store.
    IoError {
        /// Underlying IO error detail.
        source: String,
    },
}

impl std::fmt::Display for RecoveryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BudgetExhausted { attempt, budget } => write!(
                f,
                "recovery budget exhausted: attempt {attempt} exceeds budget {budget}; \
                 stop retrying this recovery route and escalate"
            ),
            Self::NotFound { intent_id } => write!(
                f,
                "recovery intent {intent_id:?} not found in store; \
                 record it before incrementing its budget"
            ),
            Self::Corrupt { source } => write!(
                f,
                "recovery intent store is corrupt: {source}; \
                 refusing to reset budgets from a damaged store"
            ),
            Self::IoError { source } => write!(f, "recovery intent store IO error: {source}"),
        }
    }
}

impl std::error::Error for RecoveryError {}

/// Persistent store for recovery intents and their retry budgets.
///
/// See the [module docs](self) for the budget persistence guarantee.
#[derive(Debug)]
pub struct RecoveryIntentStore {
    /// Path to the JSONL store file.
    path: PathBuf,
    /// In-memory view of the store, reloaded from disk under lock on mutation.
    intents: HashMap<String, RecoveryIntent>,
    /// Cross-process file lock serialising reads and writes.
    file_lock: FileLock,
}

impl RecoveryIntentStore {
    /// Open or create a store rooted at the given workspace directory.
    ///
    /// The store file lives at
    /// `<workspace>/.ralph/agent/recovery-intents.jsonl`. If it exists its
    /// intents are loaded (under a shared lock); a corrupt file returns
    /// [`RecoveryError::Corrupt`].
    pub fn open(workspace: &Path) -> Result<Self, RecoveryError> {
        let path = workspace.join(".ralph").join(RECOVERY_INTENTS_RELATIVE_PATH);
        let file_lock = FileLock::new(&path).map_err(|e| RecoveryError::IoError {
            source: format!("failed to create file lock for recovery store: {e}"),
        })?;

        let intents = {
            let _guard = file_lock.shared().map_err(|e| RecoveryError::IoError {
                source: format!("failed to acquire shared lock: {e}"),
            })?;
            Self::load_from_file(&path)?
        };

        Ok(Self {
            path,
            intents,
            file_lock,
        })
    }

    /// Record a new recovery intent, or replace an existing one with the same
    /// `intent_id`.
    ///
    /// The write is flushed to disk under an exclusive lock before returning, so
    /// the intent is durable across restarts immediately.
    pub fn record(&mut self, intent: RecoveryIntent) -> Result<(), RecoveryError> {
        let _guard = self.file_lock.exclusive().map_err(|e| RecoveryError::IoError {
            source: format!("failed to acquire exclusive lock: {e}"),
        })?;
        // Reload under the exclusive lock so concurrent writes are preserved.
        self.intents = Self::load_from_file(&self.path)?;
        self.intents.insert(intent.intent_id.clone(), intent);
        self.persist_locked()
    }

    /// Get an intent by id from the in-memory view loaded at `open`.
    #[must_use]
    pub fn get(&self, intent_id: &str) -> Option<&RecoveryIntent> {
        self.intents.get(intent_id)
    }

    /// Increment the attempt count for an intent and persist it.
    ///
    /// Returns the new attempt count on success. When the increment pushes the
    /// count past `budget`, the intent is marked `exhausted`, persisted, and
    /// [`RecoveryError::BudgetExhausted`] is returned. Because the exhausted
    /// state is persisted, further increments keep returning
    /// `BudgetExhausted` (idempotently, never panicking) and stay blocked
    /// across restarts — the budget does not reset.
    pub fn increment_attempt(&mut self, intent_id: &str) -> Result<u32, RecoveryError> {
        let _guard = self.file_lock.exclusive().map_err(|e| RecoveryError::IoError {
            source: format!("failed to acquire exclusive lock: {e}"),
        })?;
        // Reload under the exclusive lock so concurrent writes are preserved.
        self.intents = Self::load_from_file(&self.path)?;

        let Some(intent) = self.intents.get_mut(intent_id) else {
            return Err(RecoveryError::NotFound {
                intent_id: intent_id.to_string(),
            });
        };

        intent.attempt_count += 1;
        let exhausted_now = intent.attempt_count > intent.budget;
        if exhausted_now {
            intent.exhausted = true;
        }
        // Capture Copy values; the mutable borrow ends here, before persist.
        let attempt = intent.attempt_count;
        let budget = intent.budget;

        // Persist the mutated state (including the exhausted flag) before
        // returning, so the budget is durable across restarts.
        self.persist_locked()?;

        if exhausted_now {
            Err(RecoveryError::BudgetExhausted { attempt, budget })
        } else {
            Ok(attempt)
        }
    }

    /// Whether an intent's budget is exhausted (from the in-memory view).
    #[must_use]
    pub fn is_exhausted(&self, intent_id: &str) -> bool {
        self.intents
            .get(intent_id)
            .map(|i| i.exhausted)
            .unwrap_or(false)
    }

    /// Load and parse the store file. A missing file is an empty store; a
    /// present-but-unparseable line is a hard [`RecoveryError::Corrupt`] error.
    fn load_from_file(path: &Path) -> Result<HashMap<String, RecoveryIntent>, RecoveryError> {
        if !path.exists() {
            return Ok(HashMap::new());
        }
        let content = std::fs::read_to_string(path).map_err(|e| RecoveryError::IoError {
            source: format!("failed to read recovery store: {e}"),
        })?;
        let mut intents = HashMap::new();
        for (idx, line) in content.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let intent: RecoveryIntent =
                serde_json::from_str(line).map_err(|e| RecoveryError::Corrupt {
                    source: format!("line {}: {e}", idx + 1),
                })?;
            intents.insert(intent.intent_id.clone(), intent);
        }
        Ok(intents)
    }

    /// Rewrite the whole store file from the in-memory map. Assumes the caller
    /// already holds the exclusive lock. Intents are written in `intent_id`
    /// order for stable, diffable output.
    fn persist_locked(&self) -> Result<(), RecoveryError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| RecoveryError::IoError {
                source: format!("failed to create recovery store directory: {e}"),
            })?;
        }
        let mut ordered: Vec<&RecoveryIntent> = self.intents.values().collect();
        ordered.sort_by(|a, b| a.intent_id.cmp(&b.intent_id));

        let mut content = String::new();
        for intent in ordered {
            let json = serde_json::to_string(intent).map_err(|e| RecoveryError::IoError {
                source: format!("failed to serialize recovery intent: {e}"),
            })?;
            content.push_str(&json);
            content.push('\n');
        }
        std::fs::write(&self.path, content).map_err(|e| RecoveryError::IoError {
            source: format!("failed to write recovery store: {e}"),
        })?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn intent(id: &str, attempt_count: u32, budget: u32) -> RecoveryIntent {
        RecoveryIntent {
            intent_id: id.to_string(),
            target_hat: "fixer".to_string(),
            reason: "test recovery".to_string(),
            attempt_count,
            budget,
            exhausted: false,
        }
    }

    /// Test 1: a recorded intent is readable after dropping and reopening the
    /// store — routing decisions persist across restarts.
    #[test]
    fn u11_recovery_intent_persists_across_restart() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();

        {
            let mut store = RecoveryIntentStore::open(workspace).unwrap();
            store.record(intent("intent-1", 1, 3)).unwrap();
        }

        // Reopen from the same workspace.
        let store = RecoveryIntentStore::open(workspace).unwrap();
        let got = store.get("intent-1").expect("intent must survive restart");
        assert_eq!(got.intent_id, "intent-1");
        assert_eq!(got.target_hat, "fixer");
        assert_eq!(got.attempt_count, 1);
        assert_eq!(got.budget, 3);
        assert!(!got.exhausted);
    }

    /// Test 2: the budget continues from where it left off after a restart; it
    /// never resets.
    #[test]
    fn u11_budget_continues_after_restart() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();

        {
            let mut store = RecoveryIntentStore::open(workspace).unwrap();
            store.record(intent("intent-1", 2, 3)).unwrap();
        }

        // Reopen and keep incrementing — the count resumes at 2, not 0.
        let mut store = RecoveryIntentStore::open(workspace).unwrap();

        let new_count = store.increment_attempt("intent-1").unwrap();
        assert_eq!(new_count, 3, "budget continues: 2 + 1 = 3, not a reset to 1");

        let err = store.increment_attempt("intent-1").unwrap_err();
        assert_eq!(
            err,
            RecoveryError::BudgetExhausted {
                attempt: 4,
                budget: 3
            },
            "attempt 4 breaches budget 3"
        );
    }

    /// Test 3: once the budget is exhausted, further increments stay blocked
    /// idempotently (no panic) and `is_exhausted` reports `true`.
    #[test]
    fn u11_exhausted_budget_blocks_exactly_once() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();

        let mut store = RecoveryIntentStore::open(workspace).unwrap();
        store.record(intent("intent-1", 0, 1)).unwrap();

        // attempt_count 0 -> 1, within budget 1.
        assert_eq!(store.increment_attempt("intent-1").unwrap(), 1);

        // attempt_count 1 -> 2 > budget 1: exhausted.
        let err = store.increment_attempt("intent-1").unwrap_err();
        assert_eq!(
            err,
            RecoveryError::BudgetExhausted {
                attempt: 2,
                budget: 1
            }
        );

        // Still exhausted — idempotent, no panic.
        let err = store.increment_attempt("intent-1").unwrap_err();
        assert!(matches!(err, RecoveryError::BudgetExhausted { .. }));

        assert!(store.is_exhausted("intent-1"));
    }

    /// Exhaustion survives a restart: the blocked state is durable, not just
    /// in-memory.
    #[test]
    fn u11_exhaustion_survives_restart() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();

        {
            let mut store = RecoveryIntentStore::open(workspace).unwrap();
            store.record(intent("intent-1", 0, 1)).unwrap();
            store.increment_attempt("intent-1").unwrap();
            store.increment_attempt("intent-1").unwrap_err();
        }

        let mut store = RecoveryIntentStore::open(workspace).unwrap();
        assert!(
            store.is_exhausted("intent-1"),
            "exhausted flag must persist across restart"
        );
        let err = store.increment_attempt("intent-1").unwrap_err();
        assert!(matches!(err, RecoveryError::BudgetExhausted { .. }));
    }

    /// Incrementing an unknown intent is a clean `NotFound`, not a panic.
    #[test]
    fn u11_unknown_intent_is_not_found() {
        let tmp = TempDir::new().unwrap();
        let mut store = RecoveryIntentStore::open(tmp.path()).unwrap();
        let err = store.increment_attempt("nope").unwrap_err();
        assert_eq!(
            err,
            RecoveryError::NotFound {
                intent_id: "nope".to_string()
            }
        );
        assert!(!store.is_exhausted("nope"));
    }

    /// A corrupt store line fails closed rather than silently resetting a
    /// budget.
    #[test]
    fn u11_corrupt_store_fails_closed() {
        let tmp = TempDir::new().unwrap();
        let workspace = tmp.path();
        let path = workspace.join(".ralph").join(RECOVERY_INTENTS_RELATIVE_PATH);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not valid json\n").unwrap();

        let err = RecoveryIntentStore::open(workspace).unwrap_err();
        assert!(matches!(err, RecoveryError::Corrupt { .. }));
    }
}
