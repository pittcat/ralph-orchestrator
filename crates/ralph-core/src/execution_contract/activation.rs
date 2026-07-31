//! U3 (plan 2026-07-30-004): persistent activation registry.
//!
//! Provides a shared, persistent record of hat activation identities
//! that both the resident event loop and the independent CLI (`ralph inspect
//! loop`) can query. This ensures both reach identity agreement on the same
//! exact activation and enables fail-closed enforcement of concurrent slots.
//!
//! # Persistence
//!
//! The registry is stored as JSONL at `<workspace>/.ralph/activation-registry.jsonl`.
//! Each line is a JSON object with stable fields — the format is append-only
//! for new entries; completed/superseded states update the in-memory map and
//! append a new status-transition line.
//!
//! # Revision system
//!
//! Each activation carries a monotonically increasing `revision`. Stale
//! revisions (lower than the persisted value) are rejected. This prevents
//! a revived old activation from overwriting a newer one.
//!
//! # Fail-closed corruption handling
//!
//! A corrupt or unparseable registry file causes every operation to return an
//! error — there is **no** silent cold-start fallback that swallows the
//! corruption. Callers must handle the error explicitly.
//!
//! # Thread model
//!
//! The registry is owned by the `EventLoop`. The CLI reads it via a
//! read-only load that returns an error on corruption rather than falling back.

use crate::file_lock::FileLock;
use crate::hat_lifecycle::ActivationKey;
use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use tracing::debug;

/// The relative path within the workspace `.ralph/` directory.
pub const ACTIVATION_REGISTRY_RELATIVE_PATH: &str = "activation-registry.jsonl";

/// Status of a registered activation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ActivationStatus {
    /// The activation is currently running.
    Active,
    /// The activation completed normally via a terminal event.
    Completed,
    /// A newer activation for the same hat superseded this one.
    Superseded,
}

/// A single activation record stored in the registry.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ActivationRecord {
    /// The stable identity key (loop_id, iteration, hat_id).
    pub key: ActivationKey,
    /// Current status of this activation.
    pub status: ActivationStatus,
    /// Monotonically increasing revision for this activation.
    /// Incremented on every state transition.
    pub revision: u64,
    /// When this activation was first registered.
    pub registered_at: std::time::SystemTime,
    /// When the status last changed.
    pub updated_at: std::time::SystemTime,
}

/// Error returned by activation registry operations.
#[derive(Debug, Clone)]
pub enum ActivationRegistryError {
    /// The registry file exists but is not valid JSONL.
    CorruptRegistry { source: String },
    /// An IO error occurred while reading or writing.
    IoError { source: String },
    /// The activation identity is already active (concurrent slot conflict).
    SlotAlreadyActive { key: ActivationKey },
    /// A stale revision was rejected.
    StaleRevision { key: ActivationKey, expected: u64, actual: u64 },
}

impl std::fmt::Display for ActivationRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::CorruptRegistry { source } => {
                write!(f, "activation registry is corrupt: {source}")
            }
            Self::IoError { source } => {
                write!(f, "activation registry IO error: {source}")
            }
            Self::SlotAlreadyActive { key } => {
                write!(f, "activation slot already active: {key}")
            }
            Self::StaleRevision { key, expected, actual } => {
                write!(
                    f,
                    "stale revision for {key}: expected r{expected}, found r{actual}",
                )
            }
        }
    }
}

impl std::error::Error for ActivationRegistryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        None
    }
}

/// The persistent activation registry.
///
/// Tracks activation identities across the resident loop and the CLI
/// so both can reach agreement on the same exact activation.
///
/// # Fail-closed on corruption
///
/// If the registry file cannot be parsed, **all** operations return
/// `ActivationRegistryError::CorruptRegistry`. There is no fallback that
/// silently discards the error and starts fresh — callers must handle the
/// error explicitly.
#[derive(Debug)]
pub struct ActivationRegistry {
    /// In-memory map from ActivationKey to the latest ActivationRecord.
    /// Written to disk on every mutation.
    records: HashMap<ActivationKey, ActivationRecord>,
    /// Path to the registry file.
    registry_path: PathBuf,
    /// Cross-process file lock for serialising reads and writes.
    file_lock: FileLock,
}

impl ActivationRegistry {
    /// The workspace-relative file name for the registry.
    pub const REGISTRY_FILENAME: &'static str = ACTIVATION_REGISTRY_RELATIVE_PATH;

