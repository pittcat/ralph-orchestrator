// U2a: 真正跨子文件共享的 helper(KTD4 严格原则)。
//
// 从原 `loop_runner/tests.rs:770-874` 行迁出。后续 U2b-U2h 主题子文件
// 通过 `use super::common::*;` 引用本模块的 helper。
//
// 本模块所有 `pub(super)` 函数在 `loop_runner::tests::*` 命名空间下可见,
// 即 sibling 子文件(`fake_path` / 后续 `wave` / `hooks` 等)与原 `tests.rs` 内
// `#[test]` 函数都能直接调用。

use super::*;
use ralph_core::diagnostics::HookRunTelemetryEntry;

// ──────────────────────────────────────────────────────────────────────
// dispatch_test_event_loop* (3 个 + 1 个 yaml 变种)
// ──────────────────────────────────────────────────────────────────────

#[cfg(unix)]
pub(super) fn dispatch_test_event_loop(workspace_root: &Path) -> EventLoop {
    let mut config = RalphConfig::default();
    config.core.workspace_root = workspace_root.to_path_buf();
    EventLoop::new(config)
}

#[cfg(unix)]
pub(super) fn dispatch_test_event_loop_with_context(
    workspace_root: &Path,
) -> (EventLoop, LoopContext) {
    let mut config = RalphConfig::default();
    config.core.workspace_root = workspace_root.to_path_buf();
    let context = LoopContext::primary(workspace_root.to_path_buf());
    let event_loop = EventLoop::with_context(config, context.clone());
    (event_loop, context)
}

pub(super) fn dispatch_test_event_loop_from_yaml_with_context(
    workspace_root: &Path,
    yaml: &str,
) -> (EventLoop, LoopContext) {
    let mut config: RalphConfig = serde_yaml::from_str(yaml).expect("parse config");
    config.core.workspace_root = workspace_root.to_path_buf();
    let context = LoopContext::primary(workspace_root.to_path_buf());
    let event_loop = EventLoop::with_context(config, context.clone());
    (event_loop, context)
}

#[cfg(unix)]
pub(super) fn dispatch_test_event_loop_with_diagnostics(workspace_root: &Path) -> EventLoop {
    let mut config = RalphConfig::default();
    config.core.workspace_root = workspace_root.to_path_buf();
    let diagnostics =
        ralph_core::diagnostics::DiagnosticsCollector::with_enabled(workspace_root, true)
            .expect("create diagnostics collector");
    EventLoop::with_diagnostics(config, diagnostics)
}

// ──────────────────────────────────────────────────────────────────────
// read_hook_*  (后续 U2c 拆 hooks.rs 时迁走)
// ──────────────────────────────────────────────────────────────────────

#[cfg(unix)]
pub(super) fn read_hook_run_telemetry_entries(workspace_root: &Path) -> Vec<HookRunTelemetryEntry> {
    let diagnostics_root = workspace_root.join(".ralph").join("diagnostics");
    let mut session_dirs: Vec<_> = std::fs::read_dir(&diagnostics_root)
        .expect("read diagnostics root")
        .filter_map(Result::ok)
        .collect();
    session_dirs.sort_by_key(|entry| entry.path());

    let latest_session = session_dirs
        .last()
        .expect("at least one diagnostics session should exist");
    let hook_runs_path = latest_session.path().join("hook-runs.jsonl");
    let content = std::fs::read_to_string(&hook_runs_path).expect("read hook-runs.jsonl");

    content
        .lines()
        .map(|line| serde_json::from_str(line).expect("parse hook run telemetry entry"))
        .collect()
}

#[cfg(unix)]
pub(super) fn read_hook_log(log_path: &Path) -> Vec<String> {
    std::fs::read_to_string(log_path)
        .expect("read hook log")
        .lines()
        .map(str::to_string)
        .collect()
}

#[cfg(unix)]
pub(super) fn read_hook_payload_log(log_path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(log_path)
        .expect("read hook payload log")
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse hook payload JSON"))
        .collect()
}

// ──────────────────────────────────────────────────────────────────────
// suspend_outcome / block_on_test_future / empty_hook_metadata
// ──────────────────────────────────────────────────────────────────────

pub(super) fn suspend_outcome_with_mode(
    phase_event: HookPhaseEvent,
    hook_name: &str,
    suspend_mode: HookSuspendMode,
) -> HookDispatchOutcome {
    HookDispatchOutcome {
        phase_event,
        hook_name: hook_name.to_string(),
        disposition: HookDisposition::Suspend,
        suspend_mode,
        failure: Some(HookDispatchFailure::HookRunFailed {
            exit_code: Some(41),
            timed_out: false,
        }),

        mutation_parse_outcome: HookMutationParseOutcome::Disabled,
    }
}

pub(super) fn suspend_outcome(phase_event: HookPhaseEvent, hook_name: &str) -> HookDispatchOutcome {
    suspend_outcome_with_mode(phase_event, hook_name, HookSuspendMode::WaitForResume)
}

pub(super) fn block_on_test_future<F>(future: F) -> F::Output
where
    F: std::future::Future,
{
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("build tokio runtime")
        .block_on(future)
}

pub(super) fn empty_hook_metadata() -> serde_json::Map<String, serde_json::Value> {
    serde_json::Map::new()
}

// ──────────────────────────────────────────────────────────────────────
// build_*_payload_input (4 个)
// ──────────────────────────────────────────────────────────────────────

