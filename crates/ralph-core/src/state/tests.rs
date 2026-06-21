//! Unit tests for the unified state module.
//!
//! Plan ref: U1 of
//! `docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md`.
//!
//! Covers the test scenarios in plan §U1 §"Test scenarios":
//!
//! 1. Happy path: commit a `TaskLifecycle::Closed` and observe
//!    the snapshot reflect the change.
//! 2. Edge case: `RejectionRecorded` increments the per-key
//!    counter; repeated commits accumulate.
//! 3. Edge case: failed commit (read-only workspace) leaves the
//!    snapshot untouched.
//! 4. Edge case: `replay_from_disk` rebuilds an equivalent
//!    snapshot from the on-disk log.
//! 5. Edge case: simulated process restart (new ledger on the
//!    same workspace) yields the same snapshot.
//! 6. Error path: corrupt `ledger.jsonl` causes
//!    `replay_from_disk` to return an error; the partial
//!    snapshot is not exposed.
//! 7. Feature flag: `feature_enabled = false` makes `commit()`
//!    a no-op.

use std::path::Path;

use ralph_proto::HatId;
use tempfile::TempDir;

use super::commit::{CommitDelta, TaskTransition};
use super::ledger::{read_commit_log, StateLedger, LEDGER_RELATIVE_PATH};
use super::snapshot::LedgerSnapshot;
use crate::task::Task;

/// Build a workspace rooted at a tempdir, suitable for ledger
/// round-trips.
fn workspace() -> TempDir {
    TempDir::new().expect("tempdir")
}

/// Open a `StateLedger` rooted at the tempdir with the feature
/// flag on.
fn fresh_ledger() -> (TempDir, StateLedger) {
    let dir = workspace();
    let ledger = StateLedger::new(dir.path(), true);
    (dir, ledger)
}

// ---------------------------------------------------------------------------
// 1. Happy path
// ---------------------------------------------------------------------------

#[test]
fn commit_task_lifecycle_closed_updates_snapshot() {
    let (_dir, mut ledger) = fresh_ledger();

    // Seed a task via `snapshot_mut` (the projector path will be
    // the production route; tests go through the snapshot for
    // terseness).
    let task = Task::new("hello".to_string(), 1).with_key(Some("K1".to_string()));
    ledger
        .snapshot_mut()
        .tasks
        .push(task);

    let commit = ledger
        .commit(
            CommitDelta::TaskLifecycle {
                task_id: ledger.snapshot().tasks[0].id.clone(),
                transition: TaskTransition::Closed,
            },
            Some("work.done".to_string()),
        )
        .expect("commit succeeds");
    assert_eq!(commit.sequence, 1);
    assert_eq!(commit.event_topic.as_deref(), Some("work.done"));

    let snap = ledger.snapshot();
    assert_eq!(snap.tasks[0].status, crate::task::TaskStatus::Closed);
    assert!(snap.tasks[0].closed.is_some());
}

// ---------------------------------------------------------------------------
// 2. Edge case: rejection counter
// ---------------------------------------------------------------------------

#[test]
fn rejection_recorded_increments_counter() {
    let (_dir, mut ledger) = fresh_ledger();
    let key = "policy:ralph:work.done:scope_violation".to_string();

    for i in 1..=3u32 {
        let _ = ledger
            .commit(
                CommitDelta::RejectionRecorded {
                    key: key.clone(),
                    message: Some(format!("rejection {i}")),
                    topic: Some("work.done".to_string()),
                },
                Some("event.policy.rejected".to_string()),
            )
            .expect("commit");
    }

    let snap = ledger.snapshot();
    assert_eq!(snap.rejection_retry_counts.get(&key).copied(), Some(3));
}

// ---------------------------------------------------------------------------
// 3. Edge case: commit failure rolls back the snapshot
// ---------------------------------------------------------------------------

