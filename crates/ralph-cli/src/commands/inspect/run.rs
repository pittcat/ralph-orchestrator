//! `ralph inspect run` — aggregate run observation (run-observation/v1).
//!
//! feat(monitor) foundation plan (2026-10-28-1949) Unit 1: emits the
//! stable observation contract the monitor service collects. The
//! command enumerates runs from the workspace's evidence sources —
//! `.ralph/runs/*.manifest.json` (authoritative when present), the
//! loop registry, the primary `loop.lock`, and the merge queue — and
//! normalizes each run into a coarse `running | exited | unknown`
//! state. Fine-grained health classification (collector / runtime /
//! backend layers, `exited_merged` vs `exited_success` vs
//! `exited_unknown`) is the monitor's job; this command only ships
//! evidence.
//!
//! Read-only: the command never creates `.ralph/` files and never
//! holds locks — registry and queue files are parsed directly so
//! `inspect run` cannot mutate the workspace it observes.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use chrono::{DateTime, Utc};
use clap::Parser;
use ralph_core::loop_registry::LoopEntry;
use ralph_core::merge_queue::{MergeQueue, MergeState};
use ralph_core::run_manifest::{self, RunManifest};
use ralph_core::truncate_with_ellipsis;
use ralph_core::{LockMetadata, LoopLock};
use serde::Serialize;

use super::InspectProfilesFormat;
use super::descriptor::{self, DescriptorResolution};

/// Observation envelope schema tag.
pub const OBSERVATION_SCHEMA_VERSION: &str = "run-observation/v1";

/// Arguments for `ralph inspect run`.
#[derive(Parser, Debug)]
pub struct InspectRunArgs {
    /// Workspace root (default: current directory).
    #[arg(long)]
    pub root: Option<PathBuf>,

    /// Output format (human or json). JSON is the stable
    /// `run-observation/v1` contract consumed by the monitor
    /// collector.
    #[arg(long, value_enum, default_value_t = InspectProfilesFormat::Human)]
    pub format: InspectProfilesFormat,
}

/// Terminal evidence normalized for the observation payload.
#[derive(Debug, Clone, Serialize)]
pub struct TerminalObservation {
    /// `TerminationReason::as_str()` vocabulary.
    pub reason: String,
    /// When the terminal record was written.
    pub at: DateTime<Utc>,
    /// In-loop merge commit, when produced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_commit: Option<String>,
}

/// Evidence flags describing which sources a run was observed from.
///
/// Monitors use these to distinguish "run converged" (terminal +
/// manifest) from "run vanished" (dead pid, no manifest, no terminal)
/// without trusting any single file.
#[derive(Debug, Clone, Default, Serialize)]
pub struct RunEvidence {
    /// `.ralph/runs/<id>.manifest.json` parsed successfully.
    pub manifest_present: bool,
    /// A manifest file exists but failed to parse.
    #[serde(skip_serializing_if = "is_false")]
    pub manifest_corrupt: bool,
    /// The run appears in `.ralph/loops.json`.
    #[serde(skip_serializing_if = "is_false")]
    pub registry_present: bool,
    /// The primary `loop.lock` contributed this run.
    #[serde(skip_serializing_if = "is_false")]
    pub lock_present: bool,
    /// Merge queue state for this run id, when queued.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_state: Option<String>,
}

fn is_false(v: &bool) -> bool {
    !*v
}

// ── Plan5-U1 capability declarations ───────────────────────────────

/// Topics the `emit` action accepts (v1 whitelist, D2). Both topics
/// have real runtime consumers: `loop.cancel` maps to the Cancelled
/// terminal and `task.resume` is used by the recovery responder
/// (ralph-proto `topics.rs`).
pub const EMIT_TOPICS: &[&str] = &["loop.cancel", "task.resume"];

/// One declared capability (`inspect run` `actions[]` element, D1).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ActionDecl {
    /// `stop` | `retry` | `merge` | `emit`.
    pub id: String,
    /// Whether the action may be dispatched right now.
    pub available: bool,
    /// Human-readable reason when `available` is false.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Evidence hash the caller must echo back as `expected_revision`
    /// on mutations (D5). Computed over the same evidence snapshot so
    /// a state flip changes it.
    pub revision: String,
    /// Typed input contract; only `emit` declares one in v1.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<serde_json::Value>,
}

