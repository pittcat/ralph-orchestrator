use super::*;
use crate::test_support::CwdGuard;
use ralph_core::HatRegistry;
use ralph_core::planning_session::{ConversationEntry, ConversationType};
use ralph_proto::{Hat, Topic};
use std::collections::HashSet;
use std::ffi::OsStr;
use std::sync::Arc;
use std::sync::Mutex;

// ──────────────────────────────────────────────────────────────────────
// Test execution requirements (Unit 4 of plan 2026-06-06-001, follow-up
// to Unit 3's "5 pre-existing test failures" note):
//
// These tests touch two **process-global** `Mutex` / `LazyLock` singletons
// declared in `loop_runner/tests/fake_path.rs`:
//
//   - FAKE_PATH_BACKEND_SERIAL      (fake-PATH backend installation guard)
//   - FAKE_PATH_BACKEND_BIN         (fake-PATH backend bin dir)
//
// (Historical note: there used to be two more — `MOCK_ACP_EXECUTIONS` and
// `MOCK_ACP_EXECUTION_SERIAL`. On 2026-07-16 they were confirmed dead code
// and removed as part of plan `2026-07-16-005-refactor-ralph-cli-parallel-tests-plan`,
// Unit 5 path B. See
// `.ralph/review/2026-07-16-005-refactor-ralph-cli-parallel-tests-plan/scratch/u1-parallel-failure-characterization.md`
// §5.3 for the grep proof.)
//
// The two remaining `FAKE_PATH_BACKEND_*` locks live inside the same
// binary and historically led to a `cli-serial` override in
// `.config/nextest.toml` (`max-threads = 1`). The 2026-07-16 plan
// removes that override after empirical proof (Unit 1 + Unit 6 of the
// plan) that nextest's process-per-test isolation makes the
// shared-process Mutex semantics moot — every test runs in its own
// process, so the locks are not actually shared across tests.
//
// **Run via `./scripts/run-tests.sh` or `cargo nextest run -p ralph-cli --bin ralph`.**
// `cargo nextest run` is the only supported default entry point; the
// `cargo test -p ralph-cli` fallback is documented in
// `docs/solutions/developer-experience/ralph-cli-loop-runner-tests-must-run-serial.md`
// (kept for historical context, not the recommended path).
//
// Do NOT add `#[ignore]` to the wave / FAKE_PATH tests as a "fix" for
// parallel-load failures: they are real tests of the production
// runner code path, and skipping them defeats the regression guard.
// ──────────────────────────────────────────────────────────────────────────────────────────────────────

// U2a: tests 目录骨架。
//
// 本文件是 `loop_runner/tests` 目录的入口(原 `loop_runner/tests.rs` 的目录版)。
// 顶部 1-50 行与原 `tests.rs` 头部 1-50 行**逐字节一致**(preflight 验证点)。
// 本文件自身不持有任何 `static` Mutex:
// - 2 个 `FAKE_PATH_BACKEND_*` private `static` Mutex 在 `fake_path.rs`(R5)
// - 2 个 `MOCK_ACP_*` `pub static` Mutex 在 `loop_runner/wave/acp_mock.rs`(不在本 plan 范围)
//
// 子模块拆分拓扑(KTD3):
// - `common`: 真正跨子文件共享的 helper,`pub(super)` 暴露
// - `fake_path`: fake-PATH 后端安装 helper + 2 个 `FAKE_PATH_BACKEND_*` private `static` Mutex
// - `wave`(U2b): wave / acp / MockAcpExecution / forced_test_wave_pty_failure 测试 +
//   wave 特定 helper(`make_test_wave` / `make_worker_event` / `emit_wave_validation_marker` 等),
//   从原 `legacy.rs` 段 1(行 3713-6504)+ 段 2(行 8628-8958)迁出
// - `legacy`: U2b 后剩余非 wave 测试(hard_gate / suspend / merge_queue / late_events /
//   diagnostics / preset_lint / pipeline 等)
// - `hooks`(U2c): dispatch_phase_event_hooks / lifecycle_hooks / hook mutation
//   namespace / retry-backoff / wait-then-retry / blocking-outcomes 等测试族
// - 后续 U2d-U2h 按主题逐步拆出

mod common;
mod fake_path;
mod hard_gate;
mod hard_gate_payload_contract;
mod hooks;
mod legacy;
mod wave;
mod wave_supervisor;