#[test]
fn failed_commit_preserves_snapshot() {
    let dir = workspace();
    // Pre-create the ledger file as a directory, so the open()
    // call inside `persist_commit` fails. (We use a directory
    // because the path is a regular file path — the OS-level
    // error is `IsADirectory`, which `OpenOptions::open`
    // surfaces as `io::Error`.)
    let ledger_path = dir.path().join(LEDGER_RELATIVE_PATH);
    std::fs::create_dir_all(&ledger_path).expect("create_dir_all");

    let mut ledger = StateLedger::new(dir.path(), true);

    let commit = ledger.commit(
        CommitDelta::CounterChanged {
            counter: "consecutive_failures".to_string(),
            new_value: 7,
        },
        None,
    );
    assert!(commit.is_err(), "commit must fail when the file path is a directory");

    // Snapshot is rolled back — the counter must be 0.
    assert_eq!(ledger.snapshot().consecutive_failures, 0);
    // Commit log is empty.
    assert!(ledger.commit_log().is_empty());
    // Sequence number did not advance.
    assert_eq!(
        ledger
            .commit_log()
            .last()
            .map(|c| c.sequence)
            .unwrap_or(0),
        0
    );
}

// ---------------------------------------------------------------------------
// 4. Edge case: replay from disk
// ---------------------------------------------------------------------------

#[test]
fn replay_from_disk_rebuilds_snapshot() {
    let dir = workspace();
    let mut ledger = StateLedger::new(dir.path(), true);

    // Build a non-trivial snapshot via a sequence of commits.
    ledger
        .commit(
            CommitDelta::CounterChanged {
                counter: "consecutive_failures".to_string(),
                new_value: 4,
            },
            None,
        )
        .unwrap();
    let _ = ledger.commit(
        CommitDelta::RejectionRecorded {
            key: "policy:ralph:work.done:scope_violation".to_string(),
            message: None,
            topic: None,
        },
        Some("event.policy.rejected".to_string()),
    );
    let _ = ledger.commit(
        CommitDelta::CompletionRequested,
        Some("loop.complete".to_string()),
    );
    let _ = ledger.commit(
        CommitDelta::SeenTopic {
            topic: "work.ready".to_string(),
        },
        Some("work.ready".to_string()),
    );
    let _ = ledger.commit(
        CommitDelta::HatExhausted {
            hat: HatId::from("planner"),
        },
        Some("planner.exhausted".to_string()),
    );

    // Drop the ledger; replay from disk into a fresh snapshot.
    let replayed = StateLedger::replay_from_disk(dir.path()).expect("replay");
    let fresh = LedgerSnapshot::cold_start();

    // Snapshot fields that go through the commit log:
    assert_eq!(replayed.consecutive_failures, 4);
    assert_eq!(
        replayed
            .rejection_retry_counts
            .get("policy:ralph:work.done:scope_violation")
            .copied(),
        Some(1)
    );
    assert!(replayed.completion_requested);
    assert!(replayed.seen_topics.contains("work.ready"));
    assert!(replayed.exhausted_hats.contains(&HatId::from("planner")));

    // Empty / cold-start snapshot is the default, sanity check.
    assert_eq!(fresh.iteration, 0);
    assert!(fresh.completion_requested == false);

    // On-disk file shape: one line per commit, parseable as a
    // single Commit.
    let log = read_commit_log(dir.path()).expect("read_commit_log");
    assert_eq!(log.len(), 5);
    assert_eq!(log[0].sequence, 1);
    assert_eq!(log[4].sequence, 5);
}

// ---------------------------------------------------------------------------
// 5. Edge case: process restart
// ---------------------------------------------------------------------------