/// `sha256` evidence hash (D5): `run_id|state|manifest_revision|
/// merge_state|pending_merge_count`. Exposed `pub(crate)` so the
/// `loops --format json` receipts can recompute `revision_after` with
/// the identical function instead of drifting apart.
pub(crate) fn compute_revision(
    run_id: &str,
    state: &str,
    manifest_revision: u64,
    merge_state: Option<&str>,
    pending_merge_count: usize,
) -> String {
    use sha2::{Digest, Sha256};
    let mut sha = Sha256::new();
    sha.update(
        format!(
            "{}|{}|{}|{}|{}",
            run_id,
            state,
            manifest_revision,
            merge_state.unwrap_or("none"),
            pending_merge_count,
        )
        .as_bytes(),
    );
    format!("{:x}", sha.finalize())
}

/// Action availability rules (D2, mirrored from `loops.rs`):
///
/// - `stop`: the owning pid must be alive — `loops stop` resolves the
///   lock and signals it; a dead/unknown run has nothing to signal.
/// - `retry`: `loops retry` is `retry_merge` — it only re-runs a merge
///   whose queue entry is `needs_review` (anything else bails).
/// - `merge`: `loops merge` requires a stopped loop and either a
///   queued/needs-review queue entry (gated by the same
///   `merge_button_state` the UI button uses) or an orphan worktree it
///   can enqueue; `merging`/`merged`/`discarded` and live runs bail.
///   The primary run has no merge semantics at all.
/// - `emit`: `ralph emit --file` writes to the run's events file, so
///   both a live pid and a known events file are required; topics are
///   whitelisted (v1: `loop.cancel`, `task.resume`).
#[allow(clippy::too_many_arguments)]
fn compute_actions(
    run_id: &str,
    kind: &str,
    pid_alive: bool,
    state: &str,
    merge_state: Option<&str>,
    pending_merge_count: usize,
    manifest_revision: u64,
    events_file_present: bool,
    workspace: &Path,
) -> Vec<ActionDecl> {
    let revision = compute_revision(
        run_id,
        state,
        manifest_revision,
        merge_state,
        pending_merge_count,
    );
    let decl =
        |id: &str, available: bool, reason: Option<String>, schema: Option<serde_json::Value>| {
            ActionDecl {
                id: id.to_string(),
                available,
                reason,
                revision: revision.clone(),
                input_schema: schema,
            }
        };

    let stop = if pid_alive {
        decl("stop", true, None, None)
    } else if state == "exited" {
        decl("stop", false, Some("run already exited".into()), None)
    } else {
        decl(
            "stop",
            false,
            Some("process state unknown — nothing verified to signal".into()),
            None,
        )
    };

    let retry = match merge_state {
        Some("needs_review") => decl("retry", true, None, None),
        Some(s) => decl(
            "retry",
            false,
            Some(format!(
                "merge state is '{s}' — only needs-review can be retried"
            )),
            None,
        ),
        None => decl(
            "retry",
            false,
            Some("run has no merge queue entry".into()),
            None,
        ),
    };

    let merge = if kind == "primary" {
        decl(
            "merge",
            false,
            Some("primary run has no merge semantics".into()),
            None,
        )
    } else if pid_alive {
        decl(
            "merge",
            false,
            Some("loop still running — stop it first".into()),
            None,
        )
    } else {
        match merge_state {
            Some("merged") => decl("merge", false, Some("already merged".into()), None),
            Some("discarded") => decl("merge", false, Some("loop discarded".into()), None),
            Some("merging") => decl(
                "merge",
                false,
                Some("merge already in progress".into()),
                None,
            ),
            Some(_) => {
                // queued / needs_review: defer to the same state the
                // merge button reports so the two paths never disagree.
                match ralph_core::merge_queue::merge_button_state(workspace, run_id) {
                    Ok(ralph_core::MergeButtonState::Active) => decl("merge", true, None, None),
                    Ok(ralph_core::MergeButtonState::Blocked { reason }) => {
                        decl("merge", false, Some(reason), None)
                    }
                    Err(e) => decl(
                        "merge",
                        false,
                        Some(format!("merge state unknown: {e}")),
                        None,
                    ),
                }
            }
            None if kind == "worktree" => {
                // `loops merge` enqueues orphan worktrees on demand —
                // available with the caveat that a missing branch
                // surfaces at dispatch time.
                decl("merge", true, None, None)
            }
            _ => decl("merge", false, Some("not in merge queue".into()), None),
        }
    };

    let emit_schema = serde_json::json!({ "topics": EMIT_TOPICS });
    let emit = if pid_alive && events_file_present {
        decl("emit", true, None, Some(emit_schema))
    } else if !pid_alive {
        decl(
            "emit",
            false,
            Some("run already exited — no live events file".into()),
            Some(emit_schema),
        )
    } else {
        decl(
            "emit",
            false,
            Some("no events file known for this run".into()),
            Some(emit_schema),
        )
    };

    vec![stop, retry, merge, emit]
}

