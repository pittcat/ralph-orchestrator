//! Task tracking for Ralph.
//!
//! Lightweight task tracking system inspired by Steve Yegge's Beads.
//! Provides structured task data with JSONL persistence and dependency tracking.

use serde::{Deserialize, Serialize};

/// Status of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    /// Not started
    Open,
    /// Being worked on
    InProgress,
    /// Complete
    Closed,
    /// Failed/abandoned
    Failed,
}

impl TaskStatus {
    /// Returns true if this status is terminal (Closed or Failed).
    ///
    /// Terminal statuses indicate the task is done and no longer needs attention.
    pub fn is_terminal(&self) -> bool {
        matches!(self, TaskStatus::Closed | TaskStatus::Failed)
    }
}

/// A task in the task tracking system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    /// Unique ID: task-{unix_timestamp}-{4_hex_chars}
    pub id: String,

    /// Short description
    pub title: String,

    /// Optional detailed description
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Stable key for idempotent orchestrator-managed tasks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,

    /// Current state
    pub status: TaskStatus,

    /// Priority 1-5 (1 = highest)
    pub priority: u8,

    /// Tasks that must complete before this one
    #[serde(default)]
    pub blocked_by: Vec<String>,

    /// Loop ID that created this task (from `.ralph/current-loop-id` marker).
    /// Used to filter tasks by ownership when multiple loops share a task list.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_id: Option<String>,

    /// Hat ID that created this task, when the task was emitted from an
    /// agent context. Used to authorize lifecycle operations and prevent
    /// cross-hat tampering. None for human-CLI tasks or legacy entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_hat_id: Option<String>,

    /// Creation timestamp (ISO 8601)
    pub created: String,

    /// Start timestamp (ISO 8601), if the task entered in_progress.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started: Option<String>,

    /// Completion timestamp (ISO 8601), if closed
    #[serde(skip_serializing_if = "Option::is_none")]
    pub closed: Option<String>,
}

impl Task {
    /// Creates a new task with the given title and priority.
    pub fn new(title: String, priority: u8) -> Self {
        Self {
            id: Self::generate_id(),
            title,
            description: None,
            key: None,
            status: TaskStatus::Open,
            priority: priority.clamp(1, 5),
            blocked_by: Vec::new(),
            loop_id: None,
            owner_hat_id: None,
            created: chrono::Utc::now().to_rfc3339(),
            started: None,
            closed: None,
        }
    }

    /// Sets the loop ID for this task.
    pub fn with_loop_id(mut self, loop_id: Option<String>) -> Self {
        self.loop_id = loop_id;
        self
    }

    /// Sets the owning hat ID for this task.
    pub fn with_owner_hat(mut self, owner_hat_id: Option<String>) -> Self {
        self.owner_hat_id = owner_hat_id;
        self
    }

    /// Generates a unique task ID: task-{timestamp}-{hex_suffix}
    pub fn generate_id() -> String {
        use std::time::{SystemTime, UNIX_EPOCH};
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Time went backwards");
        let timestamp = duration.as_secs();
        let hex_suffix = format!("{:04x}", duration.subsec_micros() % 0x10000);
        format!("task-{}-{}", timestamp, hex_suffix)
    }

    /// Build a fix-unit task id from a `(plan_name, fix_round,
    /// fix_unit_index)` triple, optionally combined with a Unix
    /// timestamp. The format
    /// `task-{plan_slug}-fix{round}u{unit}-{ts|hex}` is:
    ///
    /// - **Stable across retries inside one fix-unit**: two
    ///   consecutive `work.ready` emits for the same fix-unit
    ///   can be diffed by `(plan_slug, fix_round, fix_unit_index)`
    ///   without the timestamp.
    /// - **Globally unique across fix-units**: combining the
    ///   triple with `unix_ts` (or the unique id generator if
    ///   `unix_ts` is `None`) prevents the
    ///   `ce-executor-serial` primary-20260629-170451 bug,
    ///   where the coordinator's prompt template reused the
    ///   same task_id for fix-01 and fix-02 — that surfaced as a
    ///   "21 seconds and the same row in tasks.jsonl" storm.
    /// - **Aligned with `ralph_core::preset_lint` checks**: the
    ///   projector can detect "two `work.ready` events with the
    ///   same task_id but different step prefixes" and reject the
    ///   second emit loudly.
    pub fn fix_unit_task_id(
        plan_name: &str,
        fix_round: u32,
        fix_unit_index: u32,
        unix_ts: Option<u64>,
    ) -> String {
        let plan_slug = sanitize_plan_slug(plan_name);
        let ts = unix_ts.unwrap_or_else(|| {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        });
        format!("task-{plan_slug}-fix{fix_round:02}u{fix_unit_index:02}-{ts:x}")
    }

