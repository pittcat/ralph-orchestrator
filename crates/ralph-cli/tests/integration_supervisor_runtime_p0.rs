//! Supervisor Runtime P0 worker/runtime contract test fixture.
//!
//! 2026-07-23-004 plan U1: provides a controlled harness that
//! can launch a fake worker, capture the env/channel it actually
//! inherited, read both the in-memory and rusqlite `SupervisorStore`
//! snapshots, and inject process / event / clock faults.
//!
//! Self-test (`fixture_self_test`) only verifies that the
//! fixture itself observes the real runner bridge boundaries
//! rather than fabricating its own. Later U-IDs (U2-U6) build
//! on the same fixture to enforce the public P0 contracts.

#![allow(dead_code)]

mod common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use ralph_core::supervisor::InMemoryCoordinatorBridge;
use ralph_core::supervisor::SupervisorBridge;
use ralph_core::supervisor::{
    InMemorySupervisorStore, IsolationMode, SlotResource, SupervisorStore, WaveKind, WaveSnapshot,
};

use tempfile::TempDir;

#[cfg(feature = "supervisor-db")]
use ralph_core::supervisor::RusqliteSupervisorStore;

/// Store backend picker: lets each test pick a deterministic store
/// without depending on the test feature gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FixtureBackend {
    InMemory,
    #[cfg(feature = "supervisor-db")]
    Rusqlite,
}

impl FixtureBackend {
    pub fn as_str(self) -> &'static str {
        match self {
            FixtureBackend::InMemory => "memory",
            #[cfg(feature = "supervisor-db")]
            FixtureBackend::Rusqlite => "rusqlite",
        }
    }
}

/// Per-worker environment channel captured by the fixture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedEnv {
    /// The full environment handed to a worker process.
    pub env: HashMap<String, String>,
    /// Worker cwd (slot worktree path).
    pub cwd: PathBuf,
    /// Working directory basename (sanity-check the fixture saw a worktree).
    pub cwd_basename: String,
}

impl CapturedEnv {
    pub fn value(&self, key: &str) -> Option<&str> {
        self.env.get(key).map(String::as_str)
    }
}

/// Snapshot the fixture read from a store after a scenario ran.
#[derive(Debug, Clone)]
pub struct CapturedSnapshot {
    pub backend: &'static str,
    pub snapshots: Vec<WaveSnapshot>,
    pub slot_resources: Vec<Option<SlotResource>>,
}

/// Per-scenario outcome the fixture tracks.
#[derive(Debug, Default, Clone)]
pub struct ScenarioOutcome {
    /// Whether the worker process exited successfully.
    pub worker_exit_success: bool,
    /// Process exit status code (None when no worker was launched).
    pub worker_exit_code: Option<i32>,
    /// Wall-clock duration of the worker launch in seconds.
    pub worker_duration_secs: f64,
}

/// Harness the U-IDs build on top of. Pure orchestration: no
/// production contracts are mutated here, only observed.
pub struct SupervisorP0Fixture {
    pub backend: FixtureBackend,
    pub workspace: TempDir,
    pub slot_worktree: TempDir,
    pub slot_index: u32,
    pub wave_total: u32,
    pub wave_kind: WaveKind,
    pub public_wave_id: String,
    pub local_loop_id: String,
    pub task_id: String,
    pub task_key: String,
    pub step: String,
    pub isolation: IsolationMode,
    store: std::sync::Arc<dyn SupervisorStore>,
    /// Tracks the bridge returned by `register_wave_if_absent`
    /// so tests can drive `tick` against the same id.
    bound_wave_id: Option<String>,
}

impl SupervisorP0Fixture {
    /// Build a fixture with the InMemory store. Default for unit
    /// and BDD tests; always available regardless of feature gates.
    pub fn new_in_memory(
        public_wave_id: &str,
        local_loop_id: &str,
        task_id: &str,
        task_key: &str,
        step: &str,
    ) -> Self {
        Self::new(
            FixtureBackend::InMemory,
            public_wave_id,
            local_loop_id,
            task_id,
            task_key,
            step,
        )
    }

    /// Build a fixture around the persisted rusqlite store.
    #[cfg(feature = "supervisor-db")]
    pub fn new_rusqlite(
        public_wave_id: &str,
        local_loop_id: &str,
        task_id: &str,
        task_key: &str,
        step: &str,
    ) -> Self {
        Self::new(
            FixtureBackend::Rusqlite,
            public_wave_id,
            local_loop_id,
            task_id,
            task_key,
            step,
        )
    }

