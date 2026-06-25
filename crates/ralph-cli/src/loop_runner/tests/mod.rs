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
// These tests touch four **process-global** `Mutex` / `LazyLock` singletons
// declared further down in this file:
//
//   - MOCK_ACP_EXECUTIONS           (mock ACP backend queue)
//   - MOCK_ACP_EXECUTION_SERIAL     (mock ACP execution guard)
//   - FAKE_PATH_BACKEND_SERIAL      (fake-PATH backend installation guard)
//   - FAKE_PATH_BACKEND_BIN         (fake-PATH backend bin dir)
//
// The locks are intentionally process-global because the wave / FAKE_PATH
// test scaffolding is shared across many test functions and serializing
// within the binary process keeps the wave fixtures consistent.
//
// Consequence: under **plain `cargo test` (default test-threads)**, the
// 5xx+ tests in this binary run in parallel inside a single OS process
// and **share the same Mutexes**. A panic in one test poisons the
// `FAKE_PATH_BACKEND_SERIAL` Mutex; every subsequent test that goes
// through `install_fake_path_backends(...)` then panics on
// `PoisonError { .. }`. Similarly, time-sensitive tests like
// `test_process_pending_merges_redirects_subprocess_output_to_log_file`
// use a 500ms sleep to wait for the sub-process to flush its log file;
// under parallel load the sub-process can take longer, producing
// spurious failures. None of these are real bugs.
//
// The project's `scripts/run-tests.sh` and the `nextest` profile
// (`.config/nextest.toml`) put this entire binary in the `cli-serial`
// test group with `max-threads = 1`, which sidesteps both problems.
//
// **Run via `./scripts/run-tests.sh` or `cargo nextest run -p ralph-cli --bin ralph`.**
// If you must run with raw `cargo test`, pass `--test-threads=1`:
//
//     cargo test -p ralph-cli --bin ralph -- --test-threads=1
//
// Do NOT add `#[ignore]` to the wave / FAKE_PATH tests as a "fix" for
// the parallel-load failures: they are real tests of the production
// runner code path, and skipping them defeats the regression guard.
// ──────────────────────────────────────────────────────────────────────

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