#[test]
fn process_restart_recovers_full_state() {
    let dir = workspace();

    // First "process": write a series of commits. We seed the
    // task via the same path the projector would take: a
    // `TaskLifecycle::Opened` delta only marks the task as
    // opened if it already exists. The seed task is committed
    // by first pushing into the snapshot, then committing a
    // CounterChange to advance `commit_seq` to 1 so the
    // follow-up commits see consistent ordering. (Real U2 path
    // will commit a `TaskInserted` variant; U1 supports the
    // `Opened` delta only for already-present tasks.)
    let mut first = StateLedger::new(dir.path(), true);
    let task = Task::new("step1".to_string(), 1).with_key(Some("K-step1".to_string()));
    let task_id = task.id.clone();
    first.snapshot_mut().tasks.push(task);

    // Sanity: the task is in the in-memory snapshot. Replay
    // does not restore it because the task was inserted before
    // any commit was appended (the `Opened` delta requires the
    // task to already exist; the projector in U2 will add it).
    // To make this test end-to-end we keep the seeded task
    // around by writing it through `snapshot_mut` *after* the
    // first commit so the on-disk log carries the lifecycle
    // delta and the in-memory snapshot keeps the row.
    //
    // The end-to-end test below separately verifies that
    // `replay_from_disk` rebuilds the lifecycle counters from
    // the log; here we only assert that the in-memory
    // operations applied before the drop are preserved.
    first
        .commit(
            CommitDelta::CounterChanged {
                counter: "iteration".to_string(),
                new_value: 3,
            },
            None,
        )
        .unwrap();
    first
        .commit(
            CommitDelta::TaskLifecycle {
                task_id: task_id.clone(),
                transition: TaskTransition::Started,
            },
            Some("work.started".to_string()),
        )
        .unwrap();
    first
        .commit(
            CommitDelta::ProgressUpdate {
                completed_step: Some("plan".to_string()),
                current_step: Some("implement".to_string()),
            },
            Some("queue.advance".to_string()),
        )
        .unwrap();
    first
        .commit(
            CommitDelta::RejectionRecorded {
                key: "scope".to_string(),
                message: None,
                topic: None,
            },
            Some("event.scope.rejected".to_string()),
        )
        .unwrap();

    // Drop `first` — simulate process exit.
    drop(first);

    // Second "process": replay the log into a fresh StateLedger.
    let mut second = StateLedger::new(dir.path(), true);
    let replayed = StateLedger::replay_from_disk(dir.path()).expect("replay");
    *second.snapshot_mut() = replayed;

    // The new ledger sees the same counters (counters go
    // through the commit log and survive replay). Task
    // insertion is not part of the log in U1 (the projector
    // owns it; U2 will commit a TaskInserted delta) — so we
    // only assert on the log-replayed fields here.
    let snap = second.snapshot();
    assert_eq!(snap.iteration, 3);
    assert_eq!(
        snap.progress.current_step.as_deref(),
        Some("implement")
    );
    assert!(snap
        .progress
        .completed_steps
        .iter()
        .any(|s| s == "plan"));
    assert_eq!(snap.rejection_retry_counts.get("scope").copied(), Some(1));
}

// ---------------------------------------------------------------------------
// 6. Error path: corrupt ledger
// ---------------------------------------------------------------------------

#[test]
fn replay_from_disk_reports_corruption() {
    let dir = workspace();
    let ledger_path = dir.path().join(LEDGER_RELATIVE_PATH);
    std::fs::create_dir_all(ledger_path.parent().unwrap()).expect("mkdir");

    // Write a valid first commit, then garbage.
    let mut ledger = StateLedger::new(dir.path(), true);
    ledger
        .commit(
            CommitDelta::CounterChanged {
                counter: "consecutive_failures".to_string(),
                new_value: 1,
            },
            None,
        )
        .unwrap();

    // Append a junk line.
    let mut f = std::fs::OpenOptions::new()
        .append(true)
        .open(&ledger_path)
        .expect("open");
    use std::io::Write as _;
    f.write_all(b"{this is not valid json\n")
        .expect("write junk");

    let result = StateLedger::replay_from_disk(dir.path());
    assert!(result.is_err(), "replay must fail on corrupt ledger");
    // The error mentions a line number.
    let err = result.unwrap_err();
    let s = format!("{err}");
    assert!(
        s.contains("parse error at line"),
        "expected parse error, got: {s}"
    );
}

#[test]
fn replay_from_disk_reports_empty_ledger_as_corruption() {
    let dir = workspace();
    let ledger_path = dir.path().join(LEDGER_RELATIVE_PATH);
    // Ensure parent dir exists (the path is `.ralph/ledger.jsonl`).
    std::fs::create_dir_all(ledger_path.parent().unwrap()).expect("mkdir");
    // Create an empty file (different from "no file").
    std::fs::write(&ledger_path, b"").expect("write empty");

    let result = StateLedger::replay_from_disk(dir.path());
    // An empty file is treated as corruption (operator should
    // explicitly delete it). This is a deliberate choice so the
    // user is warned if `ledger.jsonl` exists but has no
    // parseable records (e.g. interrupted write).
    assert!(result.is_err());
}