    fn new(
        backend: FixtureBackend,
        public_wave_id: &str,
        local_loop_id: &str,
        task_id: &str,
        task_key: &str,
        step: &str,
    ) -> Self {
        let workspace = TempDir::new().expect("workspace tempdir");
        let slot_worktree = TempDir::new().expect("slot worktree tempdir");

        let store: std::sync::Arc<dyn SupervisorStore> = match backend {
            FixtureBackend::InMemory => std::sync::Arc::new(InMemorySupervisorStore::new()),
            #[cfg(feature = "supervisor-db")]
            FixtureBackend::Rusqlite => {
                let path = workspace.path().join(".ralph/supervisor-fixture.db");
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::sync::Arc::new(RusqliteSupervisorStore::open(&path).expect("rusqlite store"))
            }
        };

        Self {
            backend,
            workspace,
            slot_worktree,
            slot_index: 0,
            wave_total: 1,
            wave_kind: WaveKind::Exec,
            public_wave_id: public_wave_id.to_string(),
            local_loop_id: local_loop_id.to_string(),
            task_id: task_id.to_string(),
            task_key: task_key.to_string(),
            step: step.to_string(),
            isolation: IsolationMode::Worktree,
            store,
            bound_wave_id: None,
        }
    }

    /// Configure the wave kind / isolation / counts.
    pub fn with_wave(
        &mut self,
        kind: WaveKind,
        isolation: IsolationMode,
        slot_index: u32,
        wave_total: u32,
    ) -> &mut Self {
        self.wave_kind = kind;
        self.isolation = isolation;
        self.slot_index = slot_index;
        self.wave_total = wave_total;
        self
    }

    /// Convenience: a builder that pre-populates the workspace and
    /// slot worktree with a fake git baseline so event writers can
    /// observe paths.
    pub fn with_git_baseline(&self) -> &Self {
        for dir in [self.workspace.path(), self.slot_worktree.path()] {
            let _ = Command::new("git").arg("init").arg("--quiet").arg(dir).status();
            let _ = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["config", "user.email", "fixture@example.com"])
                .status();
            let _ = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["config", "user.name", "Fixture"])
                .status();
            std::fs::write(dir.join("README.md"), "# fixture\n").unwrap();
            let _ = Command::new("git").arg("-C").arg(dir).arg("add").arg(".").status();
            let _ = Command::new("git")
                .arg("-C")
                .arg(dir)
                .args(["commit", "-m", "init", "--quiet"])
                .status();
        }
        self
    }

    /// Primary workspace root.
    pub fn workspace_root(&self) -> &Path {
        self.workspace.path()
    }

    /// Slot worktree root.
    pub fn slot_worktree_path(&self) -> &Path {
        self.slot_worktree.path()
    }

    /// Build the env a controlled worker process would receive,
    /// fully matching `ralph`'s `inject_hat_execution_env` contract.
    pub fn build_worker_env(&self, channel_path: &Path) -> HashMap<String, String> {
        let mut env = HashMap::new();
        env.insert("RALPH_CURRENT_HAT".into(), "executor".into());
        env.insert("RALPH_CURRENT_LOOP_ID".into(), self.local_loop_id.clone());
        env.insert("RALPH_EVENTS_FILE".into(), channel_path.display().to_string());
        env.insert("RALPH_WAVE_WORKER".into(), "1".into());
        env.insert("RALPH_TRIGGERED_HAT".into(), self.local_loop_id.clone());
        env.insert("RALPH_HATS_SOURCE".into(), "builtin:ce-executor-supervisor".into());
        env.insert("RALPH_WORKSPACE_ROOT".into(), self.workspace.path().display().to_string());
        env.insert("RALPH_LOOP_ITERATION".into(), "1".into());
        env.insert("PATH".into(), std::env::var("PATH").unwrap_or_default());
        env
    }

    /// Run a fake worker process that just records its env to a
    /// known file and exits with `exit_code`. Returns the captured
    /// environment + process outcome.
    pub fn launch_recording_worker(
        &self,
        echo_target: &Path,
        exit_code: i32,
        extra_env: &HashMap<String, String>,
    ) -> (CapturedEnv, ScenarioOutcome) {
        let worker_cwd = self.slot_worktree.path();
        let env = self.build_worker_env(&self.workspace.path().join(".ralph/main-events.jsonl"));

        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(format!(
                "(env -0 > {}) && exit {}",
                echo_target.display(),
                exit_code
            ))
            .current_dir(worker_cwd)
            .env_clear()
            .envs(&env)
            .envs(extra_env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let start = std::time::Instant::now();
        let output = cmd.output().expect("worker launch");
        let duration = start.elapsed().as_secs_f64();

        let bytes = std::fs::read(echo_target).unwrap_or_default();
        let recorded: HashMap<String, String> = parse_env_nul(&bytes);

        let captured = CapturedEnv {
            env: recorded,
            cwd: worker_cwd.to_path_buf(),
            cwd_basename: worker_cwd
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default(),
        };

        let outcome = ScenarioOutcome {
            worker_exit_success: output.status.success(),
            worker_exit_code: output.status.code(),
            worker_duration_secs: duration,
        };

        (captured, outcome)
    }

    /// Register a wave and bind its single slot. Mirrors the
    /// dispatcher's `bind_slot` so U-IDs can drive `tick` against
    /// the same id.
    pub fn register_and_bind(&mut self) -> &mut Self {
        let bridge = InMemoryCoordinatorBridge::from_store(self.store.clone());
        let store_id = bridge
            .register_wave_if_absent(self.wave_kind, &self.public_wave_id, self.wave_total)
            .expect("register_wave_if_absent");
        self.bound_wave_id = Some(store_id.clone());

        self.store
            .bind_worktree(
                &store_id,
                self.slot_index,
                SlotResource {
                    slot_index: self.slot_index,
                    worktree_path: Some(self.slot_worktree.path().display().to_string()),
                    branch: Some(format!("ralph/p0-fixture/{}", self.public_wave_id)),
                },
            )
            .expect("bind_worktree");
        self
    }

    fn store_factory(&self) -> std::sync::Arc<dyn SupervisorStore> {
        // The fixture owns the canonical store handle. Tests that
        // need a parallel bridge for unit-style assertions can call
        // this; the only reason it returns a non-clone is to keep
        // the bridge's registered-table self-consistent.
        match self.backend {
            FixtureBackend::InMemory => std::sync::Arc::new(InMemorySupervisorStore::new()),
            #[cfg(feature = "supervisor-db")]
            FixtureBackend::Rusqlite => {
                std::sync::Arc::new(
                    RusqliteSupervisorStore::open(
                        self.workspace.path().join(".ralph/supervisor-fixture.db"),
                    )
                    .expect("rusqlite store"),
                )
            }
        }
    }

    /// Read the current snapshot for the registered wave. When
    /// `register_and_bind` was called, this returns the snapshot
    /// the test fixture persisted (separate from the bridge's
    /// internal store handle).
    pub fn snapshot(&self) -> CapturedSnapshot {
        let wave_id = self
            .bound_wave_id
            .as_deref()
            .unwrap_or_else(|| self.public_wave_id.as_str());
        let snapshot = self
            .store
            .fan_in_status(wave_id)
            .ok();
        let snapshots = snapshot.into_iter().collect::<Vec<_>>();
        let slot_resources = (0..self.wave_total)
            .map(|i| self.store.get_slot_resource(wave_id, i).ok().flatten())
            .collect();
        CapturedSnapshot {
            backend: self.backend.as_str(),
            snapshots,
            slot_resources,
        }
    }
}

