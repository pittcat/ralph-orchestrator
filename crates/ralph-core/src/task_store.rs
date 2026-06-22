//! Persistent task storage with JSONL format.
//!
//! TaskStore provides load/save operations for the .ralph/agent/tasks.jsonl file,
//! with convenience methods for querying and updating tasks.
//!
//! # Multi-loop Safety
//!
//! When multiple Ralph loops run concurrently (in worktrees), this store uses
//! file locking to ensure safe concurrent access:
//!
//! - **Shared locks** for reading: Multiple loops can read simultaneously
//! - **Exclusive locks** for writing: Only one loop can write at a time
//!
//! Use `load()` and `save()` for simple single-operation access, or use
//! `with_exclusive_lock()` for read-modify-write operations that need atomicity.

use crate::file_lock::FileLock;
use crate::task::{Task, TaskStatus};
use std::io;
use std::path::Path;
use tracing::warn;

/// A store for managing tasks with JSONL persistence and file locking.
pub struct TaskStore {
    path: std::path::PathBuf,
    tasks: Vec<Task>,
    lock: FileLock,
    /// R4 (2026-06-14-003 plan): when `true`, `ensure` enforces the
    /// "current U principle": a task whose key slug matches the
    /// `uN-` / `uNa-` shape must not collide with an open task for
    /// a different `uN` within the same `(loop_id, plan_name, step)`.
    /// Defaults to `false` for backward compatibility; the
    /// `ce-executor-serial` preset opts in via
    /// `EventLoopConfig.enforce_current_unit`.
    enforce_current_unit: bool,
}

/// Parses a JSONL line into a Task, logging a warning on failure.
fn parse_task_line(line: &str) -> Option<Task> {
    match serde_json::from_str(line) {
        Ok(task) => Some(task),
        Err(e) => {
            warn!(
                error = %e,
                line = line.chars().take(200).collect::<String>(),
                "Skipping malformed task line in JSONL"
            );
            None
        }
    }
}

impl TaskStore {
    /// Loads tasks from the JSONL file at the given path.
    ///
    /// If the file doesn't exist, returns an empty store.
    /// Logs warnings for malformed JSON lines and skips them.
    ///
    /// Uses a shared lock to allow concurrent reads from multiple loops.
    pub fn load(path: &Path) -> io::Result<Self> {
        let lock = FileLock::new(path)?;
        let _guard = lock.shared()?;

        let tasks = if path.exists() {
            let content = std::fs::read_to_string(path)?;
            content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| parse_task_line(line))
                .collect()
        } else {
            Vec::new()
        };

