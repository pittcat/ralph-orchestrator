//! 2026-09-03-0959 plan U6 — DAG scheduler module root.
//!
//! This module is the **integration layer** that wires the
//! generic job kernel (`runtime_job`) into the per-Unit
//! pipeline. Files:
//!   - `jobs` — `JobPipeline::advance` drives one Unit through
//!     `Execute → Review → Verify` with token CAS, pool caps,
//!     and a typed three-fix-attempt budget.
//!   - `driver` — `DagSchedulerDriver::observe_accepted` is the
//!     hook the `EventLoop` calls when an accepted result lands;
//!     it routes the event into the right pipeline slot.
//!   - `shadow` — `ShadowSinkReader::read` is the read-only
//!     projection used by the inspect command (mirrors U5's
//!     `dag_shadow::ShadowSink`).
//!   - `worktree` (U7) — `UnitWorktree::acquire` binds each
//!     Unit's trusted worktree to a verified base commit.
//!     Reuses the existing worktree if its branch tip matches
//!     the verified base; rejects on host-dirty / host-untracked
//!     or base-mismatch.
//!   - `integration` (U7) — `IntegrationOrchestrator::integrate`
//!     runs the per-target CAS fast-forward pipeline: second
//!     changed-path check, lane lease acquire, targeted gate,
//!     CAS FF, idempotent integration record.
//!
//! U6 intentionally does NOT own the integration-half
//! authorisation gate (the changed-path check that runs again
//! before integrator's FF pass). That gate is U7's concern;
//! U6 computes the changed-path *result* and stores it on the
//! descriptor so U7 can authorise against the same value.

pub mod driver;
pub mod integration;
pub mod jobs;
pub mod recovery;
pub mod shadow;
pub mod worktree;

// Step 1+2(2026-09-03-0959 DAG 接线):driver/jobs 与 runtime_job
// 全量 promote 为生产可见;EventLoop 接线前尚无 bin 侧生产调用方,
// 各 item 以最小粒度 `#[allow(dead_code)]` 标注并指向
// `presets/en/parallel-forge-preset-author-notes.md`「promote 前置
// 义务清单」。本模块仍不做 `dag_scheduler::*` 平铺 re-export——
// 测试经 `super::*` / 完整模块路径访问,接线 Step 引入生产调用方时
// 再按需 re-export。
