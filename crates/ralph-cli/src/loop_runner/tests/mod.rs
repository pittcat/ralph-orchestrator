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
// Test execution requirements:
//
// These tests touch two process-global fake-PATH fixture guards declared in
// `fake_path.rs`. Nextest's process-per-test isolation keeps those fixtures
// independent across tests. Run this suite with `cargo nextest run`; if the
// raw `cargo test` fallback is unavoidable, pass `-- --test-threads=1`.
// Do not add `#[ignore]` to wave or fake-PATH tests: they exercise the real
// runner path and remain part of the regression guard.
// ──────────────────────────────────────────────────────────────────────

// This module is the directory-based entry point for the loop-runner tests.
// Shared helpers live in `common`; fixture-specific helpers live in
// `fake_path`; topic-focused tests are grouped in the sibling modules below.

mod common;
mod fake_path;
mod hard_gate;
mod hard_gate_payload_contract;
mod hooks;
mod legacy;
mod wave;
mod wave_supervisor;