    /// Returns true if this task is ready to work on (open + no blockers pending).
    pub fn is_ready(&self, all_tasks: &[Task]) -> bool {
        if self.status != TaskStatus::Open {
            return false;
        }
        self.blocked_by.iter().all(|blocker_id| {
            all_tasks
                .iter()
                .find(|t| &t.id == blocker_id)
                .is_some_and(|t| t.status == TaskStatus::Closed)
        })
    }

    /// Sets the description of the task.
    pub fn with_description(mut self, description: Option<String>) -> Self {
        self.description = description;
        self
    }

    /// Sets the stable orchestration key for the task.
    pub fn with_key(mut self, key: Option<String>) -> Self {
        self.key = key;
        self
    }

    /// Adds a blocker task ID.
    pub fn with_blocker(mut self, task_id: String) -> Self {
        self.blocked_by.push(task_id);
        self
    }

    /// Marks the task as in progress and records a start timestamp if absent.
    pub fn start(&mut self) {
        self.status = TaskStatus::InProgress;
        if self.started.is_none() {
            self.started = Some(chrono::Utc::now().to_rfc3339());
        }
        self.closed = None;
    }

    /// Reopens a terminal task for further work.
    pub fn reopen(&mut self) {
        self.status = TaskStatus::Open;
        self.closed = None;
    }
}

/// Returns true when `caller_hat` may perform lifecycle mutations
/// (`start` / `close` / `fail` / `reopen`) on `task`.
pub fn can_hat_mutate_task_lifecycle(
    task: &Task,
    caller_hat: &str,
    coordinator_hats: &[String],
) -> bool {
    if task.owner_hat_id.as_deref() == Some(caller_hat) {
        return true;
    }
    coordinator_hats.iter().any(|h| h == caller_hat)
}

/// Pick the hat that should close `task` when `source_hat` cannot.
pub fn lifecycle_close_delegate_hat(
    task: &Task,
    source_hat: &str,
    coordinator_hats: &[String],
) -> String {
    if can_hat_mutate_task_lifecycle(task, source_hat, coordinator_hats) {
        return source_hat.to_string();
    }
    if let Some(owner) = task.owner_hat_id.as_deref() {
        if coordinator_hats.iter().any(|h| h == owner) {
            return owner.to_string();
        }
    }
    coordinator_hats
        .first()
        .cloned()
        .unwrap_or_else(|| source_hat.to_string())
}

/// Actionable denial message when `caller_hat` cannot mutate `task`.
pub fn task_lifecycle_denied_message(
    task: &Task,
    caller_hat: &str,
    coordinator_hats: &[String],
    operation: &str,
) -> String {
    let owner = task.owner_hat_id.as_deref().unwrap_or("?");
    if operation == "close" {
        let delegate = lifecycle_close_delegate_hat(task, caller_hat, coordinator_hats);
        if delegate != caller_hat {
            return format!(
                "{operation}: task {tid} is owned by hat '{owner}' and caller '{caller}' cannot close it. \
                 Ask hat '{delegate}' to run `ralph tools task close {tid}` first, then re-emit work.done with task_id={tid}.",
                operation = operation,
                tid = task.id,
                owner = owner,
                caller = caller_hat,
                delegate = delegate,
            );
        }
    }
    format!(
        "{operation}: task {tid} is owned by hat '{owner}' but caller is '{caller}' (not in coordinator_hats)",
        operation = operation,
        tid = task.id,
        owner = owner,
        caller = caller_hat,
    )
}