/// One normalized run in the observation payload.
#[derive(Debug, Clone, Serialize)]
pub struct RunObservation {
    /// Stable run identity (`primary-*` / `loop-*`); never `"(primary)"`.
    pub run_id: String,
    /// `primary` or `worktree`.
    pub kind: String,
    /// Coarse v1 vocabulary: `running` | `exited` | `unknown`.
    pub state: String,
    /// Owning process id, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo_root: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub worktree_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_preview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// Resolved `monitor-descriptor/v1` sidecar for the run's preset
    /// (`null` when none resolved). `builtin`/`workspace`/`none`/
    /// `invalid`/`unsupported_version` provenance lives in
    /// `preset_descriptor_from`.
    pub preset_descriptor: Option<serde_json::Value>,
    /// Provenance of `preset_descriptor`; `"none"` when the run has no
    /// preset or no matching sidecar.
    pub preset_descriptor_from: String,
    /// Resolver diagnostic for `invalid`/`unsupported_version`
    /// outcomes — the field that explains *why* no descriptor shipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset_descriptor_diagnostic: Option<String>,
    /// Sanitized DAG scheduler summary passthrough — emitted only
    /// when the run's workspace carries a durable `.ralph/dag.db`
    /// (i.e. the run observed `scheduler_mode: dag`). Same shape as
    /// `inspect loop`'s `scheduler` block so monitor consumers share
    /// one schema.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<ralph_core::supervisor::SchedulerInspectSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub started_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub events_file: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal: Option<TerminalObservation>,
    /// Manifest file path (relative to the runs-dir parent when it
    /// lives inside the inspected workspace, absolute otherwise).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest: Option<String>,
    /// Plan3-U1 activity ledger — present only when the file exists
    /// (a run that never reached a backend call has none).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity_file: Option<String>,
    /// Plan5-U1 capability declarations — the monitor's operation
    /// menu is rendered verbatim from this list (D1).
    pub actions: Vec<ActionDecl>,
    /// Plan5-U2: manifest `client_request_id` passthrough — the
    /// monitor matches a launch operation to the run it spawned.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
    pub evidence: RunEvidence,
}

#[derive(Serialize)]
struct InspectRunOutput {
    schema_version: &'static str,
    workspace: PathBuf,
    generated_at: DateTime<Utc>,
    runs: Vec<RunObservation>,
}

/// Everything `aggregate_runs` needs, gathered from the filesystem.
///
/// Kept as a plain struct so the aggregation function itself is pure
/// and testable with synthetic fixtures.
struct GatheredEvidence {
    manifests: Vec<run_manifest::DiscoveredManifest>,
    registry_entries: Vec<LoopEntry>,
    lock: Option<LockMetadata>,
    current_loop_id: Option<String>,
    current_events: Option<String>,
    merge_states: BTreeMap<String, MergeState>,
}

/// Registry file shape (`{"loops": [LoopEntry, ...]}`).
#[derive(serde::Deserialize)]
struct RegistryFile {
    #[serde(default)]
    loops: Vec<LoopEntry>,
}

/// Read `.ralph/loops.json` without creating it or holding a lock.
fn read_registry_entries(workspace: &Path) -> Vec<LoopEntry> {
    let path = workspace.join(ralph_core::LoopRegistry::REGISTRY_FILE);
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    serde_json::from_str::<RegistryFile>(&raw)
        .map(|f| f.loops)
        .unwrap_or_default()
}

