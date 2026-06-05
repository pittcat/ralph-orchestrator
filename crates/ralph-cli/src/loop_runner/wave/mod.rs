#![allow(unused_imports)]

mod acp_mock;
mod dispatcher;
mod io;
mod worker;

pub use acp_mock::AcpWaveExecutionResult;
#[cfg(test)]
pub use acp_mock::{MOCK_ACP_EXECUTION_SERIAL, MOCK_ACP_EXECUTIONS, MockAcpExecution};
pub use dispatcher::{WaveOutputs, execute_wave, handle_wave_events};
pub use io::{
    extract_readable_delta, merge_wave_results_to_events_file, push_to_tui_iteration,
    push_to_wave_worker_buffer, read_worker_events, read_worker_events_with_retry,
    truncate_wave_worker_preview,
};
pub use worker::{
    WaveWorkerExecutionMode, WaveWorkerOutcome, WaveWorkerStreamHandler,
    execute_wave_worker_acp_prompt, run_wave_worker, run_wave_worker_acp, run_wave_worker_pty,
    wave_worker_execution_mode,
};