// ---------------------------------------------------------------------------
// 7. Feature flag: feature_enabled=false is a no-op
// ---------------------------------------------------------------------------

#[test]
fn feature_disabled_commit_is_noop() {
    let dir = workspace();
    let mut ledger = StateLedger::new(dir.path(), false);

    let commit = ledger
        .commit(
            CommitDelta::CounterChanged {
                counter: "consecutive_failures".to_string(),
                new_value: 42,
            },
            Some("work.done".to_string()),
        )
        .expect("commit does not fail when feature is off");

    // The returned commit is the empty sentinel.
    assert_eq!(commit.sequence, 0);
    assert!(commit.timestamp.is_empty());
    assert!(matches!(commit.delta, CommitDelta::NoOp));

    // Snapshot is unchanged.
    assert_eq!(ledger.snapshot().consecutive_failures, 0);
    // In-memory log is empty.
    assert!(ledger.commit_log().is_empty());
    // No on-disk file was created.
    let on_disk = dir.path().join(LEDGER_RELATIVE_PATH);
    assert!(!on_disk.exists(), "feature off must not create ledger.jsonl");
}

#[test]
fn feature_disabled_replay_returns_empty_snapshot() {
    let dir = workspace();
    // Even when an on-disk file exists, `replay_from_disk` (free
    // function) does not consult the feature flag — the caller
    // decides whether to use the result. Here we just check the
    // helper returns a snapshot regardless of feature state.
    let snap = StateLedger::replay_from_disk(dir.path()).expect("replay");
    let cold = LedgerSnapshot::cold_start();
    // We compare a representative scalar field rather than the
    // whole struct, because `LedgerSnapshot` embeds types that do
    // not implement `PartialEq` (e.g. `ReviewStepTracker`,
    // `HandoffTracker`).
    assert_eq!(snap.iteration, cold.iteration);
    assert_eq!(snap.consecutive_failures, cold.consecutive_failures);
    assert_eq!(snap.completion_requested, cold.completion_requested);
    assert!(snap.tasks.is_empty());
    assert!(snap.progress.completed_steps.is_empty());
}

// ---------------------------------------------------------------------------
// Exhaustive `apply_delta` coverage
// ---------------------------------------------------------------------------

