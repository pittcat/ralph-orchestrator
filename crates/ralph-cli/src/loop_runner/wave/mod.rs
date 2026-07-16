// 2026-07-16 (plan 2026-07-16-005, Unit 5 path B): `acp_mock` 模块
// (AcpWaveExecutionResult / MockAcpExecution / MOCK_ACP_* statics)
// 已确认为死代码并删除。证据见 U1 笔记
// `.ralph/review/2026-07-16-005-refactor-ralph-cli-parallel-tests-plan/scratch/u1-parallel-failure-characterization.md` §5.3。
// git history 保留;若未来需要恢复 mock ACP,`git log -- crates/ralph-cli/src/loop_runner/wave/acp_mock.rs` 可找回。

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
// 2026-07-03-001 plan U12: export the supervisor bridge so the
// runtime can spawn it when `supervisor.enabled: true`.
pub use supervisor_bridge::{
    BridgeDispatchOutcome, BridgeError, CoordinatorSupervisorBridge, MockSupervisorBridge,
    SlotBinding, SupervisorBridge, is_supervisor_path_enabled,
};
pub use worker::{
    WaveWorkerExecutionMode, WaveWorkerOutcome, WaveWorkerStreamHandler, run_wave_worker,
    run_wave_worker_pty, wave_worker_execution_mode,
};
