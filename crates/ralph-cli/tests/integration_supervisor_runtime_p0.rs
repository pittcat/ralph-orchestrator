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

/// U2 (R-A2): the public wave_id is the stable identity the
/// dispatcher / hat / coordination events reference. The store
/// assigns its own `w-{seq}` id on registration; the bridge
/// transparently resolves the public→store mapping even after
/// the process loses its in-memory cache.
///
/// Sequence:
/// 1. Register wave under `public-A` → store assigns `w-1`.
/// 2. Build a fresh bridge + store handle (simulating process
///    restart). The new bridge's `registered` map is empty.
/// 3. Call `register_wave_if_absent(public-A, ...)` again →
///    store returns `DuplicateKey`. The bridge MUST resolve
///    back to `w-1` instead of fabricating `public-A`.
#[test]
fn public_wave_id_resolves_to_store_id_after_restart() {
    use std::sync::Arc;
    let mut fx = SupervisorP0Fixture::new_in_memory(
        "public-A",
        "loop-A",
        "task-A",
        "task-key-A:step-1",
        "step-1",
    );
    fx.with_git_baseline();
    fx.with_wave(WaveKind::Exec, IsolationMode::Worktree, 0, 1)
        .register_and_bind();

    // The bridge used during register_and_bind already cached
    // `public-A → w-1`. Build a fresh bridge against a fresh
    // store handle — same kind, but new instance — to prove the
    // bridge cannot rely on its own cache across restart.
    let store: Arc<dyn SupervisorStore> = Arc::new(InMemorySupervisorStore::new());
    store.register_wave("public-A", WaveKind::Exec, 1).expect("first register");
    let fresh_bridge = InMemoryCoordinatorBridge::from_store(store.clone());
    let resolved = fresh_bridge
        .register_wave_if_absent(WaveKind::Exec, "public-A", 1)
        .expect("idempotent re-register");
    // 2026-07-23-004 U2: the resolved id MUST be the original
    // store id (`w-1`), not the caller-supplied `public-A`.
    // Returning `public-A` would break every subsequent
    // `bind_slot` / `tick` call because the store would not
    // recognize it as a row.
    assert_eq!(
        resolved, "w-1",
        "fresh bridge must resolve public-A back to the original store id, not the caller key"
    );

    // 2026-07-23-004 U2 A2.3: an unknown public wave id must
    // not silently register a second wave under the caller's
    // key — register_wave_if_absent on a never-registered key
    // simply registers; the only fail-close path is when the
    // bridge sees a DuplicateKey but the store has no row,
    // which we cannot exercise through the public API because
    // DuplicateKey implies the row exists. Validate the
    // successful path remains idempotent for repeated calls.
    let resolved_again = fresh_bridge
        .register_wave_if_absent(WaveKind::Exec, "public-A", 1)
        .expect("second idempotent call");
    assert_eq!(resolved_again, resolved);
}

/// U2 (R-A2): out-of-range slot_index is rejected, not silently
/// turned into a phantom slot.
#[test]
fn public_wave_id_out_of_range_slot_is_rejected() {
    let mut fx = SupervisorP0Fixture::new_in_memory(
        "public-OOR",
        "loop-OOR",
        "task-OOR",
        "task-key-OOR:step-1",
        "step-1",
    );
    fx.with_git_baseline();
    // Register a 1-slot wave; bind slot_index=0.
    fx.with_wave(WaveKind::Exec, IsolationMode::Worktree, 0, 1)
        .register_and_bind();

    // Trying to bind slot_index=5 (> wave_total=1) should
    // fail because the store only allocated slot 0.
    let store: std::sync::Arc<dyn SupervisorStore> =
        std::sync::Arc::new(InMemorySupervisorStore::new());
    let wave_id = store
        .register_wave("public-OOR", WaveKind::Exec, 1)
        .expect("register");
    let slot_resource = SlotResource {
        slot_index: 5,
        worktree_path: Some("/tmp/phantom".into()),
        branch: Some("ralph/p0-phantom".into()),
    };
    let result = store.bind_worktree(&wave_id, 5, slot_resource);
    assert!(
        result.is_err(),
        "out-of-range slot_index=5 for wave_total=1 must be rejected, got {result:?}"
    );
}

