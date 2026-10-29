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
    RunObservation {
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
    }
}

fn observation_from_registry(
    entry: &LoopEntry,
    workspace: &Path,
    ev: &GatheredEvidence,
) -> RunObservation {
    let alive = entry.is_pid_alive();
    let events_file =
        read_trimmed(&Path::new(&entry.workspace).join(".ralph/current-events")).map(PathBuf::from);
    RunObservation {
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
    }
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
                runs.insert(
                    run_id.clone(),
                    RunObservation {
                        run_id,
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
                    },
                );
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
        runs.insert(
            loop_id.clone(),
            RunObservation {
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
            },
        );
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