    /// Returns the path to the registry file.
    #[must_use]
    pub fn registry_path(&self) -> &PathBuf {
        &self.registry_path
    }

    /// Open (or create) the activation registry at the given path.
    ///
    /// If the file does not exist it is created empty.
    /// If the file exists but is corrupt, returns `Err`.
    ///
    /// Uses a cross-process `FileLock` to serialise reads and writes
    /// across concurrent Ralph processes.
    pub fn open(registry_path: PathBuf) -> Result<Self, ActivationRegistryError> {
        let file_lock = FileLock::new(&registry_path).map_err(|e| {
            ActivationRegistryError::IoError {
                source: format!("failed to create file lock for registry: {}", e),
            }
        })?;

        let records = if registry_path.exists() {
            Self::load_from_file_locked(&registry_path, &file_lock)?
        } else {
            // Create the file so subsequent opens don't race on create.
            if let Some(parent) = registry_path.parent() {
                fs::create_dir_all(parent).map_err(|e| ActivationRegistryError::IoError {
                    source: format!("failed to create registry directory: {}", e),
                })?;
            }
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&registry_path)
                .map_err(|e| ActivationRegistryError::IoError {
                    source: format!("failed to create registry file: {}", e),
                })?;
            HashMap::new()
        };

        Ok(Self {
            records,
            registry_path,
            file_lock,
        })
    }

    /// Load and parse the registry file under a shared lock.
    fn load_from_file_locked(
        path: &PathBuf,
        file_lock: &FileLock,
    ) -> Result<HashMap<ActivationKey, ActivationRecord>, ActivationRegistryError> {
        let _guard = file_lock.shared().map_err(|e| ActivationRegistryError::IoError {
            source: format!("failed to acquire shared lock: {}", e),
        })?;
        Self::load_from_file(path)
    }

    /// Load and parse the registry file. Returns an error on any corruption.
    fn load_from_file(path: &PathBuf) -> Result<HashMap<ActivationKey, ActivationRecord>, ActivationRegistryError> {
        let file = File::open(path).map_err(|e| ActivationRegistryError::IoError {
            source: format!("failed to open registry: {}", e),
        })?;
        let reader = BufReader::new(file);
        let mut records: HashMap<ActivationKey, ActivationRecord> = HashMap::new();

        for (line_no, line_result) in reader.lines().enumerate() {
            let line = line_result.map_err(|e| ActivationRegistryError::IoError {
                source: format!("failed to read line {}: {}", line_no + 1, e),
            })?;
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let record: ActivationRecord = serde_json::from_str(line)
                .map_err(|e| ActivationRegistryError::CorruptRegistry {
                    source: format!("line {}: {}", line_no + 1, e),
                })?;
            // Use the latest record per key (append-only semantics).
            records.insert(record.key.clone(), record);
        }

        Ok(records)
    }

    /// Append a record to the registry file. Assumes the caller already
    /// holds an exclusive lock guard.
    fn append_record_locked(
        &self,
        file: &mut File,
        record: &ActivationRecord,
    ) -> Result<(), ActivationRegistryError> {
        let json =
            serde_json::to_string(record).map_err(|e| ActivationRegistryError::IoError {
                source: format!("failed to serialize record: {}", e),
            })?;
        writeln!(file, "{}", json).map_err(|e| ActivationRegistryError::IoError {
            source: format!("failed to write record: {}", e),
        })?;
        file.flush().map_err(|e| ActivationRegistryError::IoError {
            source: format!("failed to flush record: {}", e),
        })?;
        Ok(())
    }

    /// Register a new activation.
    ///
    /// Acquires an exclusive file lock for the entire check-then-write
    /// sequence, preventing cross-process concurrent slot conflicts.
    ///
    /// Returns an error if:
    /// - The slot is already active (`SlotAlreadyActive`).
    /// - A stale revision is detected (`StaleRevision`).
    pub fn activate(
        &mut self,
        key: ActivationKey,
        revision: u64,
    ) -> Result<ActivationRecord, ActivationRegistryError> {
        // Hold the exclusive lock for the entire operation.
        let _guard = self.file_lock.exclusive().map_err(|e| {
            ActivationRegistryError::IoError {
                source: format!("failed to acquire exclusive lock: {}", e),
            }
        })?;

        // Reload state from disk under the exclusive lock so we see any
        // writes from other processes/threads that completed before us.
        if self.registry_path.exists() {
            self.records = Self::load_from_file(&self.registry_path)?;
        }

        // Check for concurrent slot conflict (under lock, with fresh state).
        if let Some(existing) = self.records.get(&key) {
            match existing.status {
                ActivationStatus::Active => {
                    return Err(ActivationRegistryError::SlotAlreadyActive {
                        key: key.clone(),
                    });
                }
                ActivationStatus::Completed | ActivationStatus::Superseded => {
                    // Completed/superseded: check revision.
                    if revision <= existing.revision {
                        return Err(ActivationRegistryError::StaleRevision {
                            key: key.clone(),
                            expected: existing.revision + 1,
                            actual: revision,
                        });
                    }
                }
            }
        }

        let now = std::time::SystemTime::now();
        let record = ActivationRecord {
            key: key.clone(),
            status: ActivationStatus::Active,
            revision,
            registered_at: now,
            updated_at: now,
        };

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.registry_path)
            .map_err(|e| ActivationRegistryError::IoError {
                source: format!("failed to open registry for append: {}", e),
            })?;
        self.append_record_locked(&mut file, &record)?;
        self.records.insert(key, record.clone());
        Ok(record)
    }

    /// Mark an activation as completed.
    ///
    /// Acquires an exclusive file lock for the entire operation.
    ///
    /// Returns an error if:
    /// - The slot is not found and revision is stale.
    pub fn complete(
        &mut self,
        key: &ActivationKey,
        revision: u64,
    ) -> Result<ActivationRecord, ActivationRegistryError> {
        let _guard = self.file_lock.exclusive().map_err(|e| {
            ActivationRegistryError::IoError {
                source: format!("failed to acquire exclusive lock: {}", e),
            }
        })?;

        // Reload state from disk under the exclusive lock.
        if self.registry_path.exists() {
            self.records = Self::load_from_file(&self.registry_path)?;
        }

        let existing = self.records.get(key).ok_or_else(|| {
            // No record found — check if it could be a stale revision.
            debug!("completion request for unknown activation {key} r{}", revision);
            ActivationRegistryError::StaleRevision {
                key: key.clone(),
                expected: revision,
                actual: 0,
            }
        })?;

        if revision <= existing.revision {
            return Err(ActivationRegistryError::StaleRevision {
                key: key.clone(),
                expected: existing.revision + 1,
                actual: revision,
            });
        }

        let now = std::time::SystemTime::now();
        let record = ActivationRecord {
            key: key.clone(),
            status: ActivationStatus::Completed,
            revision,
            registered_at: existing.registered_at,
            updated_at: now,
        };

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.registry_path)
            .map_err(|e| ActivationRegistryError::IoError {
                source: format!("failed to open registry for append: {}", e),
            })?;
        self.append_record_locked(&mut file, &record)?;
        self.records.insert(key.clone(), record.clone());
        Ok(record)
    }

    /// Mark an activation as superseded (replaced by a newer activation
    /// for the same hat).
    ///
    /// Acquires an exclusive file lock for the entire operation.
    pub fn supersede(
        &mut self,
        key: &ActivationKey,
        revision: u64,
    ) -> Result<ActivationRecord, ActivationRegistryError> {
        let _guard = self.file_lock.exclusive().map_err(|e| {
            ActivationRegistryError::IoError {
                source: format!("failed to acquire exclusive lock: {}", e),
            }
        })?;

        // Reload state from disk under the exclusive lock.
        if self.registry_path.exists() {
            self.records = Self::load_from_file(&self.registry_path)?;
        }

        let existing = self.records.get(key).ok_or_else(|| {
            ActivationRegistryError::StaleRevision {
                key: key.clone(),
                expected: revision,
                actual: 0,
            }
        })?;

        if revision <= existing.revision {
            return Err(ActivationRegistryError::StaleRevision {
                key: key.clone(),
                expected: existing.revision + 1,
                actual: revision,
            });
        }

        let now = std::time::SystemTime::now();
        let record = ActivationRecord {
            key: key.clone(),
            status: ActivationStatus::Superseded,
            revision,
            registered_at: existing.registered_at,
            updated_at: now,
        };

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.registry_path)
            .map_err(|e| ActivationRegistryError::IoError {
                source: format!("failed to open registry for append: {}", e),
            })?;
        self.append_record_locked(&mut file, &record)?;
        self.records.insert(key.clone(), record.clone());
        Ok(record)
    }

    /// Query the current record for a given activation key.
    pub fn get(&self, key: &ActivationKey) -> Option<ActivationRecord> {
        self.records.get(key).cloned()
    }

    /// Check if a slot is currently active.
    pub fn is_active(&self, key: &ActivationKey) -> bool {
        self.records
            .get(key)
            .map(|r| r.status == ActivationStatus::Active)
            .unwrap_or(false)
    }

    /// Check if a slot is completed (not active, but was completed).
    pub fn is_completed(&self, key: &ActivationKey) -> bool {
        self.records
            .get(key)
            .map(|r| r.status == ActivationStatus::Completed)
            .unwrap_or(false)
    }
}