/// Sanitize a plan name into a slug safe to embed in a task_id.
/// Concretely, lower-case the string and replace anything that
/// is not an ASCII alnum with `_`. This is used by
/// [`Task::fix_unit_task_id`] so the generator can fold over
/// arbitrary plan names without breaking the id format. The
/// `ce-executor-serial` prompt template keeps `plan_name`
/// ASCII-clean for the same reason.
fn sanitize_plan_slug(plan_name: &str) -> String {
    let mut buf = String::with_capacity(plan_name.len());
    for ch in plan_name.chars() {
        if ch.is_ascii_alphanumeric() {
            buf.push(ch.to_ascii_lowercase());
        } else if !buf.ends_with('_') {
            buf.push('_');
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_creation() {
        let task = Task::new("Test task".to_string(), 2);
        assert_eq!(task.title, "Test task");
        assert_eq!(task.priority, 2);
        assert_eq!(task.status, TaskStatus::Open);
        assert!(task.blocked_by.is_empty());
        assert!(task.key.is_none());
        assert!(task.started.is_none());
        assert!(task.loop_id.is_none());
        assert!(task.owner_hat_id.is_none());
    }

    #[test]
    fn test_priority_clamping() {
        let task_low = Task::new("Low".to_string(), 0);
        assert_eq!(task_low.priority, 1);

        let task_high = Task::new("High".to_string(), 10);
        assert_eq!(task_high.priority, 5);
    }

    #[test]
    fn test_task_id_format() {
        let task = Task::new("Test".to_string(), 1);
        assert!(task.id.starts_with("task-"));
        let parts: Vec<&str> = task.id.split('-').collect();
        assert_eq!(parts.len(), 3);
    }

    #[test]
    fn test_is_ready_open_no_blockers() {
        let task = Task::new("Test".to_string(), 1);
        assert!(task.is_ready(&[]));
    }

    #[test]
    fn test_is_ready_with_open_blocker() {
        let blocker = Task::new("Blocker".to_string(), 1);
        let mut task = Task::new("Test".to_string(), 1);
        task.blocked_by.push(blocker.id.clone());

        assert!(!task.is_ready(std::slice::from_ref(&blocker)));
    }

    #[test]
    fn test_is_ready_with_closed_blocker() {
        let mut blocker = Task::new("Blocker".to_string(), 1);
        blocker.status = TaskStatus::Closed;

        let mut task = Task::new("Test".to_string(), 1);
        task.blocked_by.push(blocker.id.clone());

        assert!(task.is_ready(std::slice::from_ref(&blocker)));
    }

    #[test]
    fn test_is_not_ready_when_not_open() {
        let mut task = Task::new("Test".to_string(), 1);
        task.status = TaskStatus::Closed;
        assert!(!task.is_ready(&[]));

        task.status = TaskStatus::InProgress;
        assert!(!task.is_ready(&[]));

        task.status = TaskStatus::Failed;
        assert!(!task.is_ready(&[]));
    }

    #[test]
    fn test_is_terminal() {
        assert!(!TaskStatus::Open.is_terminal());
        assert!(!TaskStatus::InProgress.is_terminal());
        assert!(TaskStatus::Closed.is_terminal());
        assert!(TaskStatus::Failed.is_terminal());
    }

    #[test]
    fn test_with_key_sets_stable_key() {
        let task = Task::new("Test".to_string(), 1).with_key(Some("spec:build".to_string()));
        assert_eq!(task.key.as_deref(), Some("spec:build"));
    }

    #[test]
    fn test_with_owner_hat_stamps_owner() {
        let task = Task::new("Test".to_string(), 1).with_owner_hat(Some("executor".to_string()));
        assert_eq!(task.owner_hat_id.as_deref(), Some("executor"));
    }

    #[test]
    fn lifecycle_close_delegate_routes_coordinator_owned_task_to_coordinator() {
        let task = Task::new("t1".to_string(), 1).with_owner_hat(Some("coordinator".to_string()));
        let coordinators = vec!["coordinator".to_string()];
        assert!(!can_hat_mutate_task_lifecycle(
            &task,
            "executor",
            &coordinators
        ));
        assert_eq!(
            lifecycle_close_delegate_hat(&task, "executor", &coordinators),
            "coordinator"
        );
    }

    #[test]
    fn lifecycle_close_delegate_keeps_owner_executor() {
        let task = Task::new("t1".to_string(), 1).with_owner_hat(Some("executor".to_string()));
        let coordinators = vec!["coordinator".to_string()];
        assert!(can_hat_mutate_task_lifecycle(
            &task,
            "executor",
            &coordinators
        ));
        assert_eq!(
            lifecycle_close_delegate_hat(&task, "executor", &coordinators),
            "executor"
        );
    }

    #[test]
    fn task_lifecycle_denied_message_close_mentions_delegate_coordinator() {
        let task =
            Task::new("task-1".to_string(), 1).with_owner_hat(Some("coordinator".to_string()));
        let msg =
            task_lifecycle_denied_message(&task, "executor", &["coordinator".to_string()], "close");
        assert!(msg.contains("Ask hat 'coordinator'"));
        assert!(msg.contains("ralph tools task close task-1"));
    }

    #[test]
    fn test_with_loop_id_stamps_loop() {
        let task = Task::new("Test".to_string(), 1).with_loop_id(Some("loop-x".to_string()));
        assert_eq!(task.loop_id.as_deref(), Some("loop-x"));
    }

    #[test]
    fn test_legacy_task_without_owner_hat_deserializes() {
        let json = r#"{
            "id": "task-1234-abcd",
            "title": "Legacy",
            "status": "open",
            "priority": 3,
            "created": "2026-01-01T00:00:00Z"
        }"#;
        let task: Task = serde_json::from_str(json).expect("legacy task should parse");
        assert!(task.loop_id.is_none());
        assert!(task.owner_hat_id.is_none());
    }

    #[test]
    fn test_start_marks_task_in_progress() {
        let mut task = Task::new("Test".to_string(), 1);
        task.start();
        assert_eq!(task.status, TaskStatus::InProgress);
        assert!(task.started.is_some());
        assert!(task.closed.is_none());
    }

    #[test]
    fn test_reopen_resets_terminal_state() {
        let mut task = Task::new("Test".to_string(), 1);
        task.status = TaskStatus::Closed;
        task.closed = Some(chrono::Utc::now().to_rfc3339());
        task.reopen();
        assert_eq!(task.status, TaskStatus::Open);
        assert!(task.closed.is_none());
    }

    // 2026-06-30 P0-3 (primary-20260629-170451 diagnosis):
    // The coordinator's prompt template reused the same
    // `task_id` for fix-01 and fix-02 — produce rows in the
    // tasks.jsonl but two distinct `task_key`s routed to the
    // projector in two separate `work.ready` events. The
    // dedup helpers `TaskStore::ensure` now anchor on
    // `(loop_id, task_key)`, so the projection does not in
    // fact create two rows any more — but the runtime
    // tracking in `LoopState` (which key is on
    // `(plan_name, step, task_id)`) loses per-fix-unit
    // identity, so the counter resets between fix units. The
    // remediation is to give the coordinator a deterministic
    // id generator that mints a fresh id per fix-unit tuple;
    // the projector then enforces "one task per
    // `(plan, fix_round, fix_unit_index)`" by rejecting
    // re-emission of the same id with a different key.
    #[test]
    fn test_fix_unit_task_id_is_unique_per_triple() {
        let id_a = Task::fix_unit_task_id("ce-executor-serial", 1, 1, Some(0x1234));
        let id_b = Task::fix_unit_task_id("ce-executor-serial", 1, 2, Some(0x1234));
        let id_c = Task::fix_unit_task_id("ce-executor-serial", 2, 1, Some(0x1234));
        assert!(
            id_a.starts_with("task-ce_executor_serial-fix01u01-"),
            "id_a: {id_a}"
        );
        assert!(
            id_b.starts_with("task-ce_executor_serial-fix01u02-"),
            "id_b: {id_b}"
        );
        assert!(
            id_c.starts_with("task-ce_executor_serial-fix02u01-"),
            "id_c: {id_c}"
        );
        let ids = vec![id_a.clone(), id_b.clone(), id_c.clone()];
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(
            unique.len(),
            3,
            "fix_round × fix_unit_index must yield three distinct ids, got {ids:?}"
        );
    }

    #[test]
    fn test_fix_unit_task_id_handles_unicode_plan_name() {
        // Non-alphanumeric chars collapse into a single `_`
        // (the sanitizer skips duplicates), then `001` is kept
        // verbatim. The slug therefore ends as `_001`.
        let id = Task::fix_unit_task_id("中文方案-001", 0, 0, Some(1));
        assert!(
            id.starts_with("task-_001"),
            "expected unicode plan name to slug to `_001`, got {id}"
        );
        assert!(id.ends_with("-fix00u00-1"), "got {id}");
    }
}