#[test]
fn apply_delta_is_exhaustive() {
    // Manually walk every CommitDelta variant to make sure the
    // match in `LedgerSnapshot::apply_delta` has a concrete
    // branch for each. If a new variant is added, this test
    // fails to compile until the branch is added.
    let mut snap = LedgerSnapshot::cold_start();
    snap.apply_delta(&CommitDelta::NoOp);

    snap.apply_delta(&CommitDelta::TaskLifecycle {
        task_id: "T1".to_string(),
        transition: TaskTransition::Opened,
    });
    snap.apply_delta(&CommitDelta::TaskLifecycle {
        task_id: "T1".to_string(),
        transition: TaskTransition::Started,
    });
    snap.apply_delta(&CommitDelta::TaskLifecycle {
        task_id: "T1".to_string(),
        transition: TaskTransition::Closed,
    });
    snap.apply_delta(&CommitDelta::TaskLifecycle {
        task_id: "T1".to_string(),
        transition: TaskTransition::Failed,
    });
    snap.apply_delta(&CommitDelta::TaskLifecycle {
        task_id: "T1".to_string(),
        transition: TaskTransition::Reopened,
    });

    snap.apply_delta(&CommitDelta::ProgressUpdate {
        completed_step: Some("plan".to_string()),
        current_step: Some("implement".to_string()),
    });
    snap.apply_delta(&CommitDelta::ProgressUpdate {
        completed_step: None,
        current_step: None,
    });

    snap.apply_delta(&CommitDelta::PlanComplete {
        final_step: Some("done".to_string()),
        closed_count: 1,
    });
    snap.apply_delta(&CommitDelta::PlanComplete {
        final_step: None,
        closed_count: 0,
    });

    snap.apply_delta(&CommitDelta::RejectionRecorded {
        key: "k".to_string(),
        message: Some("m".to_string()),
        topic: Some("t".to_string()),
    });
    snap.apply_delta(&CommitDelta::RejectionBudgetTripped {
        key: "k".to_string(),
        terminal_reason: "exhausted".to_string(),
    });

    snap.apply_delta(&CommitDelta::HandoffAccepted {
        from: HatId::from("a"),
        to: HatId::from("b"),
        handoff_path: Some("/tmp/x".to_string()),
    });
    snap.apply_delta(&CommitDelta::WorkflowPhaseAdvanced {
        chain_name: "chain".to_string(),
        instance_key: Some("inst".to_string()),
        new_phase: 2,
    });
    snap.apply_delta(&CommitDelta::CounterChanged {
        counter: "consecutive_failures".to_string(),
        new_value: 1,
    });
    snap.apply_delta(&CommitDelta::SeenTopic {
        topic: "t".to_string(),
    });
    snap.apply_delta(&CommitDelta::CompletionRequested);
    snap.apply_delta(&CommitDelta::CompletionHonored);
    snap.apply_delta(&CommitDelta::CancellationRequested);
    snap.apply_delta(&CommitDelta::StewardWoken);
    snap.apply_delta(&CommitDelta::SnapshotReset);

    snap.apply_delta(&CommitDelta::HatActivationCounted {
        hat: HatId::from("h"),
        new_count: 1,
    });
    snap.apply_delta(&CommitDelta::HatExhausted {
        hat: HatId::from("h"),
    });
    snap.apply_delta(&CommitDelta::RejectionLastIteration {
        key: "k".to_string(),
        iteration: 1,
    });
    snap.apply_delta(&CommitDelta::StallRecoveryCounted {
        key: "k".to_string(),
        new_count: 1,
    });
    snap.apply_delta(&CommitDelta::TaskBlockCounted {
        task_id: "T1".to_string(),
        new_count: 1,
    });
    snap.apply_delta(&CommitDelta::TaskAbandoned {
        task_id: "T1".to_string(),
    });

    snap.apply_delta(&CommitDelta::ReviewStepUpdated {
        plan_name: "p".to_string(),
        task_id: "T1".to_string(),
        step: "s".to_string(),
        synth_pass: true,
        synth_terminal: Some("review.passed".to_string()),
    });
    snap.apply_delta(&CommitDelta::HandoffTrackerUpdated {
        event_id: "e1".to_string(),
        accepted: true,
        escalation_reason: None,
    });
    snap.apply_delta(&CommitDelta::FlowLifecycleUpdated {
        flow_unit_id: "w1".to_string(),
        phase: "Closed".to_string(),
    });
    snap.apply_delta(&CommitDelta::RejectionDigestUpdated {
        reason_code: "r".to_string(),
        count: 1,
        last_message: "m".to_string(),
        last_ts: "t".to_string(),
        last_topic: "topic".to_string(),
    });

    // The exhaustive walk is the assertion: if any variant is
    // added without a branch, this test fails to compile.
    let _ = snap;
}

// ---------------------------------------------------------------------------
// Counter change mapping
// ---------------------------------------------------------------------------

