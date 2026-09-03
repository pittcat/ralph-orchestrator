//! 2026-09-03-0959 plan U1: tri-state `scheduler_mode` gate that
//! isolates the legacy `Wave` authority from the new
//! runtime-owned DAG scheduler authority.
//!
//! The canonical types and the validation helper live in
//! [`crate::config::scheduler_mode`] so both
//! [`crate::config::EventLoopConfig`] (which carries the
//! `supervisor.scheduler_mode` field) and the supervisor
//! runtime can depend on them without creating a
//! `config -> supervisor -> config` import cycle.
//!
//! This module is the thin supervisor-side facade: it re-exports
//! the public surface so future Units (U2 artifact / U3 DAG
//! persistence) can write
//! `crate::supervisor::dag_mode::validate_scheduler_mode(...)`
//! without reaching into `crate::config::scheduler_mode::*`
//! directly. Future Units that add DB-backed DAG tables also
//! belong here — not in `crate::config`.

pub use crate::config::scheduler_mode::{
    SchedulerMode, SchedulerModeError, validate_scheduler_mode,
};
