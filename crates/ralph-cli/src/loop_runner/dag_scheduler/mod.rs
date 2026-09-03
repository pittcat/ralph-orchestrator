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
//!
//! U6 intentionally does NOT own the integration-half
//! authorisation gate (the changed-path check that runs again
//! before integrator's FF pass). That gate is U7's concern;
//! U6 computes the changed-path *result* and stores it on the
//! descriptor so U7 can authorise against the same value.

pub mod driver;
pub mod jobs;
pub mod shadow;

// U6 ships these modules but does NOT re-export their public types
// at the `dag_scheduler::*` path: every type's bin-side consumer is
// U7's integration concern. Keeping them un-exported here means the
// bin "ralph" target sees zero dead-code for the U6 surface. Tests
// reach them via `super::*` and `crate::loop_runner::dag_scheduler::*`
// (full module path), so this is consistent with the test mod's
// `use super::*;` convention.