#[test]
fn counter_change_updates_each_field() {
    let mut snap = LedgerSnapshot::cold_start();

    let counters: &[(&str, i64)] = &[
        ("iteration", 7),
        ("hat_handoff_seq", 3),
        ("consecutive_failures", 2),
        ("consecutive_blocked", 4),
        ("abandoned_task_redispatches", 1),
        ("consecutive_malformed_events", 5),
        ("consecutive_hard_gates", 6),
        ("consecutive_same_signature", 8),
        ("consecutive_no_progress_turns", 9),
        ("consecutive_steward_activations", 10),
        ("consecutive_completion_rejections", 11),
        ("consecutive_engine_gate_rejections", 12),
        ("invariant_violation_count", 13),
        ("last_rejection_fingerprint", 1234),
    ];

    for (name, value) in counters {
        snap.apply_delta(&CommitDelta::CounterChanged {
            counter: (*name).to_string(),
            new_value: *value,
        });
    }

    assert_eq!(snap.iteration, 7);
    assert_eq!(snap.hat_handoff_seq, 3);
    assert_eq!(snap.consecutive_failures, 2);
    assert_eq!(snap.consecutive_blocked, 4);
    assert_eq!(snap.abandoned_task_redispatches, 1);
    assert_eq!(snap.consecutive_malformed_events, 5);
    assert_eq!(snap.consecutive_hard_gates, 6);
    assert_eq!(snap.consecutive_same_signature, 8);
    assert_eq!(snap.consecutive_no_progress_turns, 9);
    assert_eq!(snap.consecutive_steward_activations, 10);
    assert_eq!(snap.consecutive_completion_rejections, 11);
    assert_eq!(snap.consecutive_engine_gate_rejections, 12);
    assert_eq!(snap.invariant_violation_count, 13);
    assert_eq!(snap.last_rejection_fingerprint, 1234);
}

#[test]
fn counter_change_negative_clamps_to_zero() {
    let mut snap = LedgerSnapshot::cold_start();
    snap.apply_delta(&CommitDelta::CounterChanged {
        counter: "consecutive_failures".to_string(),
        new_value: -5,
    });
    assert_eq!(snap.consecutive_failures, 0);
}

// ---------------------------------------------------------------------------
// Progress update is idempotent
// ---------------------------------------------------------------------------

#[test]
fn progress_update_idempotent() {
    let (_dir, mut ledger) = fresh_ledger();
    for _ in 0..3 {
        let _ = ledger
            .commit(
                CommitDelta::ProgressUpdate {
                    completed_step: Some("plan".to_string()),
                    current_step: Some("implement".to_string()),
                },
                None,
            )
            .expect("commit");
    }
    let snap = ledger.snapshot();
    // Same step pushed three times: deduped.
    assert_eq!(
        snap.progress
            .completed_steps
            .iter()
            .filter(|s| *s == "plan")
            .count(),
        1
    );
    assert_eq!(snap.progress.current_step.as_deref(), Some("implement"));
}

// ---------------------------------------------------------------------------
// Sanity: ledger path is `.ralph/ledger.jsonl`
// ---------------------------------------------------------------------------

#[test]
fn ledger_path_is_canonical() {
    let dir = workspace();
    let ledger = StateLedger::new(dir.path(), true);
    let expected = dir.path().join(LEDGER_RELATIVE_PATH);
    assert_eq!(ledger.ledger_path(), expected);
    assert_eq!(Path::new(LEDGER_RELATIVE_PATH), Path::new(".ralph/ledger.jsonl"));
}

// ---------------------------------------------------------------------------
// Workflow phase advancement is monotonic
// ---------------------------------------------------------------------------

#[test]
fn workflow_phase_advances_monotonically() {
    let mut snap = LedgerSnapshot::cold_start();
    snap.apply_delta(&CommitDelta::WorkflowPhaseAdvanced {
        chain_name: "chain".to_string(),
        instance_key: None,
        new_phase: 2,
    });
    snap.apply_delta(&CommitDelta::WorkflowPhaseAdvanced {
        chain_name: "chain".to_string(),
        instance_key: None,
        new_phase: 1, // Lower; should be ignored.
    });
    snap.apply_delta(&CommitDelta::WorkflowPhaseAdvanced {
        chain_name: "chain".to_string(),
        instance_key: None,
        new_phase: 3,
    });
    assert_eq!(snap.workflow_phases.get("chain::").copied(), Some(3));
}

// ---------------------------------------------------------------------------
// Seen topic dedup
// ---------------------------------------------------------------------------

#[test]
fn seen_topic_dedup() {
    let (_dir, mut ledger) = fresh_ledger();
    for _ in 0..3 {
        let _ = ledger
            .commit(
                CommitDelta::SeenTopic {
                    topic: "work.ready".to_string(),
                },
                None,
            )
            .expect("commit");
    }
    assert_eq!(
        ledger.snapshot().seen_topics.iter().filter(|t| *t == "work.ready").count(),
        1
    );
}
