pub mod dispatch;
pub mod format;
pub mod mutation;
pub mod retry;
pub mod termination;

pub use dispatch::*;
pub use format::*;
pub use mutation::*;
pub use retry::*;
// 保留 `pub use termination::*;` glob:`tests/legacy.rs:2870, 2877` 用短名
// `loop_termination_phase_events(&TerminationReason::...)` 调用,通过
// `use super::super::*;` → `pub use hooks::*;` → `pub use termination::*;`
// 解析到 `loop_runner::hooks::termination::loop_termination_phase_events`。
// 删除此行会触发 E0425("cannot find function in this scope")。
pub use termination::*;
