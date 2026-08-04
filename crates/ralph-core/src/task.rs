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

/// Confirmation lifecycle of a gate-protected task mutation.
///
/// A protected Apply (agent `task add` / `task ensure` with the
/// verify gate active) records a `Pending` confirmation on the task
/// row; the same loop/hat must consume it via
/// `ralph tools task confirm` before the next protected mutation is
/// admitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationState {
    /// Recorded by a successful protected Apply, not yet confirmed.
    Pending,
    /// Consumed by a matching `task confirm` invocation.
    Confirmed,
}

/// Result of matching `task confirm` arguments against a stored
/// [`TaskConfirmation`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmMatch {
    /// Reference, digest and scope all match and the state is
    /// `Pending` — transition to `Confirmed`.
    Apply,
    /// Reference, digest and scope all match and the state is
    /// already `Confirmed` — idempotent no-op (no disk rewrite).
    /// A cross-scope repeat of a confirmed record is a [`Self::Mismatch`].
    AlreadyConfirmed,
    /// The reference matches but the digest or the loop/hat scope
    /// differs — the state stays as recorded.
    Mismatch,
    /// The reference does not match — nothing to confirm here.
    Unavailable,
}

/// Confirmation record attached to a task row by a gate-protected
/// Apply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskConfirmation {
    /// Lifecycle state (`pending` until a matching confirm runs).
    pub state: ConfirmationState,
    /// Unique reference minted at Apply time; the confirming caller
    /// must present it verbatim.
    pub reference: String,
    /// Mutation fingerprint (SHA-256 of verb + canonical payload +
    /// loop + hat) recorded at Apply time.
    pub digest: String,
    /// Loop the protected mutation ran in (empty string when absent,
    /// mirroring the verify-gate identifier convention).
    pub loop_id: String,
    /// Hat the protected mutation ran in (empty string when absent).
    pub hat_id: String,
    /// RFC3339 timestamp of the Apply that recorded the confirmation.
    pub created: String,
}