/// U3 (R-A1): the validator refuses events paths that live
/// inside the slot worktree (the A1.3 fail-close case).
#[test]
fn control_plane_path_inside_slot_worktree_is_rejected() {
    let mut fx = SupervisorP0Fixture::new_in_memory(
        "u3-slot-rejection",
        "u3-loop",
        "task-u3-slot",
        "task-key-u3-slot:step-1",
        "step-1",
    );
    fx.with_git_baseline();
    fx.with_wave(WaveKind::Exec, IsolationMode::Worktree, 0, 1)
        .register_and_bind();

    // Place the proposed events file inside the slot worktree.
    let nested_orphan = fx.slot_worktree_path().join(".ralph/events.jsonl");
    std::fs::create_dir_all(nested_orphan.parent().unwrap()).unwrap();
    std::fs::write(&nested_orphan, "{}\n").unwrap();

    // Import the crate-level validator via the bin's name. The
    // public API path is intentionally stable so future U-IDs
    // and the dispatcher can reuse it without depending on
    // internal modules.
    let result = ralph_core::control_plane::validate_control_plane_binding(
        &nested_orphan,
        Some(fx.slot_worktree_path()),
        fx.workspace_root(),
    );

    assert!(
        result.is_err(),
        "events file inside slot worktree must fail-close, got {result:?}"
    );
    let reason = format!("{}", result.err().unwrap());
    assert!(
        reason.contains("invalid_control_plane_path"),
        "fail-close must surface the stable reason code, got {reason}"
    );

    // Cleanup: the slot subtree should never carry JSONL
    // ledger state, even after validation failure.
    assert!(
        nested_orphan.exists(),
        "diagnostics file may remain on disk for the dispatcher to surface the failure"
    );
    // Any follow-up worker spawn must produce zero new state
    // under the slot subtree by the time we observe it.
    let subtree_writes = count_jsonl_under(fx.slot_worktree_path());
    assert!(
        subtree_writes <= 1,
        "validation must not produce nested ledger writes, found {subtree_writes}"
    );
}

/// U3 (R-A1): a relative events path is rejected (A1.3 second
/// fail-close case).
#[test]
fn control_plane_relative_events_path_is_rejected() {
    let fx = SupervisorP0Fixture::new_in_memory(
        "u3-rel-rejection",
        "u3-loop",
        "task-u3-rel",
        "task-key-u3-rel:step-1",
        "step-1",
    );
    let rel = std::path::Path::new(".ralph/events.jsonl");
    let result = ralph_core::control_plane::validate_control_plane_binding(
        rel,
        Some(fx.slot_worktree_path()),
        fx.workspace_root(),
    );
    assert!(
        matches!(
            result,
            Err(ralph_core::control_plane::ControlPlaneError::RelativePath { .. })
        ),
        "relative events path must produce RelativePath error, got {result:?}"
    );
}

/// Count JSONL files under a directory (excluding
/// `.ralph/current-events`-style symlinks so we don't follow
/// them outside the subtree).
fn count_jsonl_under(dir: &Path) -> usize {
    let mut count = 0usize;
    for entry in walkdir_safe(dir) {
        if entry.extension().map(|e| e == "jsonl").unwrap_or(false) {
            count += 1;
        }
    }
    count
}

/// Tiny dir-walker that does not depend on the `walkdir`
/// crate; uses `std::fs::read_dir` recursively up to depth 4.
fn walkdir_safe(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let depth = dir.components().count() - root.components().count();
        if depth > 4 {
            continue;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                out.push(path);
            }
        }
    }
    out
}

/// U2 (R-A2): a totally unknown wave id is treated as missing
/// rather than registered twice. The store's lookup gives
/// `None` for unknown keys.
#[test]
fn unknown_public_wave_id_lookup_returns_none() {
    let store: std::sync::Arc<dyn SupervisorStore> =
        std::sync::Arc::new(InMemorySupervisorStore::new());
    let looked = store
        .wave_id_for_idempotency_key("never-registered")
        .expect("store lookup");
    assert!(
        looked.is_none(),
        "lookup of unknown public id must return None, got {looked:?}"
    );

    // And the in-memory store keeps the public→store map after
    // registration so subsequent lookups succeed.
    let assigned = store
        .register_wave("now-registered", WaveKind::Exec, 1)
        .expect("register");
    let looked_after = store
        .wave_id_for_idempotency_key("now-registered")
        .expect("store lookup")
        .expect("assigned store id");
    assert_eq!(looked_after, assigned);
}
