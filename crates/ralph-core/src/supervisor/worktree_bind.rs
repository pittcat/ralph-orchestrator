//! 2026-07-03-001 plan U10: worktree binding helper.
//!
//! Bridge between the supervisor coordinator (U8) and the
//! existing `worktree` module. The runtime injects the
//! binding result into `SupervisorStore::bind_worktree`
//! before a slot dispatches. The function is **pure of
//! shell-out**: the production impl uses the existing
//! `worktree::create_worktree`; tests inject `WorktreeFactory`
//! to assert factory call args without touching git.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::worktree::{self, Worktree, WorktreeConfig};

use super::{IsolationMode, SlotResource, SupervisorStoreError, SupervisorStoreResult, WaveKind};

/// Bundle handed back to the dispatcher when a worktree slot
/// is bound. The envelope groups everything the coordinator
/// needs to spawn the worker without reaching back into git.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreeBinding {
    /// `slot_resources` row the store persists.
    pub resource: SlotResource,
    /// Environment variables injected into the worker's
    /// `Command::envs()` call (U13 / U14). The names are the
    /// SSOT for the worker contract — see `worker_env_keys`.
    pub env: HashMap<String, String>,
    /// Absolute worktree path resolved against the repo root.
    /// `None` for `SharedReadonly` slots.
    pub worktree_path: Option<PathBuf>,
}

/// Worker env keys. The constants here are the source of truth
/// for the dispatcher + worker handshake. Drift detection
/// happens through `worker_env_keys_match_docs`.
pub mod env_keys {
    /// `1` when the worker is a wave worker.
    pub const RALPH_WAVE_WORKER: &str = "RALPH_WAVE_WORKER";
    /// Path to the worker's bound worktree (empty string for
    /// `shared_readonly` slots).
    pub const RALPH_WAVE_WORKTREE_PATH: &str = "RALPH_WAVE_WORKTREE_PATH";
    /// Git branch the worker should commit on top of.
    pub const RALPH_WAVE_WORKTREE_BRANCH: &str = "RALPH_WAVE_WORKTREE_BRANCH";
    /// Wave correlation id (matches `wave_id`).
    pub const RALPH_WAVE_ID: &str = "RALPH_WAVE_ID";
    /// 0-based slot index inside the wave.
    pub const RALPH_WAVE_INDEX: &str = "RALPH_WAVE_INDEX";
    /// Logical worker dimension (`exec` / `fix` / `review`).
    pub const RALPH_WAVE_KIND: &str = "RALPH_WAVE_KIND";
    /// Returned in test for drift detection.
    pub const EXPECTED: &[&str] = &[
        RALPH_WAVE_WORKER,
        RALPH_WAVE_WORKTREE_PATH,
        RALPH_WAVE_WORKTREE_BRANCH,
        RALPH_WAVE_ID,
        RALPH_WAVE_INDEX,
        RALPH_WAVE_KIND,
    ];
}

/// Factory trait decoupled so tests can assert the call
/// without invoking `git worktree add`. Production callers
/// pass the default factory which calls
/// `worktree::create_worktree`.
pub trait WorktreeFactory: std::fmt::Debug + Send + Sync {
    fn create(&self, repo_root: PathBuf, branch: String) -> Result<Worktree, WorktreeError>;
}

/// Production default: delegates to the existing
/// `worktree::create_worktree`. The branch name encodes the
/// loop_id + wave_kind + slot_index so per-slot workers get
/// unique refs (U12 wires the loop_id from `RALPH_CURRENT_LOOP_ID`).
#[derive(Debug, Default)]
pub struct DefaultWorktreeFactory;

impl WorktreeFactory for DefaultWorktreeFactory {
    fn create(&self, repo_root: PathBuf, branch: String) -> Result<Worktree, WorktreeError> {
        let config = WorktreeConfig::default();
        worktree::create_worktree(&repo_root, &branch, &config).map_err(WorktreeError::from)
    }
}

/// Local error so the helper signature stays host-agnostic.
/// Production callers translate to `SupervisorStoreError`.
#[derive(Debug, thiserror::Error)]
pub enum WorktreeError {
    #[error("worktree module rejected: {0}")]
    CreateFailed(String),
    #[error("not a git repository: {0}")]
    NotARepo(String),
}

impl From<worktree::WorktreeError> for WorktreeError {
    fn from(err: worktree::WorktreeError) -> Self {
        match err {
            worktree::WorktreeError::NotARepo(p) => WorktreeError::NotARepo(p),
            other => WorktreeError::CreateFailed(other.to_string()),
        }
    }
}

