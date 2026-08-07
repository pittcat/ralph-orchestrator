//! Wave dispatcher façade — re-exports the production symbols from
//! `dispatcher/{dispatch,worker_lifecycle,fan_in,coordination,salvage,
//!  deadlines,outcomes}.rs`. Behaviour is unchanged from the pre-split
//! single-file layout (plan `2026-08-07-008`).
//!
//! `wave/mod.rs` continues to `pub use dispatcher::*;` for the
//! same surface; the only thing that changed is the physical layout
//! of the production code.

pub(crate) mod coordination;
pub(crate) mod deadlines;
pub(crate) mod dispatch;
pub(crate) mod fan_in;
pub(crate) mod outcomes;
pub(crate) mod salvage;
pub(crate) mod worker_lifecycle;

// Re-export public surface — must stay byte-equivalent to what
// `wave/mod.rs` previously imported from the single-file dispatcher.
pub(crate) use dispatch::WORKER_TIMEOUT_ERR_PREFIX;
pub use dispatch::{
    HandleWaveOutcome, WaveDispatchLimits, WaveDispatchOutcome, WaveOutputs, execute_wave,
    handle_wave_events,
};
pub(crate) use dispatch::{
    ProductionExecutor, WaveWorkerExecutor, WorkerRequest,
    boot_dispatch_pending_redrive_if_resuming, dispatch_pending_redrive_waves,
    dispatch_redrive_child_wave, dispatch_wave_inner, execute_wave_via_supervisor_with_executor,
    handle_wave_rejection,
};

pub(crate) use fan_in::{SupervisorFanInOutcome, TerminalFanInContext, run_supervisor_fan_in};

pub(crate) use coordination::{
    CoordCommitOutcome, ReviewDoneHints, append_supervisor_coord_event, build_review_done_hints,
    build_review_reconciliation, build_wave_complete_payload, build_wave_failed_payload,
    collect_review_dimensions, commit_complete_coord_event, commit_failed_coord_event,
    compute_review_missing_dimensions, payload_object, unix_now_secs,
};
pub(crate) use outcomes::{
    ClassifiedReason, ClassifiedSlot, classify_slot_attempt, classify_slot_result,
    compute_slot_batch_fingerprint, finalize_global_exceeded, finalize_timeout,
    inject_synthetic_failures, merge_round_into, outcome_for_completion,
    record_loop_max_runtime_envelope, record_outcome, record_wave_spawn_failed_envelope,
    record_wave_timeout_envelope, reported_failure_detail, take_results,
    wait_for_progress_reporter,
};

pub(crate) use salvage::{
    append_wave_channel_to_marker, build_empty_projection_receipt, build_wave_failed_slots_json,
    commit_salvage_batch, fingerprint_lines, merge_completed_exec_fix_slots_to_main,
    merge_completed_review_slots_to_main, project_empty_salvage, status_to_str,
    workspace_root_from_events, write_wave_diagnostics_json,
};

pub(crate) use deadlines::{
    PARTIAL_THRESHOLD_DEN, PARTIAL_THRESHOLD_NUM, aggregate_floor_for_attempts,
    aggregate_timeout_for, attempt_aware_aggregate_timeout,
    effective_detected_aggregate_deadline_secs, open_default_supervisor_store,
    parse_assigned_dimension, wave_work_budget,
};

pub(crate) use dispatch::DispatchContext;
pub(crate) use dispatch::ProgressChannels;
pub(crate) use worker_lifecycle::SupervisorSlotRelease;