fn read_trimmed(path: &Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Collect every manifest directory worth scanning: the workspace's
/// own `.ralph/runs/`, each registered worktree's `.ralph/runs/`, and
/// any `.worktrees/<name>/.ralph/runs/` still on disk (covers loops
/// whose registry entry was pruned after the pid died).
fn manifest_dirs(workspace: &Path, registry: &[LoopEntry]) -> Vec<PathBuf> {
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
    dirs.insert(run_manifest::runs_dir(workspace));
    for entry in registry {
        if let Some(ref wt) = entry.worktree_path {
            dirs.insert(run_manifest::runs_dir(Path::new(wt)));
        }
        let ws = Path::new(&entry.workspace);
        if ws != workspace {
            dirs.insert(run_manifest::runs_dir(ws));
        }
    }
    if let Ok(children) = std::fs::read_dir(workspace.join(".worktrees")) {
        for child in children.flatten() {
            let p = child.path();
            if p.is_dir() {
                dirs.insert(run_manifest::runs_dir(&p));
            }
        }
    }
    dirs.into_iter().collect()
}

fn gather_evidence(workspace: &Path) -> GatheredEvidence {
    let registry_entries = read_registry_entries(workspace);

    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut manifests = Vec::new();
    for dir in manifest_dirs(workspace, &registry_entries) {
        for m in run_manifest::discover_all(&dir) {
            if seen.insert(m.run_id.clone()) {
                manifests.push(m);
            }
        }
    }

    let lock = LoopLock::read_existing(workspace).ok().flatten();
    let ralph_dir = workspace.join(".ralph");
    let current_loop_id = read_trimmed(&ralph_dir.join("current-loop-id"));
    let current_events = read_trimmed(&ralph_dir.join("current-events"));
    let merge_states: BTreeMap<String, MergeState> = MergeQueue::new(workspace)
        .list()
        .unwrap_or_default()
        .into_iter()
        .map(|e| (e.loop_id, e.state))
        .collect();

    GatheredEvidence {
        manifests,
        registry_entries,
        lock,
        current_loop_id,
        current_events,
        merge_states,
    }
}

fn merge_state_str(state: MergeState) -> &'static str {
    match state {
        MergeState::Queued => "queued",
        MergeState::Merging => "merging",
        MergeState::Merged => "merged",
        MergeState::NeedsReview => "needs_review",
        MergeState::Discarded => "discarded",
    }
}

/// Pending merge-queue load — feeds the revision hash (D5) so any
/// queue transition invalidates outstanding capabilities.
fn pending_merge_count(ev: &GatheredEvidence) -> usize {
    ev.merge_states
        .values()
        .filter(|s| matches!(s, MergeState::Queued | MergeState::Merging))
        .count()
}

/// Populate `actions` on a finished observation from the evidence we
/// already collected (alive flag + manifest revision come from the
/// source-specific constructor).
fn attach_actions(
    obs: &mut RunObservation,
    pid_alive: bool,
    manifest_revision: u64,
    ev: &GatheredEvidence,
    workspace: &Path,
) {
    let events_file_present = obs
        .events_file
        .as_ref()
        .is_some_and(|p| PathBuf::from(p).exists());
    obs.actions = compute_actions(
        &obs.run_id,
        &obs.kind,
        pid_alive,
        &obs.state,
        obs.evidence.merge_state.as_deref(),
        pending_merge_count(ev),
        manifest_revision,
        events_file_present,
        workspace,
    );
}

/// Coarse v1 state: the pid evidence decides liveness; terminal
/// evidence decides convergence; neither → `unknown`.
fn coarse_state(pid_alive: bool, terminal_present: bool) -> &'static str {
    if pid_alive {
        "running"
    } else if terminal_present {
        "exited"
    } else {
        "unknown"
    }
}