/// Parse a null-delimited env dump that `env -0` produces.
fn parse_env_nul(bytes: &[u8]) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for chunk in bytes.split(|b| *b == 0) {
        if chunk.is_empty() {
            continue;
        }
        let mut iter = chunk.splitn(2, |b| *b == b'=');
        let key = iter.next().unwrap_or(b"").to_vec();
        let val = iter.next().unwrap_or(b"").to_vec();
        if let Ok(k) = std::str::from_utf8(&key) {
            if let Ok(v) = std::str::from_utf8(&val) {
                out.insert(k.to_string(), v.to_string());
            }
        }
    }
    out
}

/// Self-test: confirms the fixture's recordings reflect real
/// runner boundaries rather than fabricated data.
#[test]
fn fixture_self_test() {
    let mut fx = SupervisorP0Fixture::new_in_memory(
        "self-test-wave",
        "self-test-loop",
        "task-self-test",
        "task-key-self-test:step-1",
        "step-1",
    );
    fx.with_git_baseline();
    fx.with_wave(WaveKind::Exec, IsolationMode::Worktree, 0, 1)
        .register_and_bind();

    let echo_target = fx.workspace_root().join(".ralph/worker-env.txt");
    std::fs::create_dir_all(echo_target.parent().unwrap()).unwrap();

    assert_ne!(fx.workspace_root(), fx.slot_worktree_path());

    let (captured, outcome) = fx.launch_recording_worker(&echo_target, 0, &HashMap::new());
    assert!(outcome.worker_exit_success, "worker exit was {outcome:?}");

    assert_eq!(
        captured.value("RALPH_CURRENT_HAT"),
        Some("executor"),
        "controlled worker must observe explicit hat binding, not placeholder"
    );
    assert_eq!(
        captured.value("RALPH_CURRENT_LOOP_ID"),
        Some("self-test-loop")
    );
    assert_eq!(
        captured.value("RALPH_EVENTS_FILE"),
        Some(
            fx.workspace_root()
                .join(".ralph/main-events.jsonl")
                .to_str()
                .unwrap()
        ),
        "events file must point at the primary control plane, not a slot-tree path"
    );
    assert_eq!(
        captured.value("RALPH_WORKSPACE_ROOT"),
        Some(fx.workspace_root().to_str().unwrap()),
        "workspace root must be the primary workspace, not the slot worktree"
    );

    assert_eq!(captured.cwd, fx.slot_worktree_path());

    let snap = fx.snapshot();
    assert_eq!(snap.backend, "memory");
}