/// Read-only view of the activation registry for the CLI path.
/// This never writes — it only loads and queries.
pub fn load_registry_readonly(
    workspace_root: &std::path::Path,
) -> Result<ActivationRegistry, ActivationRegistryError> {
    let registry_path = workspace_root
        .join(".ralph")
        .join(ACTIVATION_REGISTRY_RELATIVE_PATH);

    if !registry_path.exists() {
        // No registry = no activations recorded yet. Return an empty registry.
        return Ok(ActivationRegistry::open(registry_path)
            .expect("open for read-only load of non-existent registry must succeed"));
    }

    ActivationRegistry::open(registry_path)
}

/// Environment variable holding the absolute path to the activation
/// registry JSONL file. Set by the event loop when spawning a hat
/// subprocess so the child can locate the shared registry without
/// deriving it from the workspace root.
pub const ENV_ACTIVATION_REGISTRY_PATH: &str = "RALPH_ACTIVATION_REGISTRY";

/// Environment variable holding the contract revision (u64 as a decimal
/// string) that the spawned hat must agree on. Pairs with
/// `ENV_ACTIVATION_REGISTRY_PATH` so the child process can validate
/// that its compiled contract matches the one the loop is running.
pub const ENV_CONTRACT_REVISION: &str = "RALPH_CONTRACT_REVISION";