/// Bind a slot to a worktree + worker env vars.
///
/// `kind == Review` returns a binding with no worktree (U2
/// contract: review slots are `SharedReadonly`). For
/// `Exec`/`Fix` the factory is invoked; the resulting
/// `Worktree`'s path becomes the `slot_resources.worktree_path`
/// AND `RALPH_WAVE_WORKTREE_PATH` env.
///
/// The function DOES NOT touch `SupervisorStore` — the caller
/// (dispatcher bridge in U12) is responsible for passing the
/// returned `resource` to `SupervisorStore::bind_worktree`.
pub fn bind_slot_worktree<F: WorktreeFactory>(
    factory: &F,
    repo_root: PathBuf,
    loop_id: &str,
    kind: WaveKind,
    wave_id: &str,
    slot_index: u32,
) -> SupervisorStoreResult<WorktreeBinding> {
    match kind {
        WaveKind::Review => Ok(review_binding(wave_id, slot_index, kind)),
        WaveKind::Exec | WaveKind::Fix => {
            exec_binding(factory, repo_root, loop_id, kind, wave_id, slot_index)
        }
    }
}

fn review_binding(wave_id: &str, slot_index: u32, kind: WaveKind) -> WorktreeBinding {
    let mut env = HashMap::new();
    env.insert(env_keys::RALPH_WAVE_WORKER.to_string(), "1".to_string());
    env.insert(
        env_keys::RALPH_WAVE_WORKTREE_PATH.to_string(),
        String::new(),
    );
    env.insert(
        env_keys::RALPH_WAVE_WORKTREE_BRANCH.to_string(),
        String::new(),
    );
    env.insert(env_keys::RALPH_WAVE_ID.to_string(), wave_id.to_string());
    env.insert(
        env_keys::RALPH_WAVE_INDEX.to_string(),
        slot_index.to_string(),
    );
    env.insert(env_keys::RALPH_WAVE_KIND.to_string(), kind.to_string());
    WorktreeBinding {
        resource: SlotResource {
            slot_index,
            worktree_path: None,
            branch: None,
        },
        env,
        worktree_path: None,
    }
}

fn exec_binding<F: WorktreeFactory>(
    factory: &F,
    repo_root: PathBuf,
    loop_id: &str,
    kind: WaveKind,
    wave_id: &str,
    slot_index: u32,
) -> SupervisorStoreResult<WorktreeBinding> {
    let branch = format!("{loop_id}-{kind}-{slot_index}");
    let wt = factory
        .create(repo_root.clone(), branch.clone())
        .map_err(|err| SupervisorStoreError::Storage(err.to_string()))?;
    let worktree_path = wt.path.clone();
    let mut env = HashMap::new();
    env.insert(env_keys::RALPH_WAVE_WORKER.to_string(), "1".to_string());
    env.insert(
        env_keys::RALPH_WAVE_WORKTREE_PATH.to_string(),
        worktree_path.to_string_lossy().into_owned(),
    );
    env.insert(
        env_keys::RALPH_WAVE_WORKTREE_BRANCH.to_string(),
        branch.clone(),
    );
    env.insert(env_keys::RALPH_WAVE_ID.to_string(), wave_id.to_string());
    env.insert(
        env_keys::RALPH_WAVE_INDEX.to_string(),
        slot_index.to_string(),
    );
    env.insert(env_keys::RALPH_WAVE_KIND.to_string(), kind.to_string());
    Ok(WorktreeBinding {
        resource: SlotResource {
            slot_index,
            worktree_path: Some(worktree_path.to_string_lossy().into_owned()),
            branch: Some(branch),
        },
        env,
        worktree_path: Some(worktree_path),
    })
}

