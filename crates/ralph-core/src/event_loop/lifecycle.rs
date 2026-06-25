// U5a: EventLoop 生命周期相关 free function SSOT。
//
// 从 event_loop/mod.rs 迁出当前仍存在的 lifecycle helper。
// impl EventLoop 方法留到 U5b-U5e 阶段处理。
//
// R-Refactor-2 / KTD5:helper 方法字节级未变(grep diff 验证)。

use super::*;

/// U2 (plan 2026-06-21-002): construct a `StateLedger` rooted at
/// `workspace`. The ledger is always enabled; the legacy in-memory
/// projection caches have been removed.
pub fn build_state_ledger_from_env(workspace: &std::path::Path) -> crate::state::StateLedger {
    debug!(
        workspace = %workspace.display(),
        "wiring fresh StateLedger into LoopState"
    );
    crate::state::StateLedger::new(workspace, true)
}