fn manifest_display_path(workspace: &Path, path: &Path) -> String {
    path.strip_prefix(workspace)
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

/// Plan3-U1 activity ledger path for a run — the file only exists
/// once the first backend call wrote its `start` record.
fn activity_file_for(workspace: &Path, run_id: &str) -> Option<PathBuf> {
    let p = workspace
        .join(".ralph")
        .join("runs")
        .join(format!("{run_id}.activity.jsonl"));
    p.exists().then_some(p)
}

/// Passthrough for `inspect loop`'s `scheduler` block, scoped to a
/// run's workspace. Emitted only when `.ralph/dag.db` exists — the
/// durable store is written exclusively by `scheduler_mode: dag`, so
/// presence alone is the non-wave signal (no config load needed).
/// Best-effort: unreadable stores degrade to an absent key, matching
/// the read-only/no-abort contract of the rest of this command.
fn scheduler_passthrough(
    run_workspace: &Path,
) -> Option<ralph_core::supervisor::SchedulerInspectSummary> {
    let db_path = crate::loop_runner::dag_scheduler::integration::dag_store_path(run_workspace);
    if !db_path.exists() {
        return None;
    }
    #[cfg(feature = "supervisor-db")]
    {
        use ralph_core::supervisor::dag_plan_receipt::DagPlanReceiptStore as _;
        if let Ok(store) =
            ralph_core::supervisor::dag_store_rusqlite::RusqliteDagPlanReceiptStore::open(&db_path)
            && let Ok(receipts) = store.list_receipts()
        {
            return Some(
                ralph_core::supervisor::SchedulerInspectSummary::from_receipt_rows(
                    &receipts, "dag",
                ),
            );
        }
    }
    // Store present but unreadable (or supervisor-db off): still
    // surface the mode marker so consumers know DAG was configured.
    Some(ralph_core::supervisor::SchedulerInspectSummary::for_mode(
        "dag",
    ))
}

fn observation_from_manifest(
    m: &RunManifest,
    manifest_path: &Path,
    workspace: &Path,
    ev: &GatheredEvidence,
) -> RunObservation {
    let alive = run_manifest::pid_alive(m.pid);
    let registry_present = ev.registry_entries.iter().any(|e| e.id == m.run_id);
    let resolution: DescriptorResolution = descriptor::resolve(m.preset.as_deref(), workspace);
    let mut obs = RunObservation {
        run_id: m.run_id.clone(),
        kind: m.kind.as_str().to_string(),
        state: coarse_state(alive, m.terminal.is_some()).to_string(),
        pid: Some(m.pid),
        cwd: Some(m.cwd.clone()),
        workspace: Some(m.workspace.clone()),
        repo_root: Some(m.repo_root.clone()),
        worktree_path: m.worktree_path.clone(),
        prompt_preview: Some(m.prompt_preview.clone()),
        prompt_sha256: Some(m.prompt_sha256.clone()),
        preset: m.preset.clone(),
        preset_descriptor: descriptor::resolution_json(&resolution),
        preset_descriptor_from: resolution.resolved_from.to_string(),
        preset_descriptor_diagnostic: resolution.diagnostic.clone(),
        scheduler: scheduler_passthrough(&m.workspace),
        plan_path: m.plan_path.clone(),
        started_at: Some(m.started_at),
        events_file: m.events_file.clone(),
        terminal: m.terminal.as_ref().map(|t| TerminalObservation {
            reason: t.reason.clone(),
            at: t.at,
            merge_commit: t.merge_commit.clone(),
        }),
        manifest: Some(manifest_display_path(workspace, manifest_path)),
        activity_file: activity_file_for(&m.workspace, &m.run_id)
            .map(|p| manifest_display_path(&m.workspace, &p)),
        evidence: RunEvidence {
            manifest_present: true,
            registry_present,
            merge_state: ev
                .merge_states
                .get(&m.run_id)
                .map(|s| merge_state_str(*s).to_string()),
            ..RunEvidence::default()
        },
        actions: Vec::new(),
        client_request_id: m.client_request_id.clone(),
    };
    attach_actions(&mut obs, alive, m.revision, ev, workspace);
    obs
}

fn observation_from_registry(
    entry: &LoopEntry,
    workspace: &Path,
    ev: &GatheredEvidence,
) -> RunObservation {
    let alive = entry.is_pid_alive();
    let events_file =
        read_trimmed(&Path::new(&entry.workspace).join(".ralph/current-events")).map(PathBuf::from);
    let mut obs = RunObservation {
        run_id: entry.id.clone(),
        kind: if entry.worktree_path.is_some() {
            "worktree".to_string()
        } else {
            "primary".to_string()
        },
        state: coarse_state(alive, false).to_string(),
        pid: Some(entry.pid),
        cwd: None,
        workspace: Some(PathBuf::from(&entry.workspace)),
        repo_root: Some(workspace.to_path_buf()),
        worktree_path: entry.worktree_path.as_ref().map(PathBuf::from),
        prompt_preview: Some(truncate_with_ellipsis(entry.prompt.trim(), 160)),
        prompt_sha256: Some(run_manifest::prompt_sha256_hex(&entry.prompt)),
        preset: None,
        preset_descriptor: None,
        preset_descriptor_from: "none".to_string(),
        preset_descriptor_diagnostic: None,
        scheduler: None,
        plan_path: None,
        started_at: Some(entry.started),
        events_file,
        terminal: None,
        manifest: None,
        activity_file: None,
        evidence: RunEvidence {
            registry_present: true,
            merge_state: ev
                .merge_states
                .get(&entry.id)
                .map(|s| merge_state_str(*s).to_string()),
            ..RunEvidence::default()
        },
        actions: Vec::new(),
        client_request_id: None,
    };
    attach_actions(&mut obs, alive, 0, ev, workspace);
    obs
}

/// Aggregate gathered evidence into normalized run observations.
///
/// Pure over `ev`: every filesystem quirk (corrupt manifest, stale
/// lock, registry-only run) is decided here so tests can drive the
/// states without spawning a real loop.
fn aggregate_runs(ev: &GatheredEvidence, workspace: &Path) -> Vec<RunObservation> {
    let mut runs: BTreeMap<String, RunObservation> = BTreeMap::new();
    let registry_ids: BTreeSet<&str> = ev.registry_entries.iter().map(|e| e.id.as_str()).collect();

    for discovered in &ev.manifests {
        let run_id = discovered.run_id.clone();
        match &discovered.manifest {
            Some(m) => {
                runs.insert(
                    run_id,
                    observation_from_manifest(m, &discovered.path, workspace, ev),
                );
            }
            None => {
                let mut obs = RunObservation {
                    run_id: run_id.clone(),
                    kind: "unknown".to_string(),
                    state: "unknown".to_string(),
                    pid: None,
                    cwd: None,
                    workspace: None,
                    repo_root: None,
                    worktree_path: None,
                    prompt_preview: None,
                    prompt_sha256: None,
                    preset: None,
                    preset_descriptor: None,
                    preset_descriptor_from: "none".to_string(),
                    preset_descriptor_diagnostic: None,
                    scheduler: None,
                    plan_path: None,
                    started_at: None,
                    events_file: None,
                    terminal: None,
                    manifest: Some(manifest_display_path(workspace, &discovered.path)),
                    activity_file: activity_file_for(workspace, &discovered.run_id)
                        .map(|p| manifest_display_path(workspace, &p)),
                    evidence: RunEvidence {
                        manifest_corrupt: true,
                        registry_present: registry_ids.contains(discovered.run_id.as_str()),
                        merge_state: ev
                            .merge_states
                            .get(&discovered.run_id)
                            .map(|s| merge_state_str(*s).to_string()),
                        ..RunEvidence::default()
                    },
                    actions: Vec::new(),
                    client_request_id: None,
                };
                attach_actions(&mut obs, false, 0, ev, workspace);
                runs.insert(run_id, obs);
            }
        }
    }

    for entry in &ev.registry_entries {
        if !runs.contains_key(&entry.id) {
            runs.insert(
                entry.id.clone(),
                observation_from_registry(entry, workspace, ev),
            );
        }
    }

    // Legacy primary evidence: a `current-loop-id` marker or live
    // `loop.lock` without a manifest still identifies the run.
    if let Some(loop_id) = ev.current_loop_id.clone().or_else(|| {
        ev.lock
            .as_ref()
            .map(|l| format!("primary-{}", l.started.format("%Y%m%d-%H%M%S")))
    }) && !runs.contains_key(&loop_id)
    {
        let alive = ev
            .lock
            .as_ref()
            .map(|l| run_manifest::pid_alive(l.pid))
            .unwrap_or(false);
        let (pid, prompt, started) = ev
            .lock
            .as_ref()
            .map(|l| (Some(l.pid), Some(l.prompt.clone()), Some(l.started)))
            .unwrap_or((None, None, None));
        let mut obs = RunObservation {
            run_id: loop_id.clone(),
            kind: "primary".to_string(),
            state: coarse_state(alive, false).to_string(),
            pid,
            cwd: Some(workspace.to_path_buf()),
            workspace: Some(workspace.to_path_buf()),
            repo_root: Some(workspace.to_path_buf()),
            worktree_path: None,
            prompt_preview: prompt
                .as_deref()
                .map(|p| truncate_with_ellipsis(p.trim(), 160)),
            prompt_sha256: prompt.as_deref().map(run_manifest::prompt_sha256_hex),
            preset: None,
            preset_descriptor: None,
            preset_descriptor_from: "none".to_string(),
            preset_descriptor_diagnostic: None,
            scheduler: None,
            plan_path: None,
            started_at: started,
            events_file: ev.current_events.as_ref().map(PathBuf::from),
            terminal: None,
            manifest: None,
            activity_file: activity_file_for(workspace, &loop_id)
                .map(|p| manifest_display_path(workspace, &p)),
            evidence: RunEvidence {
                lock_present: ev.lock.is_some(),
                registry_present: registry_ids.contains(loop_id.as_str()),
                merge_state: ev
                    .merge_states
                    .get(&loop_id)
                    .map(|s| merge_state_str(*s).to_string()),
                ..RunEvidence::default()
            },
            actions: Vec::new(),
            client_request_id: None,
        };
        attach_actions(&mut obs, alive, 0, ev, workspace);
        runs.insert(loop_id, obs);
    }

    let mut list: Vec<RunObservation> = runs.into_values().collect();
    list.sort_by(|a, b| {
        b.started_at
            .cmp(&a.started_at)
            .then_with(|| a.run_id.cmp(&b.run_id))
    });
    list
}

/// Print `runs` as a fixed-column table.
fn print_human(workspace: &Path, runs: &[RunObservation]) {
    println!("workspace: {}", workspace.display());
    if runs.is_empty() {
        println!("no runs observed");
        return;
    }
    println!(
        "{:<34} {:<9} {:<8} {:<8} {:<20} PROMPT",
        "RUN_ID", "KIND", "STATE", "PID", "TERMINAL"
    );
    for r in runs {
        let pid = r.pid.map(|p| p.to_string()).unwrap_or_else(|| "-".into());
        let terminal = r
            .terminal
            .as_ref()
            .map(|t| t.reason.clone())
            .unwrap_or_else(|| "-".into());
        let prompt = r
            .prompt_preview
            .as_deref()
            .map(|p| truncate_with_ellipsis(p, 40))
            .unwrap_or_else(|| "-".into());
        println!(
            "{:<34} {:<9} {:<8} {:<8} {:<20} {}",
            truncate_with_ellipsis(&r.run_id, 33),
            r.kind,
            r.state,
            pid,
            truncate_with_ellipsis(&terminal, 19),
            prompt,
        );
    }
}

/// Execute `ralph inspect run`.
pub fn inspect_run_command(args: &InspectRunArgs) -> Result<()> {
    let workspace = match &args.root {
        Some(root) => std::fs::canonicalize(root).unwrap_or_else(|_| root.clone()),
        None => std::env::current_dir()?,
    };

    let evidence = gather_evidence(&workspace);
    let runs = aggregate_runs(&evidence, &workspace);

    match args.format {
        InspectProfilesFormat::Json => {
            let output = InspectRunOutput {
                schema_version: OBSERVATION_SCHEMA_VERSION,
                workspace,
                generated_at: Utc::now(),
                runs,
            };
            println!("{}", serde_json::to_string_pretty(&output)?);
        }
        InspectProfilesFormat::Human => print_human(&workspace, &runs),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[allow(clippy::too_many_arguments)]
    fn actions(
        kind: &str,
        alive: bool,
        state: &str,
        merge: Option<&str>,
        events: bool,
        ws: &Path,
    ) -> Vec<ActionDecl> {
        compute_actions("loop-1", kind, alive, state, merge, 0, 0, events, ws)
    }

    fn get<'a>(acts: &'a [ActionDecl], id: &str) -> &'a ActionDecl {
        acts.iter().find(|a| a.id == id).unwrap()
    }

    #[test]
    fn running_worktree_stop_and_emit_available() {
        let t = tmp();
        let a = actions("worktree", true, "running", None, true, t.path());
        assert!(get(&a, "stop").available);
        assert!(get(&a, "emit").available);
        assert!(!get(&a, "retry").available);
        assert!(!get(&a, "merge").available);
        assert_eq!(
            get(&a, "merge").reason.as_deref(),
            Some("loop still running — stop it first")
        );
    }

    #[test]
    fn exited_needs_review_retry_available() {
        let t = tmp();
        let a = actions(
            "worktree",
            false,
            "exited",
            Some("needs_review"),
            true,
            t.path(),
        );
        assert!(get(&a, "retry").available);
        // merge_button_state: no merging entry + no live lock → Active.
        assert!(get(&a, "merge").available);
    }

    #[test]
    fn exited_queued_retry_blocked_merge_available() {
        let t = tmp();
        let a = actions("worktree", false, "exited", Some("queued"), true, t.path());
        let retry = get(&a, "retry");
        assert!(!retry.available);
        assert!(retry.reason.as_deref().unwrap().contains("queued"));
        assert!(get(&a, "merge").available);
    }

    #[test]
    fn merge_blocked_states() {
        let t = tmp();
        for (ms, want) in [
            ("merging", "merge already in progress"),
            ("merged", "already merged"),
            ("discarded", "loop discarded"),
        ] {
            let a = actions("worktree", false, "exited", Some(ms), true, t.path());
            let m = get(&a, "merge");
            assert!(!m.available, "{ms}");
            assert_eq!(m.reason.as_deref(), Some(want), "{ms}");
        }
    }

    #[test]
    fn exited_primary_has_no_merge() {
        let t = tmp();
        let a = actions("primary", false, "exited", None, true, t.path());
        let m = get(&a, "merge");
        assert!(!m.available);
        assert_eq!(
            m.reason.as_deref(),
            Some("primary run has no merge semantics")
        );
    }

    #[test]
    fn exited_worktree_without_queue_entry_can_merge() {
        // `loops merge` enqueues orphan worktrees on demand.
        let t = tmp();
        let a = actions("worktree", false, "exited", None, true, t.path());
        assert!(get(&a, "merge").available);
    }

    #[test]
    fn exited_run_disables_stop_and_emit() {
        let t = tmp();
        let a = actions("worktree", false, "exited", None, true, t.path());
        let s = get(&a, "stop");
        assert!(!s.available);
        assert_eq!(s.reason.as_deref(), Some("run already exited"));
        let e = get(&a, "emit");
        assert!(!e.available);
    }

    #[test]
    fn unknown_state_stop_reason() {
        let t = tmp();
        let a = actions("unknown", false, "unknown", None, false, t.path());
        assert!(!get(&a, "stop").available);
        assert!(
            get(&a, "stop")
                .reason
                .as_deref()
                .unwrap()
                .contains("unknown")
        );
    }

    #[test]
    fn emit_requires_events_file() {
        let t = tmp();
        let a = actions("worktree", true, "running", None, false, t.path());
        let e = get(&a, "emit");
        assert!(!e.available);
        assert_eq!(
            e.reason.as_deref(),
            Some("no events file known for this run")
        );
    }

    #[test]
    fn emit_whitelist_is_fixed() {
        let t = tmp();
        let a = actions("worktree", true, "running", None, true, t.path());
        let schema = get(&a, "emit").input_schema.clone().unwrap();
        assert_eq!(
            schema["topics"],
            serde_json::json!(["loop.cancel", "task.resume"])
        );
    }

    #[test]
    fn revision_stable_and_sensitive() {
        let t = tmp();
        let a = actions("worktree", true, "running", Some("queued"), true, t.path());
        let r1 = get(&a, "stop").revision.clone();
        assert_eq!(
            r1,
            get(&a, "emit").revision,
            "one evidence snapshot → one revision"
        );
        let b = actions("worktree", true, "running", Some("merging"), true, t.path());
        assert_ne!(r1, get(&b, "stop").revision, "merge_state flips revision");
        let c = actions("worktree", false, "exited", Some("queued"), true, t.path());
        assert_ne!(r1, get(&c, "stop").revision, "state flips revision");
    }

    #[test]
    fn revision_hash_matches_receipt_fn() {
        // The receipt path and the capability path share one hash —
        // guard against drift.
        let a = compute_revision("l", "running", 3, Some("queued"), 2);
        let b = compute_revision("l", "running", 3, Some("queued"), 2);
        assert_eq!(a, b);
        assert_ne!(a, compute_revision("l", "running", 3, None, 2));
        assert_ne!(a, compute_revision("l", "running", 4, Some("queued"), 2));
    }
}