        Ok(Self {
            path: path.to_path_buf(),
            tasks,
            lock,
            enforce_current_unit: false,
        })
    }

    /// Saves all tasks to the JSONL file.
    ///
    /// Creates parent directories if they don't exist.
    /// Uses an exclusive lock to prevent concurrent writes.
    pub fn save(&self) -> io::Result<()> {
        let _guard = self.lock.exclusive()?;

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content: String = self
            .tasks
            .iter()
            .map(|t| {
                serde_json::to_string(t).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("task serialization failed: {e}"),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?
            .join("\n");
        std::fs::write(
            &self.path,
            if content.is_empty() {
                String::new()
            } else {
                content + "\n"
            },
        )
    }

    /// Reloads tasks from disk, useful after external modifications.
    ///
    /// Logs warnings for malformed JSON lines and skips them.
    /// Uses a shared lock to allow concurrent reads.
    pub fn reload(&mut self) -> io::Result<()> {
        let _guard = self.lock.shared()?;

        self.tasks = if self.path.exists() {
            let content = std::fs::read_to_string(&self.path)?;
            content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| parse_task_line(line))
                .collect()
        } else {
            Vec::new()
        };

        Ok(())
    }

    /// Executes a read-modify-write operation atomically.
    ///
    /// Acquires an exclusive lock, reloads from disk, executes the
    /// provided function, and saves back to disk. This ensures that
    /// concurrent modifications from other loops are not lost.
    ///
    /// # Example
    ///
    /// ```ignore
    /// store.with_exclusive_lock(|store| {
    ///     let task = Task::new("New task".to_string(), 1);
    ///     store.add(task);
    /// })?;
    /// ```
    pub fn with_exclusive_lock<F, T>(&mut self, f: F) -> io::Result<T>
    where
        F: FnOnce(&mut Self) -> T,
    {
        let _guard = self.lock.exclusive()?;

        // Reload to get latest changes from other loops
        self.tasks = if self.path.exists() {
            let content = std::fs::read_to_string(&self.path)?;
            content
                .lines()
                .filter(|line| !line.trim().is_empty())
                .filter_map(|line| parse_task_line(line))
                .collect()
        } else {
            Vec::new()
        };

        // Execute the user function
        let result = f(self);

        // Save changes
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content: String = self
            .tasks
            .iter()
            .map(|t| {
                serde_json::to_string(t).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("task serialization failed: {e}"),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?
            .join("\n");
        std::fs::write(
            &self.path,
            if content.is_empty() {
                String::new()
            } else {
                content + "\n"
            },
        )?;

        Ok(result)
    }

    /// Adds a new task to the store and returns a reference to it.
    pub fn add(&mut self, task: Task) -> &Task {
        self.tasks.push(task);
        self.tasks.last().unwrap()
    }

    /// Gets a task by ID (immutable reference).
    pub fn get(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    /// Gets a task by stable key (immutable reference).
    pub fn get_by_key(&self, key: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.key.as_deref() == Some(key))
    }

    /// Gets a task by `(loop_id, key)`. Used by P2 to scope `ensure()`
    /// dedup to a single loop, preventing cross-loop collisions when
    /// two loops share a tasks.jsonl.
    ///
    /// A `loop_id == None` here matches only tasks whose own `loop_id`
    /// is also `None` (legacy / human-CLI entries).
    pub fn get_by_key_in_loop(&self, key: &str, loop_id: Option<&str>) -> Option<&Task> {
        self.tasks
            .iter()
            .find(|t| t.key.as_deref() == Some(key) && t.loop_id.as_deref() == loop_id)
    }

    /// Gets a task by ID (mutable reference).
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    /// Gets a task by stable key (mutable reference).
    pub fn get_by_key_mut(&mut self, key: &str) -> Option<&mut Task> {
        self.tasks
            .iter_mut()
            .find(|t| t.key.as_deref() == Some(key))
    }

    /// Closes a task by ID and returns a reference to it.
    pub fn close(&mut self, id: &str) -> Option<&Task> {
        if let Some(task) = self.get_mut(id) {
            task.status = TaskStatus::Closed;
            task.closed = Some(chrono::Utc::now().to_rfc3339());
            return self.get(id);
        }
        None
    }

    /// Starts a task by ID and returns a reference to it.
    pub fn start(&mut self, id: &str) -> Option<&Task> {
        if let Some(task) = self.get_mut(id) {
            task.start();
            return self.get(id);
        }
        None
    }

    /// Fails a task by ID and returns a reference to it.
    pub fn fail(&mut self, id: &str) -> Option<&Task> {
        if let Some(task) = self.get_mut(id) {
            task.status = TaskStatus::Failed;
            task.closed = Some(chrono::Utc::now().to_rfc3339());
            return self.get(id);
        }
        None
    }

    /// Reopens a task by ID and returns a reference to it.
    pub fn reopen(&mut self, id: &str) -> Option<&Task> {
        if let Some(task) = self.get_mut(id) {
            task.reopen();
            return self.get(id);
        }
        None
    }

    /// Ensures a task exists for a stable key, returning the existing or created task.
    ///
    /// If a task with the same key already exists, its non-lifecycle metadata is refreshed and
    /// the existing task is returned.
    ///
    /// When the candidate task has a `loop_id`, dedup is scoped to
    /// `(loop_id, key)` so that two loops sharing a tasks.jsonl do not
    /// collapse into a single row. When `loop_id` is `None` (human CLI
    /// / legacy), dedup falls back to the previous global-by-key rule.
    pub fn ensure(&mut self, task: Task) -> &Task {
        if let Some(key) = task.key.as_deref() {
            let new_loop = task.loop_id.as_deref();
            if let Some(existing_idx) = self.tasks.iter().position(|existing| {
                existing.key.as_deref() == Some(key) && existing.loop_id.as_deref() == new_loop
            }) {
                let existing = &mut self.tasks[existing_idx];
                existing.title = task.title;
                existing.priority = task.priority;
                if task.description.is_some() {
                    existing.description = task.description;
                }
                if !task.blocked_by.is_empty() {
                    existing.blocked_by = task.blocked_by;
                }
                return &self.tasks[existing_idx];
            }
        }

        // R4 (2026-06-14-003 plan): single-U contract.  When the
        // contract is enabled and the candidate's key slug matches the
        // `uN-` shape, refuse to push the task if the same step
        // already has an open task for a *different* unit.  Returns
        // the existing task so the caller can detect the collision
        // (and the agent can react by inspecting the existing
        // task's `unit` field).  Sub-units (`u1a` / `u1b` from
        // multi-unit splits) collapse to the same base `u1` and
        // are therefore allowed to coexist — that matches the
        // plan's "单 U 拆 sub-unit" carve-out.
        if self.enforce_current_unit {
            if let Some(collision_idx) = self.find_unit_collision_idx(&task) {
                return &self.tasks[collision_idx];
            }
        }

        self.tasks.push(task);
        self.tasks.last().unwrap()
    }

    /// Enable or disable the R4 single-U contract.  Idempotent; safe
    /// to call from the CLI / event loop bootstrap path.
    pub fn set_enforce_current_unit(&mut self, enabled: bool) {
        self.enforce_current_unit = enabled;
    }

    /// Returns true when the R4 contract is active.
    pub fn enforce_current_unit(&self) -> bool {
        self.enforce_current_unit
    }

    /// Look for an open task in the same `(loop_id, plan_name, step)`
    /// whose unit differs from `candidate`'s.  Returns the index of
    /// the existing task when found (caller should NOT push the
    /// candidate).  Returns `None` when the contract does not apply
    /// (no slug, legacy keys, or the same unit is already open).
    ///
    /// The function returns an index (not a reference) so the caller
    /// can hold the answer across the eventual `Vec::push` without
    /// tripping the borrow checker.
    ///
    /// `pub(crate)` so the state projector (which owns the single
    /// writer contract in Phase 1) can pre-check before delegating
    /// to [`TaskStore::ensure`]. See R1 in
    /// docs/plans/2026-06-17-005-fix-state-projection-phase1-review-findings-plan.md.
    pub(crate) fn find_unit_collision_idx(&self, candidate: &Task) -> Option<usize> {
        let candidate_key = candidate.key.as_deref()?;
        let candidate_unit = unit_from_key(candidate_key)?;
        let candidate_locus = task_locus(candidate_key)?;
        let candidate_loop = candidate.loop_id.as_deref();
        self.tasks.iter().position(|existing| {
            if existing.id == candidate.id {
                return false;
            }
            if existing.status.is_terminal() {
                return false;
            }
            if existing.loop_id.as_deref() != candidate_loop {
                return false;
            }
            let existing_key = match existing.key.as_deref() {
                Some(k) => k,
                None => return false,
            };
            let existing_unit = match unit_from_key(existing_key) {
                Some(u) => u,
                None => return false,
            };
            if existing_unit == candidate_unit {
                return false;
            }
            let existing_locus = match task_locus(existing_key) {
                Some(l) => l,
                None => return false,
            };
            existing_locus == candidate_locus
        })
    }
}

/// R4 (2026-06-14-003 plan): extract the unit identifier from a task
/// key's final slug.  Returns `None` when the slug does not match the
/// `uN-` shape so the contract silently falls through for legacy /
/// human-CLI keys.  Trailing letters (sub-units `u1a`, `u1b`) are
/// collapsed to the base unit so sub-units of the same parent unit
/// can coexist.  Examples:
///   `u1-impl` -> `u1`
///   `u1a-impl` -> `u1`
///   `u1b-impl` -> `u1`
///   `u10-impl` -> `u10`
///   `step-01-impl` -> `None`
fn unit_from_key(key: &str) -> Option<String> {
    let slug = key.rsplit(':').next()?;
    let bytes = slug.as_bytes();
    if bytes.first() != Some(&b'u') {
        return None;
    }
    let mut idx = 1;
    while idx < bytes.len() && bytes[idx].is_ascii_digit() {
        idx += 1;
    }
    if idx == 1 {
        // `u` followed by no digits is not a valid unit.
        return None;
    }
    // We intentionally drop a trailing letter so `u1a` and `u1b`
    // both collapse to `u1`; the contract only enforces the
    // parent-unit boundary, not the sub-unit boundary.
    Some(slug[..idx].to_string())
}

/// R4: derive the "step locus" from a task key.  Two keys with the
/// same locus are considered to belong to the same step.  The locus
/// is the `{plan_name}:step-XX` middle portion of the canonical key
/// shape.  Keys that do not match the canonical 4-segment shape
/// return `None` (the contract falls through to the legacy behaviour).
fn task_locus(key: &str) -> Option<String> {
    let mut parts = key.split(':');
    // Drop the prefix segment (`ce-executor`).
    let _ = parts.next()?;
    let plan = parts.next()?;
    let step = parts.next()?;
    // Canonical key has a 4th `:slug` segment.  Anything beyond that
    // is malformed for our contract — fall through to the legacy
    // behaviour so we never misclassify a foreign key.
    if parts.next().is_none() {
        return None;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(format!("{plan}:{step}"))
}

impl TaskStore {
    /// Returns all tasks as a slice.
    pub fn all(&self) -> &[Task] {
        &self.tasks
    }

    /// Returns all open tasks (not closed).
    pub fn open(&self) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|t| !t.status.is_terminal())
            .collect()
    }

    /// Returns all ready tasks (open with no pending blockers).
    pub fn ready(&self) -> Vec<&Task> {
        self.tasks
            .iter()
            .filter(|t| t.is_ready(&self.tasks))
            .collect()
    }

    /// Returns true if there are any open tasks.
    ///
    /// A task is considered open if it is not Closed. This includes Failed tasks.
    pub fn has_open_tasks(&self) -> bool {
        self.tasks.iter().any(|t| t.status != TaskStatus::Closed)
    }

    /// Returns true if there are any pending (non-terminal) tasks.
    ///
    /// A task is pending if its status is not terminal (i.e., not Closed or Failed).
    /// Use this when you need to check if there's active work remaining.
    pub fn has_pending_tasks(&self) -> bool {
        self.tasks.iter().any(|t| !t.status.is_terminal())
    }

    /// Verifies that every blocker ID refers to a known task in the same loop.
    ///
    /// Returns the list of missing or out-of-loop blocker IDs, in the
    /// order they appear in `task.blocked_by`. An empty return means
    /// all blockers are valid.
    pub fn invalid_blockers(&self, task: &Task) -> Vec<String> {
        task.blocked_by
            .iter()
            .filter(|bid| {
                !self
                    .tasks
                    .iter()
                    .any(|t| t.id == **bid && t.loop_id.as_deref() == task.loop_id.as_deref())
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_load_nonexistent_file() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let store = TaskStore::load(&path).unwrap();
        assert_eq!(store.all().len(), 0);
    }

    #[test]
    fn test_add_and_save() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");

        let mut store = TaskStore::load(&path).unwrap();
        let task = Task::new("Test task".to_string(), 1);
        store.add(task);
        store.save().unwrap();

        let loaded = TaskStore::load(&path).unwrap();
        assert_eq!(loaded.all().len(), 1);
        assert_eq!(loaded.all()[0].title, "Test task");
    }

    #[test]
    fn test_get_task() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let mut store = TaskStore::load(&path).unwrap();
        let task = Task::new("Test".to_string(), 1);
        let id = task.id.clone();
        store.add(task);

        assert!(store.get(&id).is_some());
        assert_eq!(store.get(&id).unwrap().title, "Test");
    }

    #[test]
    fn test_get_task_by_key() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let mut store = TaskStore::load(&path).unwrap();
        let task = Task::new("Test".to_string(), 1).with_key(Some("phase:design".to_string()));
        store.add(task);

        assert!(store.get_by_key("phase:design").is_some());
        assert_eq!(store.get_by_key("phase:design").unwrap().title, "Test");
    }

    #[test]
    fn test_close_task() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let mut store = TaskStore::load(&path).unwrap();
        let task = Task::new("Test".to_string(), 1);
        let id = task.id.clone();
        store.add(task);

        let closed = store.close(&id).unwrap();
        assert_eq!(closed.status, TaskStatus::Closed);
        assert!(closed.closed.is_some());
    }

    #[test]
    fn test_start_task() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let mut store = TaskStore::load(&path).unwrap();
        let task = Task::new("Test".to_string(), 1);
        let id = task.id.clone();
        store.add(task);

        let started = store.start(&id).unwrap();
        assert_eq!(started.status, TaskStatus::InProgress);
        assert!(started.started.is_some());
    }

    #[test]
    fn test_reopen_task() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let mut store = TaskStore::load(&path).unwrap();
        let task = Task::new("Test".to_string(), 1);
        let id = task.id.clone();
        store.add(task);
        store.close(&id);

        let reopened = store.reopen(&id).unwrap();
        assert_eq!(reopened.status, TaskStatus::Open);
        assert!(reopened.closed.is_none());
    }

    #[test]
    fn test_open_tasks() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let mut store = TaskStore::load(&path).unwrap();

        let task1 = Task::new("Open 1".to_string(), 1);
        store.add(task1);

        let mut task2 = Task::new("Closed".to_string(), 1);
        task2.status = TaskStatus::Closed;
        store.add(task2);

        assert_eq!(store.open().len(), 1);
    }

    #[test]
    fn test_ready_tasks() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let mut store = TaskStore::load(&path).unwrap();

        let task1 = Task::new("Ready".to_string(), 1);
        let id1 = task1.id.clone();
        store.add(task1);

        let mut task2 = Task::new("Blocked".to_string(), 1);
        task2.blocked_by.push(id1);
        store.add(task2);

        let ready = store.ready();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0].title, "Ready");
    }

    #[test]
    fn test_ensure_deduplicates_by_key() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let mut store = TaskStore::load(&path).unwrap();

        let first = Task::new("First".to_string(), 1).with_key(Some("impl:task-01".to_string()));
        let second = Task::new("Second".to_string(), 3).with_key(Some("impl:task-01".to_string()));

        let id = store.ensure(first).id.clone();
        let deduped_id = store.ensure(second).id.clone();
        let deduped = store
            .get_by_key("impl:task-01")
            .expect("deduped task should exist");

        assert_eq!(store.all().len(), 1);
        assert_eq!(deduped_id, id);
        assert_eq!(deduped.title, "Second");
        assert_eq!(deduped.priority, 3);
    }

    #[test]
    fn test_ensure_current_unit_rejects_precreation() {
        // R4 (2026-06-14-003 plan): when the single-U contract is
        // active and the candidate's key slug matches `uN-`, an
        // attempt to ensure a sibling unit in the same step must
        // NOT create a new task — `ensure` returns the existing
        // task so the caller can see why nothing was pushed.
        let mut store = TaskStore::load(std::path::Path::new("/tmp/r4-test.jsonl")).unwrap();
        store.set_enforce_current_unit(true);

        let u1 = Task::new("U1".into(), 1)
            .with_key(Some("ce-executor:my-plan:step-01:u1-impl".to_string()));
        let first = store.ensure(u1).id.clone();
        assert_eq!(store.all().len(), 1, "first ensure must create one row");

        let u2 = Task::new("U2".into(), 1)
            .with_key(Some("ce-executor:my-plan:step-01:u2-impl".to_string()));
        let rejected = store.ensure(u2);
        assert_eq!(rejected.id, first, "u2 must be rejected; u1 returned");
        assert_eq!(
            store.all().len(),
            1,
            "u2 row must NOT be created when u1 is open in the same step"
        );
    }

    #[test]
    fn test_ensure_same_unit_is_idempotent() {
        // R4.5: repeated `ensure` for the same `(loop_id, plan, step, unit)`
        // must collapse to a single row.
        let mut store = TaskStore::load(std::path::Path::new("/tmp/r4-test-idem.jsonl")).unwrap();
        store.set_enforce_current_unit(true);

        let key = "ce-executor:my-plan:step-01:u1-impl".to_string();
        let first = store
            .ensure(Task::new("U1".into(), 1).with_key(Some(key.clone())))
            .id
            .clone();
        let second = store
            .ensure(Task::new("U1 again".into(), 1).with_key(Some(key)))
            .id
            .clone();
        assert_eq!(first, second);
        assert_eq!(store.all().len(), 1);
    }

    #[test]
    fn test_ensure_subunit_allowed_in_same_u() {
        // R4 carve-out: sub-units (`u1a` / `u1b`) collapse to the
        // same base unit `u1` and are allowed to coexist.
        let mut store = TaskStore::load(std::path::Path::new("/tmp/r4-test-sub.jsonl")).unwrap();
        store.set_enforce_current_unit(true);

        let u1a = Task::new("U1a".into(), 1)
            .with_key(Some("ce-executor:my-plan:step-01:u1a-impl".to_string()));
        store.ensure(u1a);
        let u1b = Task::new("U1b".into(), 1)
            .with_key(Some("ce-executor:my-plan:step-01:u1b-impl".to_string()));
        let ok = store.ensure(u1b);
        assert_eq!(ok.title, "U1b");
        assert_eq!(store.all().len(), 2);
    }

    #[test]
    fn test_ensure_legacy_key_falls_through() {
        let mut store = TaskStore::load(std::path::Path::new("/tmp/r4-test-legacy.jsonl")).unwrap();
        store.set_enforce_current_unit(true);

        let a = Task::new("A".into(), 1).with_key(Some("step-01-impl".to_string()));
        let b = Task::new("B".into(), 1).with_key(Some("step-01-other".to_string()));
        store.ensure(a);
        store.ensure(b);
        assert_eq!(store.all().len(), 2);
    }

    #[test]
    fn test_unit_from_key_extracts_u_shapes() {
        assert_eq!(
            unit_from_key("ce-executor:p:step-01:u1-impl"),
            Some("u1".into())
        );
        // sub-units collapse to the base unit
        assert_eq!(
            unit_from_key("ce-executor:p:step-01:u1a-impl"),
            Some("u1".into())
        );
        assert_eq!(
            unit_from_key("ce-executor:p:step-01:u1b-impl"),
            Some("u1".into())
        );
        assert_eq!(
            unit_from_key("ce-executor:p:step-01:u10-impl"),
            Some("u10".into())
        );
        assert_eq!(unit_from_key("ce-executor:p:step-01:step-01-impl"), None);
        assert_eq!(unit_from_key("u1-impl"), Some("u1".into()));
        assert_eq!(unit_from_key("u-impl"), None);
        assert_eq!(unit_from_key(""), None);
    }

    #[test]
    fn test_task_locus_extraction() {
        assert_eq!(
            task_locus("ce-executor:my-plan:step-01:u1-impl"),
            Some("my-plan:step-01".into())
        );
        assert_eq!(task_locus("legacy-key"), None);
        assert_eq!(task_locus("a:b"), None);
    }

    #[test]
    fn test_has_open_tasks() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let mut store = TaskStore::load(&path).unwrap();

        assert!(!store.has_open_tasks());

        let task = Task::new("Test".to_string(), 1);
        store.add(task);

        assert!(store.has_open_tasks());
    }

    #[test]
    fn test_has_pending_tasks_excludes_failed() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let mut store = TaskStore::load(&path).unwrap();

        // Empty store has no pending tasks
        assert!(!store.has_pending_tasks());

        // Add an open task - should have pending
        let task1 = Task::new("Open task".to_string(), 1);
        store.add(task1);
        assert!(store.has_pending_tasks());

        // Close the task - should have no pending
        let id = store.all()[0].id.clone();
        store.close(&id);
        assert!(!store.has_pending_tasks());
    }

    #[test]
    fn test_has_pending_tasks_failed_is_terminal() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let mut store = TaskStore::load(&path).unwrap();

        // Add a task and fail it
        let task = Task::new("Failed task".to_string(), 1);
        store.add(task);
        let id = store.all()[0].id.clone();
        store.fail(&id);

        // Failed tasks are terminal, so no pending tasks
        assert!(!store.has_pending_tasks());

        // But has_open_tasks returns true (Failed != Closed)
        assert!(store.has_open_tasks());
    }

    #[test]
    fn test_reload() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");

        // Create and save initial store
        let mut store1 = TaskStore::load(&path).unwrap();
        store1.add(Task::new("Task 1".to_string(), 1));
        store1.save().unwrap();

        // Create second store that reads the same file
        let mut store2 = TaskStore::load(&path).unwrap();
        store2.add(Task::new("Task 2".to_string(), 1));
        store2.save().unwrap();

        // Reload first store to see changes
        store1.reload().unwrap();
        assert_eq!(store1.all().len(), 2);
    }

    #[test]
    fn test_with_exclusive_lock() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");

        let mut store = TaskStore::load(&path).unwrap();

        // Use with_exclusive_lock for atomic operation
        store
            .with_exclusive_lock(|s| {
                s.add(Task::new("Atomic task".to_string(), 1));
            })
            .unwrap();

        // Verify the task was saved
        let loaded = TaskStore::load(&path).unwrap();
        assert_eq!(loaded.all().len(), 1);
        assert_eq!(loaded.all()[0].title, "Atomic task");
    }

    #[test]
    fn test_concurrent_writes_with_lock() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let path_clone = path.clone();

        let barrier = Arc::new(Barrier::new(2));
        let barrier_clone = barrier.clone();

        // Thread 1: Add task 1
        let handle1 = thread::spawn(move || {
            let mut store = TaskStore::load(&path).unwrap();
            barrier.wait();

            store
                .with_exclusive_lock(|s| {
                    s.add(Task::new("Task from thread 1".to_string(), 1));
                })
                .unwrap();
        });

        // Thread 2: Add task 2
        let handle2 = thread::spawn(move || {
            let mut store = TaskStore::load(&path_clone).unwrap();
            barrier_clone.wait();

            store
                .with_exclusive_lock(|s| {
                    s.add(Task::new("Task from thread 2".to_string(), 1));
                })
                .unwrap();
        });

        handle1.join().unwrap();
        handle2.join().unwrap();

        // Both tasks should be present
        let final_store = TaskStore::load(tmp.path().join("tasks.jsonl").as_ref()).unwrap();
        assert_eq!(final_store.all().len(), 2);
    }

    #[test]
    fn test_load_skips_malformed_lines() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");

        // Write a file with one valid task line and some malformed lines
        let mut store = TaskStore::load(&path).unwrap();
        let task = Task::new("Valid task".to_string(), 1);
        store.add(task);
        store.save().unwrap();

        // Append malformed lines to the file
        let mut content = std::fs::read_to_string(&path).unwrap();
        content.push_str("this is not json\n");
        content.push_str("{\"broken\": true}\n");
        std::fs::write(&path, content).unwrap();

        // Load should succeed with only the valid task
        let loaded = TaskStore::load(&path).unwrap();
        assert_eq!(loaded.all().len(), 1);
        assert_eq!(loaded.all()[0].title, "Valid task");
    }

    #[test]
    fn test_get_by_key_in_loop_scopes_to_loop() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let mut store = TaskStore::load(&path).unwrap();

        let a = Task::new("A".to_string(), 1)
            .with_key(Some("shared:task".to_string()))
            .with_loop_id(Some("loop-a".to_string()));
        let b = Task::new("B".to_string(), 1)
            .with_key(Some("shared:task".to_string()))
            .with_loop_id(Some("loop-b".to_string()));
        let id_a = a.id.clone();
        store.add(a);
        store.add(b);

        let a_view = store
            .get_by_key_in_loop("shared:task", Some("loop-a"))
            .expect("loop-a entry exists");
        assert_eq!(a_view.id, id_a);

        let b_view = store
            .get_by_key_in_loop("shared:task", Some("loop-b"))
            .expect("loop-b entry exists");
        assert_ne!(a_view.id, b_view.id);

        // loop-c does not exist
        assert!(
            store
                .get_by_key_in_loop("shared:task", Some("loop-c"))
                .is_none()
        );
    }

    #[test]
    fn test_ensure_key_scoped_by_loop() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let mut store = TaskStore::load(&path).unwrap();

        let a_first = Task::new("A first".to_string(), 1)
            .with_key(Some("shared:task".to_string()))
            .with_loop_id(Some("loop-a".to_string()));
        let a_second = Task::new("A second".to_string(), 2)
            .with_key(Some("shared:task".to_string()))
            .with_loop_id(Some("loop-a".to_string()));
        let b = Task::new("B".to_string(), 1)
            .with_key(Some("shared:task".to_string()))
            .with_loop_id(Some("loop-b".to_string()));

        let a_id = store.ensure(a_first).id.clone();
        let deduped_id = store.ensure(a_second).id.clone();
        let b_id = store.ensure(b).id.clone();

        assert_eq!(a_id, deduped_id, "same loop dedup");
        assert_ne!(a_id, b_id, "different loop creates new task");
        assert_eq!(store.all().len(), 2);
    }

    #[test]
    fn test_ensure_same_key_no_loop_falls_back_to_global() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let mut store = TaskStore::load(&path).unwrap();

        let first = Task::new("First".to_string(), 1).with_key(Some("shared".to_string()));
        let second = Task::new("Second".to_string(), 2).with_key(Some("shared".to_string()));
        let id = store.ensure(first).id.clone();
        let dedup_id = store.ensure(second).id.clone();
        assert_eq!(id, dedup_id);
        assert_eq!(store.all().len(), 1);
    }

    #[test]
    fn test_invalid_blockers_detects_missing_and_cross_loop() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let mut store = TaskStore::load(&path).unwrap();

        let same_loop = Task::new("Same loop".to_string(), 1).with_loop_id(Some("loop-a".into()));
        let other_loop = Task::new("Other loop".to_string(), 1).with_loop_id(Some("loop-b".into()));
        let same_id = same_loop.id.clone();
        let other_id = other_loop.id.clone();
        store.add(same_loop);
        store.add(other_loop);

        let mut candidate =
            Task::new("Candidate".to_string(), 1).with_loop_id(Some("loop-a".into()));
        candidate.blocked_by = vec![same_id.clone(), "missing-id".into(), other_id.clone()];

        let invalid = store.invalid_blockers(&candidate);
        assert_eq!(invalid, vec!["missing-id".to_string(), other_id]);
    }

    #[test]
    fn test_invalid_blockers_empty_when_all_resolve() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("tasks.jsonl");
        let mut store = TaskStore::load(&path).unwrap();

        let blocker = Task::new("Blocker".to_string(), 1).with_loop_id(Some("loop-a".into()));
        let blocker_id = blocker.id.clone();
        store.add(blocker);

        let mut candidate =
            Task::new("Candidate".to_string(), 1).with_loop_id(Some("loop-a".into()));
        candidate.blocked_by = vec![blocker_id];

        assert!(store.invalid_blockers(&candidate).is_empty());
    }
}
