//! U9 / fix-plan U9: `tests/wave_supervisor.rs` — pin the
//! supervisor bridge hot-path contract at the loop_runner
//! test integration level.
//!
//! Why this file exists (fix-plan F-009 / U12 delivery side):
//! the previous supervisor plan wired the bridge types but
//! never connected them to the wave dispatcher. This file
//! locks in three named invariants so a future regression
//! (e.g. accidental reversion of the dispatcher branch or
//! dropping the bridge trait object on the floor) is caught
//! by nextest.
//!
//! As of plan 2026-08-07-006, this entry only declares the
//! behavior-family modules and re-exports the shared fixtures.
//! Each test family lives in `wave_supervisor/*.rs`.
//!
//! Test contract reference (plan §2.2):
//!   - `enabled_false_uses_wave_tracker`
//!   - `enabled_true_calls_bridge_bind_slot`
//!   - `bridge_off_no_feature_returns_error_path`

// The original flat `wave_supervisor.rs` had mid-file `use` statements
// (e.g. for `InMemorySupervisorStore`, `SupervisorStore`,
// `SupervisorFanInOutcome`, `run_supervisor_fan_in`) that subsequent
// tests relied on. After splitting into family modules, those names
// must remain reachable through `super::super::*`; declaring them
// here keeps the byte-equality contract for the existing test
// function bodies (which still reference these names directly).

mod coordination;
mod dispatch;
mod fixtures;
mod misc;
mod redrive_payload;
mod salvage_merge;
mod slot_binding;
mod supervisor;
mod timeouts;

pub(super) use crate::loop_runner::tests::fake_path;
