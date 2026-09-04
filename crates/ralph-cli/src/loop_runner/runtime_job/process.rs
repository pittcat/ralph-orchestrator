//! 2026-09-03-0959 plan U6 (D7 / D8 / D11 / E10-E12): subprocess
//! abstraction for the runtime job kernel.
//!
//! The kernel is intentionally **process-port-shaped** rather
//! than coupled to `std::process::Command` or `tokio::process`.
//! Tests use in-memory fakes (`FakeJobProcessPort`,
//! `RecordingJobProcessPort`) to drive the pipeline deterministically
//! without spawning real subprocesses.
//!
//! The trait surface matches the plan §Unit 6 §17 spec verbatim:
//!   - `pre_fence` — called BEFORE any side effect; lets the
//!     port refuse to launch when the descriptor's
//!     `forbidden_paths` are present or `allowed_paths` are
//!     missing. The kernel calls this even when the port is a
//!     no-op (real ports may use it to enforce the integration
//!     half's authorised-path set; U6 only requires the seam).
//!   - `launch` — spawn the child, return a handle the kernel
//!     can collect on.
//!   - `collect_with_deadline` — block (or yield) up to the
//!     supplied deadline; the test fakes use a 0-deadline
//!     "already done?" semantics so pipeline tests stay
//!     deterministic.
//!   - `cancel` — kill a running child by pid; the kernel uses
//!     this on `HeartbeatTimeout`.
//!
//! Production code outside tests uses the `JobProcessPort` trait
//! only. The concrete fakes (`FakeJobProcessPort`,
//! `PidHandle`, `RecordingJobProcessPort`) are `#[cfg(test)]` —
//! they exist to drive the pipeline deterministically without
//! spawning a real subprocess. U7 will introduce a real backend
//! port (PTY / tokio / etc.) that lives outside `#[cfg(test)]`.

#[cfg(test)]
use super::prompt::PromptContext;
#[cfg(test)]
use super::{JobDescriptor, ProcessResult, RuntimeJobError};

/// Opaque handle the kernel uses to collect a launched child.
/// Implementations MUST expose `pid` so the kernel can record it
/// for cancellation and so the per-attempt PID-tracking test can
/// assert.
#[cfg(test)]
pub trait JobProcessHandle: Send {
    fn pid(&self) -> i32;
}

/// Subprocess port the kernel drives. One impl per host backend
/// (PTY / pipe / tokio). The kernel is *only* coupled to this
/// trait.
#[cfg(test)]
pub trait JobProcessPort: Send {
    /// Pre-launch fence. Called BEFORE `launch`. The default
    /// `FakeJobProcessPort` impl treats every descriptor as
    /// passing; tests that exercise the failure path use the
    /// `FailingPreFence` fake.
    fn pre_fence(&self, _descriptor: &JobDescriptor) -> Result<(), RuntimeJobError> {
        Ok(())
    }

    fn launch(&self, prompt: &PromptContext) -> Result<Box<dyn JobProcessHandle>, RuntimeJobError>;

    fn collect_with_deadline(
        &self,
        handle: &dyn JobProcessHandle,
        deadline_ms: u64,
    ) -> Result<ProcessResult, RuntimeJobError>;

    fn cancel(&self, pid: i32) -> Result<(), RuntimeJobError>;
}

// ---------------------------------------------------------------------------
// Test-only concrete fakes. U7 introduces a real backend port
// that lives outside `#[cfg(test)]`.
// ---------------------------------------------------------------------------

/// Minimal concrete handle. Tests use this directly; real ports
/// return their own type behind `Box<dyn JobProcessHandle>`.
#[cfg(test)]
#[derive(Debug, Clone)]
pub struct PidHandle {
    pid: i32,
}

#[cfg(test)]
impl PidHandle {
    pub fn new(pid: i32) -> Self {
        Self { pid }
    }
}

#[cfg(test)]
impl JobProcessHandle for PidHandle {
    fn pid(&self) -> i32 {
        self.pid
    }
}

/// Recording port: captures every call so tests can assert what
/// the kernel did. `pending_results` lets the test decide when
/// each launched job "completes" — populating an entry before the
/// next `collect_with_deadline` makes the job appear done.
///
/// `pre_fence_fail_descriptor` lets a single test exercise the
/// pre-fence rejection path: set the unit_key that should trip
/// the fence and the next `pre_fence` call returns
/// `PreFenceFailed`.
#[cfg(test)]
#[derive(Debug)]
pub struct FakeJobProcessPort {
    next_pid: std::sync::Mutex<i32>,
    pub launches: std::sync::Mutex<Vec<PromptContext>>,
    pub pending_results: std::sync::Mutex<Vec<(i32, ProcessResult)>>,
    pub cancels: std::sync::Mutex<Vec<i32>>,
    pub pre_fence_calls: std::sync::Mutex<Vec<String>>,
    pub pre_fence_fail_descriptor: Option<String>,
    pub collect_calls: std::sync::Mutex<Vec<(i32, u64)>>,
}

