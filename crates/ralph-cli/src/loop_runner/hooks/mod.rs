pub mod dispatch;
pub mod format;
pub mod mutation;
pub mod retry;
pub mod termination;

pub use dispatch::*;
pub use format::*;
pub use mutation::*;
pub use retry::*;
// 2026-07-16 cleanup U3: `tests/hooks.rs:2299, 2306` use
// `loop_termination_phase_events` via the `use super::super::*;` glob in
// `mod tests`, which transitively resolves through
// `loop_runner::*` → `pub use hooks::*;` → `pub use termination::*;`.
// Removing this glob breaks the existing test imports even though
// `tests/legacy.rs` no longer uses the short name. Per KTD-3
// (公共契约 / test-fixture 守门) we keep the glob but suppress the
// "unused" warning with the `_` rename trick (also covered by
// `#[allow(unused_imports)]` since the visibility is needed for the
// module-level `pub use`).
#[allow(unused_imports)]
pub use termination::*;