impl TaskConfirmation {
    /// Mint a fresh `Pending` confirmation for a protected Apply.
    ///
    /// The reference is a `cfm-` prefixed UUIDv4 hex — the same
    /// uniqueness class as [`Task::generate_id`] collision-wise, but
    /// independent of the clock so two Applies inside the same
    /// microsecond still get distinct references.
    pub fn new_pending(digest: String, loop_id: String, hat_id: String) -> Self {
        Self {
            state: ConfirmationState::Pending,
            reference: format!("cfm-{}", uuid::Uuid::new_v4().simple()),
            digest,
            loop_id,
            hat_id,
            created: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Match `task confirm` arguments against this record without
    /// mutating it. Pure value-object transition logic; the CLI maps
    /// the outcome to exit codes and stable reason tokens.
    ///
    /// Scope is checked on every match, including the idempotent
    /// `AlreadyConfirmed` branch: a record recorded by one loop/hat
    /// can only be confirmed (or idempotently re-confirmed) by the
    /// same loop/hat. Cross-scope attempts with the right reference
    /// and digest are a [`ConfirmMatch::Mismatch`].
    pub fn match_confirm(
        &self,
        reference: &str,
        digest: &str,
        loop_id: &str,
        hat_id: &str,
    ) -> ConfirmMatch {
        if self.reference != reference {
            return ConfirmMatch::Unavailable;
        }
        if self.digest != digest {
            return ConfirmMatch::Mismatch;
        }
        if self.loop_id != loop_id || self.hat_id != hat_id {
            return ConfirmMatch::Mismatch;
        }
        match self.state {
            ConfirmationState::Confirmed => ConfirmMatch::AlreadyConfirmed,
            ConfirmationState::Pending => ConfirmMatch::Apply,
        }
    }

    /// Transition to `Confirmed`. Callers must only invoke this after
    /// [`Self::match_confirm`] returned [`ConfirmMatch::Apply`].
    pub fn mark_confirmed(&mut self) {
        self.state = ConfirmationState::Confirmed;
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

    /// Confirmation record written by a gate-protected Apply
    /// (`ralph tools task add` / `ensure` with the verify gate
    /// active for the calling agent). Absent for human-CLI tasks,
    /// bypassed mutations, and legacy rows.
    ///
    /// Boxed so the optional record does not bloat every `Task`
    /// (the state ledger's `CommitDelta::TaskInserted` variant holds
    /// a whole `Task` and clippy's `large_enum_variant` gate is
    /// ratio-sensitive). Serde treats `Box<T>` transparently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<Box<TaskConfirmation>>,

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
            confirmation: None,
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

    /// Transitions the task to the closed terminal state and records
    /// the close timestamp.
    ///
    /// This is the single shared "what a close writes" mutation used
    /// by every close path (`TaskStore::close`, `close_by_key`, and
    /// the batch settlement projection) so the close ledger shape
    /// cannot drift between them. It deliberately does NOT touch
    /// `started`: the decision of whether a never-started row is
    /// defensively started first belongs to the caller's guard
    /// (`TaskStore::close` rejects a never-started non-fix-unit;
    /// the batch path defensively starts non-fix-units but never
    /// fix-units). Fix-unit rows must always be closed with
    /// `started` left untouched.
    pub fn mark_closed(&mut self) {
        self.status = TaskStatus::Closed;
        self.closed = Some(chrono::Utc::now().to_rfc3339());
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
    if let Some(owner) = task.owner_hat_id.as_deref()
        && coordinator_hats.iter().any(|h| h == owner)
    {
        return owner.to_string();
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

    // ── Unit 1 (task confirmation): serde contract ──────────────────

    #[test]
    fn test_legacy_task_row_without_confirmation_parses_as_none() {
        let json = r#"{
            "id": "task-1234-abcd",
            "title": "Legacy",
            "status": "open",
            "priority": 3,
            "created": "2026-01-01T00:00:00Z"
        }"#;
        let task: Task = serde_json::from_str(json).expect("legacy row parses");
        assert!(
            task.confirmation.is_none(),
            "legacy rows must never grow a confirmation on parse"
        );
    }

    #[test]
    fn test_confirmation_round_trip_keeps_all_fields() {
        let mut task = Task::new("Protected row".to_string(), 2);
        task.confirmation = Some(Box::new(TaskConfirmation::new_pending(
            "digest-abc".to_string(),
            "loop-a".to_string(),
            "coordinator".to_string(),
        )));
        let reference = task
            .confirmation
            .as_ref()
            .expect("confirmation set")
            .reference
            .clone();

        let raw = serde_json::to_string(&task).expect("serialize");
        let parsed: Task = serde_json::from_str(&raw).expect("deserialize");
        let cfm = parsed
            .confirmation
            .expect("confirmation survives round-trip");
        assert_eq!(cfm.state, ConfirmationState::Pending);
        assert_eq!(cfm.reference, reference);
        assert_eq!(cfm.digest, "digest-abc");
        assert_eq!(cfm.loop_id, "loop-a");
        assert_eq!(cfm.hat_id, "coordinator");
        assert!(!cfm.created.is_empty());
    }

    #[test]
    fn test_confirmation_without_state_field_fails_closed() {
        // A row whose confirmation object lacks `state` must not parse
        // into any ConfirmationState (there is no serde default) — the
        // store's lenient line parser then skips it rather than
        // inventing a confirmed record.
        let json = r#"{
            "id": "task-1234-abcd",
            "title": "Broken",
            "status": "open",
            "priority": 3,
            "created": "2026-01-01T00:00:00Z",
            "confirmation": {
                "reference": "cfm-x",
                "digest": "d",
                "loop_id": "loop-a",
                "hat_id": "coordinator",
                "created": "2026-01-01T00:00:00Z"
            }
        }"#;
        let parsed = serde_json::from_str::<Task>(json);
        assert!(
            parsed.is_err(),
            "missing confirmation.state must fail closed, never default to confirmed"
        );
    }

    // ── Unit 1 (task confirmation): pure transition logic ───────────

    #[test]
    fn test_confirm_match_pending_to_confirmed_is_single_transition() {
        let mut cfm = TaskConfirmation::new_pending(
            "digest-1".to_string(),
            "loop-a".to_string(),
            "coordinator".to_string(),
        );
        let reference = cfm.reference.clone();
        assert_eq!(
            cfm.match_confirm(&reference, "digest-1", "loop-a", "coordinator"),
            ConfirmMatch::Apply
        );
        cfm.mark_confirmed();
        assert_eq!(cfm.state, ConfirmationState::Confirmed);
        // Repeat confirm with matching reference + digest is idempotent.
        assert_eq!(
            cfm.match_confirm(&reference, "digest-1", "loop-a", "coordinator"),
            ConfirmMatch::AlreadyConfirmed
        );
    }

    #[test]
    fn test_confirm_match_cross_scope_on_confirmed_is_mismatch() {
        // The idempotent AlreadyConfirmed branch is scoped too: a
        // confirmed record can only be re-confirmed by the loop/hat
        // that recorded it. A cross-scope repeat with the exact
        // reference + digest must surface as Mismatch, never as a
        // silent idempotent success.
        let mut cfm = TaskConfirmation::new_pending(
            "digest-1".to_string(),
            "loop-a".to_string(),
            "coordinator".to_string(),
        );
        let reference = cfm.reference.clone();
        cfm.mark_confirmed();

        // Different loop.
        assert_eq!(
            cfm.match_confirm(&reference, "digest-1", "loop-b", "coordinator"),
            ConfirmMatch::Mismatch
        );
        // Different hat.
        assert_eq!(
            cfm.match_confirm(&reference, "digest-1", "loop-a", "executor"),
            ConfirmMatch::Mismatch
        );
        // Same-scope repeat stays idempotent (exit-0 semantics).
        assert_eq!(
            cfm.match_confirm(&reference, "digest-1", "loop-a", "coordinator"),
            ConfirmMatch::AlreadyConfirmed
        );
        assert_eq!(cfm.state, ConfirmationState::Confirmed);
    }

    #[test]
    fn test_confirm_match_digest_mismatch_keeps_pending() {
        let cfm = TaskConfirmation::new_pending(
            "digest-1".to_string(),
            "loop-a".to_string(),
            "coordinator".to_string(),
        );
        let reference = cfm.reference.clone();
        assert_eq!(
            cfm.match_confirm(&reference, "digest-other", "loop-a", "coordinator"),
            ConfirmMatch::Mismatch
        );
        assert_eq!(cfm.state, ConfirmationState::Pending);
    }

    #[test]
    fn test_confirm_match_scope_mismatch_keeps_pending() {
        let cfm = TaskConfirmation::new_pending(
            "digest-1".to_string(),
            "loop-a".to_string(),
            "coordinator".to_string(),
        );
        let reference = cfm.reference.clone();
        // Different loop.
        assert_eq!(
            cfm.match_confirm(&reference, "digest-1", "loop-b", "coordinator"),
            ConfirmMatch::Mismatch
        );
        // Different hat.
        assert_eq!(
            cfm.match_confirm(&reference, "digest-1", "loop-a", "executor"),
            ConfirmMatch::Mismatch
        );
        assert_eq!(cfm.state, ConfirmationState::Pending);
    }

    #[test]
    fn test_confirm_match_wrong_reference_is_unavailable() {
        let cfm = TaskConfirmation::new_pending(
            "digest-1".to_string(),
            "loop-a".to_string(),
            "coordinator".to_string(),
        );
        assert_eq!(
            cfm.match_confirm("cfm-does-not-exist", "digest-1", "loop-a", "coordinator"),
            ConfirmMatch::Unavailable
        );
        assert_eq!(cfm.state, ConfirmationState::Pending);
    }

    #[test]
    fn test_confirmation_references_are_unique() {
        let a = TaskConfirmation::new_pending("d".to_string(), "l".to_string(), "h".to_string());
        let b = TaskConfirmation::new_pending("d".to_string(), "l".to_string(), "h".to_string());
        assert_ne!(a.reference, b.reference, "each Apply mints a fresh reference");
        assert!(a.reference.starts_with("cfm-"));
    }
}
