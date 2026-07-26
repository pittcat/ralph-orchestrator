#![allow(unused_imports)]
// Wave execution: dispatcher drives per-iteration fan-out, io handles worker event
// merging into the main events file, worker spawns isolated wave worker backends,
// supervisor_bridge routes supervisor-backed waves when enabled.

mod dispatcher;
mod io;
mod supervisor_bridge;
pub(crate) mod task_projection;
mod worker;

pub use dispatcher::{
    HandleWaveOutcome, WaveDispatchLimits, WaveDispatchOutcome, WaveOutputs, execute_wave,
    handle_wave_events,
};
// 2026-07-23-001 plan U3: `WaveWorkerExecutor` +
// `execute_wave_via_supervisor_with_executor` + `WorkerRequest`
// are needed by tests that inject a counting executor into the
// supervisor path without spawning real workers. The surface
// stays `pub(crate)` so no new public API escapes the crate.
pub(crate) use dispatcher::{
    SupervisorFanInOutcome, WaveWorkerExecutor, WorkerRequest,
    execute_wave_via_supervisor_with_executor, run_supervisor_fan_in,
};
// 2026-07-26-002 plan U8 (R10): expose the shared
// worker-timeout prefix constant so the wave_supervisor test can
// assert the worker literal and the dispatcher classifier stay
// compile-linked.
pub(crate) use dispatcher::WORKER_TIMEOUT_ERR_PREFIX;
pub use io::{
    extract_readable_delta, merge_wave_results_to_events_file, push_to_tui_iteration,
    push_to_wave_worker_buffer, read_worker_events, read_worker_events_with_retry,
    truncate_wave_worker_preview,
};
// Re-export the supervisor bridge so the runtime can spawn it when `supervisor.enabled: true`.
pub use supervisor_bridge::{
    BridgeDispatchOutcome, BridgeError, CoordinatorSupervisorBridge, MockSupervisorBridge,
    ProductionBridgeContext, SlotBinding, SupervisorBridge, fail_closed_on_bind_error,
    is_supervisor_path_enabled,
};
pub use worker::{
    WaveWorkerExecutionMode, WaveWorkerOutcome, run_wave_worker, run_wave_worker_pty,
    wave_worker_execution_mode,
};
// 2026-07-25-003 plan U5: re-export the legacy `record_outcome`
// helper so the wave_supervisor test surface can pin the
// empty-success classification against the supervisor
// classifier.
pub(crate) use dispatcher::build_wave_failed_payload;
pub(crate) use dispatcher::record_outcome;