#[cfg(test)]
impl FakeJobProcessPort {
    pub fn new(_name: impl Into<String>) -> Self {
        Self {
            next_pid: std::sync::Mutex::new(1000),
            launches: std::sync::Mutex::new(Vec::new()),
            pending_results: std::sync::Mutex::new(Vec::new()),
            cancels: std::sync::Mutex::new(Vec::new()),
            pre_fence_calls: std::sync::Mutex::new(Vec::new()),
            pre_fence_fail_descriptor: None,
            collect_calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Schedule a result the next `collect_with_deadline` call
    /// for the matching pid will return. The port pops the entry
    /// so subsequent calls return `CollectFailed("not ready")`.
    pub fn enqueue_result(&self, pid: i32, result: ProcessResult) {
        self.pending_results
            .lock()
            .expect("FakeJobProcessPort pending_results mutex poisoned")
            .push((pid, result));
    }

    pub fn set_pre_fence_fail(&mut self, descriptor_unit_key: impl Into<String>) {
        self.pre_fence_fail_descriptor = Some(descriptor_unit_key.into());
    }

    pub fn launch_count(&self) -> usize {
        self.launches
            .lock()
            .expect("FakeJobProcessPort launches mutex poisoned")
            .len()
    }
}

#[cfg(test)]
impl JobProcessPort for FakeJobProcessPort {
    fn pre_fence(&self, descriptor: &JobDescriptor) -> Result<(), RuntimeJobError> {
        self.pre_fence_calls
            .lock()
            .expect("FakeJobProcessPort pre_fence_calls mutex poisoned")
            .push(descriptor.unit_key.clone());
        if self.pre_fence_fail_descriptor.as_deref() == Some(descriptor.unit_key.as_str()) {
            return Err(RuntimeJobError::PreFenceFailed(
                "fake pre-fence rejection".to_string(),
            ));
        }
        Ok(())
    }

    fn launch(&self, prompt: &PromptContext) -> Result<Box<dyn JobProcessHandle>, RuntimeJobError> {
        let mut next = self
            .next_pid
            .lock()
            .expect("FakeJobProcessPort next_pid mutex poisoned");
        let pid = *next;
        *next = next.saturating_add(1);
        drop(next);
        self.launches
            .lock()
            .expect("FakeJobProcessPort launches mutex poisoned")
            .push(prompt.clone());
        Ok(Box::new(PidHandle::new(pid)))
    }

    fn collect_with_deadline(
        &self,
        handle: &dyn JobProcessHandle,
        deadline_ms: u64,
    ) -> Result<ProcessResult, RuntimeJobError> {
        self.collect_calls
            .lock()
            .expect("FakeJobProcessPort collect_calls mutex poisoned")
            .push((handle.pid(), deadline_ms));
        let mut queue = self
            .pending_results
            .lock()
            .expect("FakeJobProcessPort pending_results mutex poisoned");
        if let Some(pos) = queue.iter().position(|(pid, _)| *pid == handle.pid()) {
            let (_, result) = queue.remove(pos);
            return Ok(result);
        }
        Err(RuntimeJobError::CollectFailed(
            "fake port: no result ready".to_string(),
        ))
    }

    fn cancel(&self, pid: i32) -> Result<(), RuntimeJobError> {
        self.cancels
            .lock()
            .expect("FakeJobProcessPort cancels mutex poisoned")
            .push(pid);
        Ok(())
    }
}

/// Compile-time assertion that the `JobProcessHandle` trait
/// object can flow through `Box`. Keeps the public port
/// abstraction honest. Test-only — the bin target does not need
/// this guard.
#[cfg(test)]
fn _assert_handle_object_safe(_: Box<dyn JobProcessHandle>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_runner::runtime_job::Stage;
    use crate::loop_runner::runtime_job::prompt::PromptContext;
    use serde_json::json;

    fn fake_prompt(unit: &str) -> PromptContext {
        PromptContext {
            unit_key: unit.to_string(),
            job_id: "j-1".to_string(),
            hat: "executor".to_string(),
            stage: "execute".to_string(),
            allowed_paths: Vec::new(),
            forbidden_paths: Vec::new(),
            env_allowlist_keys: Vec::new(),
        }
    }

    /// Launch assigns a fresh pid each time and records the
    /// prompt verbatim.
    #[test]
    fn launch_assigns_monotonic_pids() {
        let port = FakeJobProcessPort::new("test");
        let h1 = port.launch(&fake_prompt("U1")).expect("launch 1");
        let h2 = port.launch(&fake_prompt("U2")).expect("launch 2");
        assert_eq!(h1.pid(), 1000);
        assert_eq!(h2.pid(), 1001);
        assert_eq!(port.launch_count(), 2);
    }

    /// Pre-fence rejection short-circuits before `launch` is
    /// recorded.
    #[test]
    fn pre_fence_rejection_blocks_launch() {
        let mut port = FakeJobProcessPort::new("test");
        port.set_pre_fence_fail("U-bad");
        let descriptor = JobDescriptor::new("U-bad", "j-1", "executor", Stage::Execute);
        let err = port
            .pre_fence(&descriptor)
            .expect_err("pre-fence must reject");
        assert!(matches!(err, RuntimeJobError::PreFenceFailed(_)));
        assert_eq!(port.launch_count(), 0);
    }

    /// Collect returns the enqueued result on first call and
    /// `CollectFailed` on subsequent calls.
    #[test]
    fn collect_returns_enqueued_result_then_collect_failed() {
        let port = FakeJobProcessPort::new("test");
        let h = port.launch(&fake_prompt("U1")).expect("launch");
        port.enqueue_result(
            h.pid(),
            ProcessResult::new(json!({"exit_code": 0}), Some(0), h.pid(), 5),
        );
        let r1 = port
            .collect_with_deadline(&*h, 0)
            .expect("first collect ok");
        assert_eq!(r1.exit_code, Some(0));
        let r2 = port.collect_with_deadline(&*h, 0);
        assert!(matches!(r2, Err(RuntimeJobError::CollectFailed(_))));
    }

    /// Cancel records the pid so per-attempt PID-tracking can
    /// assert.
    #[test]
    fn cancel_records_pid() {
        let port = FakeJobProcessPort::new("test");
        port.cancel(4242).expect("cancel ok");
        assert_eq!(
            port.cancels
                .lock()
                .expect("FakeJobProcessPort cancels mutex poisoned")
                .as_slice(),
            &[4242]
        );
    }
}
