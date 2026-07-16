#![allow(unused_imports)]
// Wave execution: dispatcher drives per-iteration fan-out, io handles worker event
// merging into the main events file, worker spawns isolated wave worker backends,
// supervisor_bridge routes supervisor-backed waves when enabled.

mod dispatcher;
mod io;
mod supervisor_bridge;
mod worker;

pub use dispatcher::{HandleWaveOutcome, WaveOutputs, execute_wave, handle_wave_events};
pub use io::{
    extract_readable_delta, merge_wave_results_to_events_file, push_to_tui_iteration,
    push_to_wave_worker_buffer, read_worker_events, read_worker_events_with_retry,
    truncate_wave_worker_preview,
};
// Re-export the supervisor bridge so the runtime can spawn it when `supervisor.enabled: true`.
pub use supervisor_bridge::{
    BridgeDispatchOutcome, BridgeError, CoordinatorSupervisorBridge, MockSupervisorBridge,
    SlotBinding, SupervisorBridge, is_supervisor_path_enabled,
};
pub use worker::{
    WaveWorkerExecutionMode, WaveWorkerOutcome, WaveWorkerStreamHandler, run_wave_worker,
    run_wave_worker_pty, wave_worker_execution_mode,
};
