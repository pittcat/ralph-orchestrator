//! Step Handoff mechanism — 阶段交接硬门
//!
//! Contains:
//! - [`progress_task_gate`]: pre-handoff hard gate that validates
//!   `progress.md` is consistent with `tasks.jsonl` before
//!   `queue.advance` / `plan.complete` can pass through the policy.
//!
//! Plan Unit: U4 of `2026-06-17-002-feat-ce-executor-step-handoff-plan.md`.
//! See [`crate::event_loop`] for the integration site.

pub mod progress_task_gate;