/// Confirm `kind`'s `IsolationMode` matches the binding's
/// resource shape. Pin the invariant so future integrators
/// can't accidentally hand a `SlotResource {worktree_path:
/// None}` to an `Exec` slot (or vice versa).
pub fn assert_isolation_matches(
    kind: WaveKind,
    isolation: IsolationMode,
) -> SupervisorStoreResult<()> {
    let default_isolation = match kind {
        WaveKind::Exec | WaveKind::Fix => IsolationMode::Worktree,
        WaveKind::Review => IsolationMode::SharedReadonly,
    };
    if default_isolation != isolation {
        return Err(SupervisorStoreError::InvalidTransition(format!(
            "isolation {isolation:?} disagrees with kind {kind:?} default {default_isolation:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Spy factory: records every `create()` call without
    /// invoking git. Lets us pin branch naming + path passing.
    #[derive(Debug, Default)]
    struct SpyFactory {
        calls: Arc<Mutex<Vec<(PathBuf, String)>>>,
        result_worktree_path: PathBuf,
    }

    impl WorktreeFactory for SpyFactory {
        fn create(&self, repo_root: PathBuf, branch: String) -> Result<Worktree, WorktreeError> {
            self.calls
                .lock()
                .unwrap()
                .push((repo_root.clone(), branch.clone()));
            Ok(Worktree {
                path: self.result_worktree_path.join(&branch),
                branch,
                is_main: false,
                head: None,
            })
        }
    }

    fn spy_at(path: PathBuf) -> SpyFactory {
        SpyFactory {
            calls: Arc::new(Mutex::new(Vec::new())),
            result_worktree_path: path,
        }
    }

    #[test]
    fn exec_kind_injects_worker_env_keys() {
        let f = spy_at(PathBuf::from("/var/tmp/wt"));
        let binding = bind_slot_worktree(
            &f,
            PathBuf::from("/tmp/repo"),
            "loop-1",
            WaveKind::Exec,
            "w-1",
            0,
        )
        .unwrap();
        assert!(binding.worktree_path.is_some());
        for key in env_keys::EXPECTED {
            assert!(
                binding.env.contains_key(*key),
                "exec binding must include env key {key}, got {:?}",
                binding.env.keys().collect::<Vec<_>>()
            );
        }
        // Branch naming follows `{loop_id}-{kind}-{slot_index}`.
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "loop-1-exec-0");
    }

    #[test]
    fn review_kind_returns_no_worktree() {
        let f = spy_at(PathBuf::from("/var/tmp/wt"));
        let binding = bind_slot_worktree(
            &f,
            PathBuf::from("/tmp/repo"),
            "loop-1",
            WaveKind::Review,
            "w-1",
            0,
        )
        .unwrap();
        assert!(
            binding.worktree_path.is_none(),
            "review wave slots must be shared_readonly (no worktree)"
        );
        assert!(binding.resource.worktree_path.is_none());
        assert!(binding.resource.branch.is_none());
        let calls = f.calls.lock().unwrap();
        assert!(
            calls.is_empty(),
            "review binding must not invoke the factory"
        );
    }

    #[test]
    fn fix_kind_uses_kinded_branch_prefix() {
        let f = spy_at(PathBuf::from("/var/tmp/wt"));
        let binding = bind_slot_worktree(
            &f,
            PathBuf::from("/tmp/repo"),
            "loop-z",
            WaveKind::Fix,
            "w-99",
            3,
        )
        .unwrap();
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, "loop-z-fix-3");
        let _ = binding;
    }

    #[test]
    fn factory_failure_maps_to_storage_error() {
        #[derive(Debug)]
        struct FailFactory;
        impl WorktreeFactory for FailFactory {
            fn create(
                &self,
                _repo_root: PathBuf,
                _branch: String,
            ) -> Result<Worktree, WorktreeError> {
                Err(WorktreeError::CreateFailed("git unhappy".to_string()))
            }
        }
        let err = bind_slot_worktree(
            &FailFactory,
            PathBuf::from("/tmp/repo"),
            "loop-x",
            WaveKind::Exec,
            "w-1",
            0,
        )
        .unwrap_err();
        match err {
            SupervisorStoreError::Storage(msg) => assert!(msg.contains("git unhappy")),
            other => panic!("expected Storage, got {other:?}"),
        }
    }

    #[test]
    fn env_keys_constants_are_stable() {
        // `worker_env_keys_match_docs`: the constants are
        // referenced by both the worker prompt and the
        // docs/site guide; rename detection here.
        for key in env_keys::EXPECTED {
            assert!(key.starts_with("RALPH_WAVE_"));
        }
        assert_eq!(env_keys::EXPECTED.len(), 6);
    }

    #[test]
    fn isolation_matches_kind_round_trips() {
        assert!(assert_isolation_matches(WaveKind::Exec, IsolationMode::Worktree).is_ok());
        assert!(assert_isolation_matches(WaveKind::Fix, IsolationMode::Worktree).is_ok());
        assert!(assert_isolation_matches(WaveKind::Review, IsolationMode::SharedReadonly).is_ok());
        let err = assert_isolation_matches(WaveKind::Exec, IsolationMode::SharedReadonly);
        assert!(matches!(
            err,
            Err(SupervisorStoreError::InvalidTransition(_))
        ));
    }

    #[test]
    fn exec_binding_resource_captures_path_and_branch() {
        let f = spy_at(PathBuf::from("/var/tmp/wt"));
        let binding = bind_slot_worktree(
            &f,
            PathBuf::from("/tmp/repo"),
            "loop-2",
            WaveKind::Exec,
            "w-2",
            0,
        )
        .unwrap();
        assert!(binding.resource.worktree_path.is_some());
        assert_eq!(binding.resource.branch.as_deref(), Some("loop-2-exec-0"));
    }
}