// DEC-001:本模块 4 个 `build_*_payload_input` 与 2 个 `dispatch_*_loop_termination_hooks`
// 包装 fn 名字与 `loop_runner::payload_inputs` 同名 fn 冲突(后者通过
// `loop_runner/mod.rs:pub use payload_inputs::*;` 暴露)。`#[test]` 函数在
// `loop_runner::tests::legacy` 命名空间下用 `use super::super::*;` 引入
// `loop_runner::*` 时会同时引入 `build_*_payload_input` 与本模块的同名包装 fn,
// 产生歧义 (E0659)。
//
// 解决:见 `loop_runner/mod.rs` 的修改 — `pub use payload_inputs::*;` 改为精确
// `pub use payload_inputs::build_*_payload_input as _; ...` 形式,
// 这样 `loop_runner::*` 不再 glob 暴露 `build_*_payload_input`,legacy.rs 用
// `use super::super::*;` 不会引入,`#[test]` 内的 `build_*_payload_input(...)`
// 调用解析为本模块(同 `loop_runner::tests::legacy` 命名空间下)的包装 fn。
// 本模块 4 个 fn 名字保持 `build_*_payload_input`(原 tests.rs 字面相同)。
pub(super) fn build_loop_start_payload_input(
    loop_id: &str,
    ctx: &LoopContext,
    max_iterations: u32,
    iteration_current: u32,
    active_hat: Option<String>,
) -> HookPayloadBuilderInput {
    super::super::payload_inputs::build_loop_start_payload_input(
        loop_id,
        ctx,
        max_iterations,
        iteration_current,
        active_hat,
        &empty_hook_metadata(),
    )
}

pub(super) fn build_iteration_start_payload_input(
    loop_id: &str,
    ctx: &LoopContext,
    max_iterations: u32,
    iteration_current: u32,
    active_hat: Option<String>,
    selected_hat: Option<String>,
    selected_task: Option<String>,
) -> HookPayloadBuilderInput {
    super::super::payload_inputs::build_iteration_start_payload_input(
        loop_id,
        ctx,
        max_iterations,
        iteration_current,
        active_hat,
        selected_hat,
        selected_task,
        &empty_hook_metadata(),
    )
}

pub(super) fn build_plan_created_payload_input(
    loop_id: &str,
    ctx: &LoopContext,
    max_iterations: u32,
    iteration_current: u32,
    active_hat: Option<String>,
    selected_hat: Option<String>,
    selected_task: Option<String>,
) -> HookPayloadBuilderInput {
    super::super::payload_inputs::build_plan_created_payload_input(
        loop_id,
        ctx,
        max_iterations,
        iteration_current,
        active_hat,
        selected_hat,
        selected_task,
        &empty_hook_metadata(),
    )
}

pub(super) fn build_loop_termination_payload_input(
    loop_id: &str,
    ctx: &LoopContext,
    max_iterations: u32,
    iteration_current: u32,
    active_hat: Option<String>,
    selected_hat: Option<String>,
    selected_task: Option<String>,
    termination_reason: &TerminationReason,
) -> HookPayloadBuilderInput {
    super::super::payload_inputs::build_loop_termination_payload_input(
        loop_id,
        ctx,
        max_iterations,
        iteration_current,
        active_hat,
        selected_hat,
        selected_task,
        termination_reason,
        &empty_hook_metadata(),
    )
}

// ──────────────────────────────────────────────────────────────────────
// dispatch_pre/post_loop_termination_hooks (2 个)
// ──────────────────────────────────────────────────────────────────────

// DEC-001:同 `build_*_payload_input` 注释。本模块 2 个 dispatch_*_loop_termination_hooks
// 包装 fn 名字保持与原 tests.rs 字面相同(`dispatch_pre/post_loop_termination_hooks`),
// 依赖 `loop_runner/mod.rs` 的 `pub use payload_inputs::*;` 改为精确
// `pub use ... as _;` 形式以避免重名歧义。
pub(super) async fn dispatch_pre_loop_termination_hooks(
    event_loop: &EventLoop,
    hooks_dispatch_enabled: bool,
    loop_id: &str,
    hook_engine: &HookEngine,
    hook_executor: &HookExecutor,
    suspend_state_store: &SuspendStateStore,
    ctx: &LoopContext,
    max_iterations: u32,
    reason: TerminationReason,
) -> Result<TerminationReason> {
    let mut accumulated_hook_metadata = serde_json::Map::new();
    super::super::hooks::termination::dispatch_pre_loop_termination_hooks(
        event_loop,
        hooks_dispatch_enabled,
        loop_id,
        hook_engine,
        hook_executor,
        suspend_state_store,
        ctx,
        max_iterations,
        &mut accumulated_hook_metadata,
        reason,
    )
    .await
}

pub(super) async fn dispatch_post_loop_termination_hooks(
    event_loop: &EventLoop,
    hooks_dispatch_enabled: bool,
    loop_id: &str,
    hook_engine: &HookEngine,
    hook_executor: &HookExecutor,
    suspend_state_store: &SuspendStateStore,
    ctx: &LoopContext,
    max_iterations: u32,
    reason: TerminationReason,
) -> Result<TerminationReason> {
    let mut accumulated_hook_metadata = serde_json::Map::new();
    super::super::hooks::termination::dispatch_post_loop_termination_hooks(
        event_loop,
        hooks_dispatch_enabled,
        loop_id,
        hook_engine,
        hook_executor,
        suspend_state_store,
        ctx,
        max_iterations,
        &mut accumulated_hook_metadata,
        reason,
    )
    .await
}

// ──────────────────────────────────────────────────────────────────────
// MockAcpExecution and install_mock_acp_executions were removed as dead
// code: production wave paths never pop the queue and the helper had no
// remaining call sites after the wave test split.
// ──────────────────────────────────────────────────────────────────────