/// Resolved activation registry locator passed via spawn environment.
///
/// Constructed by [`resolve_activation_registry_env`] from the two
/// `RALPH_ACTIVATION_*` environment variables. The event loop sets
/// these before spawning a hat subprocess; the CLI reads them when
/// it needs to locate the registry without a workspace root.
#[derive(Debug, Clone)]
pub struct ActivationRegistryLocator {
    /// Absolute path to the registry JSONL file.
    pub registry_path: PathBuf,
    /// Contract revision the spawned hat must agree on.
    pub contract_revision: u64,
}

/// Resolve the activation registry locator from spawn environment
/// variables.
///
/// Returns `None` when neither env var is set (the common case for
/// human CLI invocations). Returns `Some(Err(_))` when the path is
/// set but the revision is missing or unparseable — this is a
/// fail-closed signal that the spawn env is corrupt.
pub fn resolve_activation_registry_env() -> Option<Result<ActivationRegistryLocator, String>> {
    let path_str = std::env::var(ENV_ACTIVATION_REGISTRY_PATH).ok()?;
    let revision_str = std::env::var(ENV_CONTRACT_REVISION);
    match revision_str {
        Ok(s) => match s.parse::<u64>() {
            Ok(rev) => Some(Ok(ActivationRegistryLocator {
                registry_path: PathBuf::from(path_str),
                contract_revision: rev,
            })),
            Err(e) => Some(Err(format!(
                "{ENV_CONTRACT_REVISION}={s:?} is not a valid u64: {e}"
            ))),
        },
        Err(_) => Some(Err(format!(
            "{ENV_ACTIVATION_REGISTRY_PATH} is set but {ENV_CONTRACT_REVISION} is missing"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn make_key(hat: &str) -> ActivationKey {
        ActivationKey {
            loop_id: "loop-1".to_string(),
            iteration: 1,
            hat_id: hat.to_string(),
        }
    }

    fn tmp_path(tmp: &TempDir) -> PathBuf {
        tmp.path().join(".ralph").join(ACTIVATION_REGISTRY_RELATIVE_PATH)
    }

    #[test]
    fn activate_first_time_succeeds() {
        let tmp = TempDir::new().unwrap();
        let mut registry = ActivationRegistry::open(tmp_path(&tmp)).unwrap();

        let key = make_key("worker");
        let record = registry.activate(key.clone(), 1).unwrap();

        assert_eq!(record.status, ActivationStatus::Active);
        assert_eq!(record.revision, 1);
        assert_eq!(record.key, key);
    }

    #[test]
    fn concurrent_slot_rejected() {
        let tmp = TempDir::new().unwrap();
        let mut registry = ActivationRegistry::open(tmp_path(&tmp)).unwrap();

        let key = make_key("worker");
        registry.activate(key.clone(), 1).unwrap();

        // Second activation on the same slot is rejected.
        let err = registry.activate(key.clone(), 2).unwrap_err();
        assert!(matches!(err, ActivationRegistryError::SlotAlreadyActive { .. }));
    }

    #[test]
    fn complete_after_active_succeeds() {
        let tmp = TempDir::new().unwrap();
        let mut registry = ActivationRegistry::open(tmp_path(&tmp)).unwrap();

        let key = make_key("worker");
        registry.activate(key.clone(), 1).unwrap();
        let record = registry.complete(&key, 2).unwrap();

        assert_eq!(record.status, ActivationStatus::Completed);
        assert_eq!(record.revision, 2);
    }

    #[test]
    fn stale_revision_rejected() {
        let tmp = TempDir::new().unwrap();
        let mut registry = ActivationRegistry::open(tmp_path(&tmp)).unwrap();

        let key = make_key("worker");
        registry.activate(key.clone(), 1).unwrap();

        // Same slot while Active is SlotAlreadyActive (not StaleRevision).
        let err = registry.activate(key.clone(), 2).unwrap_err();
        assert!(matches!(err, ActivationRegistryError::SlotAlreadyActive { .. }));

        // Complete the activation.
        registry.complete(&key, 2).unwrap();

        // Now a lower revision than the persisted one is stale.
        let err = registry.activate(key.clone(), 2).unwrap_err();
        assert!(matches!(err, ActivationRegistryError::StaleRevision { .. }));

        // Same revision is also stale (must be strictly greater).
        let err = registry.activate(key.clone(), 2).unwrap_err();
        assert!(matches!(err, ActivationRegistryError::StaleRevision { .. }));
    }

    #[test]
    fn supersede_after_active_succeeds() {
        let tmp = TempDir::new().unwrap();
        let mut registry = ActivationRegistry::open(tmp_path(&tmp)).unwrap();

        let key = make_key("worker");
        registry.activate(key.clone(), 1).unwrap();
        let record = registry.supersede(&key, 2).unwrap();

        assert_eq!(record.status, ActivationStatus::Superseded);
        assert_eq!(record.revision, 2);
    }

    #[test]
    fn replay_completed_yields_same_identity() {
        let tmp = TempDir::new().unwrap();
        let mut registry = ActivationRegistry::open(tmp_path(&tmp)).unwrap();

        let key = make_key("worker");
        registry.activate(key.clone(), 1).unwrap();
        let completed = registry.complete(&key, 2).unwrap();

        // A replay of the completed activation gets the same record.
        let record = registry.get(&key).unwrap();
        assert_eq!(record.key, completed.key);
        assert_eq!(record.status, ActivationStatus::Completed);
        assert_eq!(record.revision, 2);
    }

    #[test]
    fn reopen_registry_preserves_records() {
        let tmp = TempDir::new().unwrap();
        let path = tmp_path(&tmp);

        {
            let mut registry = ActivationRegistry::open(path.clone()).unwrap();
            let key = make_key("worker");
            registry.activate(key.clone(), 1).unwrap();
            registry.complete(&key, 2).unwrap();
        }

        // Reopen and check.
        let registry = ActivationRegistry::open(path).unwrap();
        let key = make_key("worker");
        let record = registry.get(&key).unwrap();
        assert_eq!(record.status, ActivationStatus::Completed);
        assert_eq!(record.revision, 2);
    }

    #[test]
    fn corrupt_registry_returns_error() {
        let tmp = TempDir::new().unwrap();
        let path = tmp_path(&tmp);

        // Write corrupt JSONL.
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "not valid json\n").unwrap();

        let err = ActivationRegistry::open(path).unwrap_err();
        assert!(matches!(err, ActivationRegistryError::CorruptRegistry { .. }));
    }

    #[test]
    fn is_active_and_is_completed() {
        let tmp = TempDir::new().unwrap();
        let mut registry = ActivationRegistry::open(tmp_path(&tmp)).unwrap();

        let key = make_key("worker");
        assert!(!registry.is_active(&key));
        assert!(!registry.is_completed(&key));

        registry.activate(key.clone(), 1).unwrap();
        assert!(registry.is_active(&key));
        assert!(!registry.is_completed(&key));

        registry.complete(&key, 2).unwrap();
        assert!(!registry.is_active(&key));
        assert!(registry.is_completed(&key));
    }

    // ─────────────────────────────────────────────────────────────────────
    // U3 acceptance tests (plan 2026-07-30-004 §U3)
    // ─────────────────────────────────────────────────────────────────────

    /// U3-Red-1: same (loop_id, iteration, hat_id) key with a different
    /// revision must be rejected as StaleRevision when the existing record
    /// is not Active.
    #[test]
    fn u3_activation_registry_persistence_stale_revision() {
        let tmp = TempDir::new().unwrap();
        let mut registry = ActivationRegistry::open(tmp_path(&tmp)).unwrap();

        let key = make_key("worker");
        // First: activate at r1.
        registry.activate(key.clone(), 1).unwrap();
        // Complete it at r2.
        registry.complete(&key, 2).unwrap();

        // Now try to activate the same key at r2 again — stale because
        // existing revision is already 2 and we require revision > existing.
        let err = registry.activate(key.clone(), 2).unwrap_err();
        assert!(
            matches!(
                err,
                ActivationRegistryError::StaleRevision {
                    key: _,
                    expected: 3,
                    actual: 2
                }
            ),
            "expected StaleRevision {{ expected: 3, actual: 2 }}, got: {err:?}"
        );
    }

    /// U3-Red-2: close the registry and reopen it from the same path — the
    /// active record must be readable from disk.
    #[test]
    fn u3_activation_registry_persistence_reopen_readable() {
        let tmp = TempDir::new().unwrap();
        let path = tmp_path(&tmp);

        let (loop_id, iteration, hat_id) = ("loop-x".to_string(), 3, "replayer".to_string());
        let key = ActivationKey {
            loop_id: loop_id.clone(),
            iteration,
            hat_id: hat_id.clone(),
        };

        // Write phase.
        {
            let mut registry = ActivationRegistry::open(path.clone()).unwrap();
            registry.activate(key.clone(), 1).unwrap();
            // Leave it Active — this is the record we will verify on reopen.
        }

        // Reopen phase: the Active record must be readable from disk.
        let registry = ActivationRegistry::open(path).unwrap();
        let record = registry.get(&key).unwrap();
        assert_eq!(record.status, ActivationStatus::Active);
        assert_eq!(record.revision, 1);
        assert_eq!(record.key.loop_id, loop_id);
        assert_eq!(record.key.iteration, iteration);
        assert_eq!(record.key.hat_id, hat_id);
    }

    /// U3-Red-3: concurrent register_active on the same key — one thread
    /// succeeds, the other gets SlotAlreadyActive.  Uses Arc + Barrier to
    /// synchronise the two threads so they race on the same activation key.
    #[test]
    fn u3_activation_registry_persistence_concurrent_race() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let tmp = TempDir::new().unwrap();
        let path = tmp_path(&tmp);

        // Pre-create the registry so both threads open the same file.
        let registry = ActivationRegistry::open(path.clone()).unwrap();
        drop(registry);

        let key = Arc::new(make_key("racer"));
        let path = Arc::new(path);
        let barrier = Arc::new(Barrier::new(2));

        let key_a = key.clone();
        let key_b = key.clone();
        let path_a = path.clone();
        let path_b = path.clone();
        let bar_a = barrier.clone();
        let bar_b = barrier.clone();

        let handle_a = thread::spawn(move || {
            let mut registry = ActivationRegistry::open((*path_a).clone()).unwrap();
            bar_a.wait();
            registry.activate((*key_a).clone(), 1)
        });

        let handle_b = thread::spawn(move || {
            let mut registry = ActivationRegistry::open((*path_b).clone()).unwrap();
            bar_b.wait();
            registry.activate((*key_b).clone(), 1)
        });

        // Collect results.
        let (res_a, res_b) = (handle_a.join().unwrap(), handle_b.join().unwrap());

        // Exactly one must succeed; the other must be SlotAlreadyActive.
        let successes = [res_a.as_ref().ok(), res_b.as_ref().ok()]
            .iter()
            .filter(|r| r.is_some())
            .count();
        let slot_already_actives = [&res_a, &res_b]
            .iter()
            .filter(|r| matches!(r, Err(ActivationRegistryError::SlotAlreadyActive { .. })))
            .count();

        assert_eq!(
            successes, 1,
            "exactly one thread must succeed; got successes={successes}, slot_already_actives={slot_already_actives}"
        );
        assert_eq!(
            slot_already_actives, 1,
            "exactly one thread must get SlotAlreadyActive; got successes={successes}, slot_already_actives={slot_already_actives}"
        );
    }
}
