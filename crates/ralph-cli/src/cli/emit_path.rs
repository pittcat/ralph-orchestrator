//! Allowlist-guarded resolution of the events file path used by `ralph emit`.
//!
//! This is the P6 emit-allowlist guard: the active loop's
//! `current-candidate-events` and `current-events` marker targets are the
//! only legitimate destinations, with `.ralph/events.jsonl` accepted only
//! when neither marker exists. Any explicit `RALPH_EVENTS_FILE` / `--file`
//! target must match an allowlist entry — no silent fallback to markers.
//! One narrow exception (plan 2026-07-25-003, U2): the dispatcher-signed
//! per-slot wave channel `…/.ralph/wave-<id>-<idx>.jsonl` is accepted when
//! the caller runs in isolated mode with a hat context, since the wave
//! dispatcher creates that file and injects it via `RALPH_EVENTS_FILE`
//! without listing it in any marker.
//!
//! Originally in `main.rs`; U4 lifts it into `cli/emit_path.rs` so the
//! call sites in `commands/emit.rs` (U4 step-2) can keep the imports tight.

use anyhow::{Result, bail};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub(crate) fn resolve_marker_target(workspace_root: &Path, marker_value: &str) -> PathBuf {
    let path = PathBuf::from(marker_value.trim());
    if path.is_absolute() {
        path
    } else {
        workspace_root.join(path)
    }
}

/// Marker file written by the runtime so isolated agents and
/// cross-cutting commands agree on the active hat's write channel.
pub(crate) const HAT_EVENTS_MARKER: &str = ".ralph/current-hat-events";

/// Resolve the real hat-channel file path under a workspace.
///
/// The marker `.ralph/current-hat-events` holds a path (relative to the
/// workspace root) of the JSONL file the runtime has marked as this
/// activation's write channel. This helper reads that marker and returns
/// the real channel path; previously each call site re-implemented the
/// read-and-trim, and `task_cli::emit_close_completion_warning` ended
/// up treating the marker itself as JSONL (the P0 #1 bug).
///
/// Return value:
///
/// - `Some((path, exists))` — marker was present and parsed to a
///   non-empty path; `exists` indicates whether the channel file itself
///   is currently on disk.
/// - `None` — marker missing or empty (the legacy fallback case used by
///   `ralph events --events-source auto`).
pub(crate) fn resolve_hat_channel_file(workspace_root: &Path) -> Option<(PathBuf, bool)> {
    let marker = workspace_root.join(HAT_EVENTS_MARKER);
    let raw = fs::read_to_string(&marker).ok()?;
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    let channel = resolve_marker_target(workspace_root, trimmed);
    let exists = fs::metadata(&channel).is_ok();
    Some((channel, exists))
}

/// P6: resolve the final events file path for `ralph emit` against an
/// allowlist. The allowlist is the set of paths the active loop has marked
/// as legitimate targets: the `current-candidate-events` marker target
/// (when present), the `current-events` marker target (when present), and
/// the default `events.jsonl` only when neither marker exists. Any other
/// path — from `RALPH_EVENTS_FILE`, `--file`, or a forged marker — is
/// rejected with a clear error.
///
/// U2 (2026-07-06-002 plan, R2/R4): the function gains two optional
/// parameters so the isolated-mode fail-closed guard can see the live
/// hat context (set by the loop runner via `RALPH_CURRENT_HAT`) and
/// whether the resolved config is `event_loop.execution_mode: isolated`.
///
/// - `current_hat` — when `Some(_)`, the candidate path is checked
///   against `current-hat-events` marker so a hat running inside a
///   subtree cwd cannot re-resolve to a `subdir/.ralph/events*.jsonl`
///   orphan file (error code `orphan_events_path`).
/// - `isolated_mode` — when `true` AND the workspace root has a
///   non-empty `current-hat-events` marker, the resolver refuses to
///   fall back to `workspace_root/.ralph/events.jsonl` (the legacy
///   default); the channel marker is the only legitimate fall-through.
///
/// U2 (plan 2026-07-25-003): when `isolated_mode` is `true` AND
/// `current_hat` is `Some(_)`, an explicit target whose shape is
/// `workspace_root/.ralph/wave-<id>-<idx>.jsonl` (the dispatcher-signed
/// per-slot wave channel, injected via `RALPH_EVENTS_FILE`) is accepted
/// even though no marker lists it. Acceptance additionally requires the
/// file's `<id>` / `<idx>` segments to match `wave_id` / `slot_index`
/// verbatim — the dispatcher is the only signer that knows both values,
/// so this binds the allowlist carve-out to a real wave-worker process
/// and blocks any isolated hat from forging a path to a sibling slot's
/// channel (adversarial-01 / goal-alignment-01). All other non-allowlisted
/// explicit targets are still rejected.
pub(crate) fn resolve_emit_path(
    workspace_root: &Path,
    cli_file: &Path,
    env_events_file: Option<&str>,
    current_hat: Option<&str>,
    isolated_mode: bool,
    wave_id: Option<&str>,
    slot_index: Option<u32>,
) -> Result<PathBuf> {
    fn normalize_path(p: &Path) -> PathBuf {
        let mut out = PathBuf::new();
        for comp in p.components() {
            match comp {
                Component::CurDir => {}
                Component::ParentDir => {
                    if !out.pop() {
                        out.push(comp);
                    }
                }
                other => out.push(other),
            }
        }
        out
    }

    /// U2 (R2): classify a candidate events path inside the workspace
    /// as an "orphan subtree" target. Returns `Some(reason)` when the
    /// path lives under a nested `subdir/.ralph/...` subtree (rather
    /// than the workspace root `.ralph/...` or `.ralph/agent/...` hat
    /// channel). Such paths are produced by a hat process running
    /// inside a subtree cwd (`cd sorts/`) and falling back to
    /// `cwd/.ralph/events.jsonl` implicit default.
    ///
    /// Acceptable forms:
    ///
    /// - `workspace_root/.ralph/events*.jsonl` (default / main events)
    /// - `workspace_root/.ralph/agent/events*.jsonl` (hat-channel)
    /// - `workspace_root/.ralph/current-events-marker-target` (when the
    ///   marker itself points back into the workspace root `.ralph/`).
    ///
    /// Reject:
    ///
    /// - `workspace_root/<subdir>/.ralph/events*.jsonl` where
    ///   `<subdir>` is non-empty and not equal to `.ralph` or
    ///   `.ralph/agent`. Example: `sorts/.ralph/events.jsonl`.
    fn classify_orphan_path(
        candidate: &Path,
        workspace_root: &Path,
        workspace_canon: &Path,
    ) -> Option<String> {
        // Strip the workspace root prefix (lexical). If the candidate is
        // not even inside the workspace_root, it cannot be a "subtree
        // orphan" of this workspace; that branch is already blocked by
        // the symlink / outside-workspace guards above.
        let relative = candidate.strip_prefix(workspace_root).ok().or_else(|| {
            workspace_canon
                .strip_prefix(workspace_root)
                .ok()
                .and_then(|_| candidate.strip_prefix(workspace_canon).ok())
        })?;
        // Walk the components: the first must be `.ralph` for the path
        // to be a workspace-rooted events file. Anything else indicates
        // the candidate nests under a real subdirectory, e.g.
        // `sorts/.ralph/events.jsonl`.
        let mut comps = relative.components();
        let first = comps.next()?;
        let first_os = first.as_os_str();
        if first_os != ".ralph" {
            return Some(format!(
                "candidate path nests under `{}`, not the workspace root `.ralph/`; \
                 this looks like a hat-PWD subtree default rather than a loop-managed \
                 events file",
                first_os.to_string_lossy()
            ));
        }
        None
    }

    /// U2 (plan 2026-07-25-003): recognize the dispatcher-signed per-slot
    /// wave channel `workspace_root/.ralph/wave-<id>-<idx>.jsonl`.
    ///
    /// The wave dispatcher creates this file (`wave/dispatcher.rs`) and
    /// injects it into wave workers via `RALPH_EVENTS_FILE`; it never
    /// appears in the `current-events` / `current-candidate-events` /
    /// `current-hat-events` markers, so without this shape check the P6
    /// allowlist rejects it and workers cannot emit to their own channel.
    ///
    /// Acceptance requires THREE signals to align — the path shape is
    /// necessary but no longer sufficient. This blocks any isolated hat
    /// from writing another slot's channel just by naming the file
    /// `wave-<other-id>-<other-idx>.jsonl`:
    ///
    /// 1. Path sits DIRECTLY under the workspace root `.ralph/<file>`
    ///    (not under a slot-worktree subtree, not outside the workspace).
    /// 2. File name is `wave-<id>-<idx>.jsonl` with non-empty `<id>`
    ///    and an all-ASCII-digit `<idx>`.
    /// 3. `<id>` matches the dispatcher's `RALPH_WAVE_ID` for this
    ///    worker AND `<idx>` matches `RALPH_WAVE_INDEX` for this slot.
    ///    A mismatch means the candidate belongs to a different slot
    ///    or a different wave; reject as cross-slot tampering.
    ///
    /// The call site additionally requires `isolated_mode == true` and
    /// `current_hat.is_some()`, binding acceptance to the wave-worker
    /// context (wave workers run in isolated execution with a hat id).
    /// This does NOT open arbitrary `.ralph/*.jsonl` writes.
    fn is_wave_channel_path(
        candidate: &Path,
        workspace_root: &Path,
        workspace_canon: &Path,
        expected_wave_id: Option<&str>,
        expected_slot_index: Option<u32>,
    ) -> bool {
        // Strip the workspace root prefix (lexical), trying both the raw
        // and canonical roots so macOS `/var` → `/private/var` symlink
        // forms match regardless of which form the env var carried.
        let relative = match candidate
            .strip_prefix(workspace_root)
            .or_else(|_| candidate.strip_prefix(workspace_canon))
        {
            Ok(rel) => rel,
            Err(_) => return false,
        };
        // Exactly two components: `.ralph/<file>`. Anything deeper nests
        // under a subtree (e.g. a slot worktree's `.ralph/`); anything
        // shallower is not an events file at all.
        let mut comps = relative.components();
        let first_is_ralph_dir = comps.next().is_some_and(|c| c.as_os_str() == ".ralph");
        let file_comp = comps.next();
        if !first_is_ralph_dir || file_comp.is_none() || comps.next().is_some() {
            return false;
        }
        let Some(file_name) = file_comp.and_then(|c| c.as_os_str().to_str()) else {
            return false;
        };
        // File-name pattern: wave-<id>-<idx>.jsonl.
        let Some(stem) = file_name
            .strip_prefix("wave-")
            .and_then(|s| s.strip_suffix(".jsonl"))
        else {
            return false;
        };
        // `<idx>` is the segment after the LAST dash: non-empty, all
        // ASCII digits. `<id>` is everything before it and must be
        // non-empty.
        let Some(last_dash) = stem.rfind('-') else {
            return false;
        };
        let id_part = &stem[..last_dash];
        let idx_part = &stem[last_dash + 1..];
        if id_part.is_empty()
            || idx_part.is_empty()
            || !idx_part.chars().all(|c| c.is_ascii_digit())
        {
            return false;
        }
        // Bind acceptance to the dispatcher-signed per-slot contract
        // (adversarial-01 / goal-alignment-01). The path shape alone is
        // not enough: the worker's `RALPH_WAVE_ID` / `RALPH_WAVE_INDEX`
        // must match the file's `<id>` / `<idx>` exactly. Mismatched or
        // missing values mean the candidate is some other slot's
        // channel (or someone forged the path) — reject.
        let Some(expected_wave_id) = expected_wave_id else {
            return false;
        };
        let Some(expected_slot_index) = expected_slot_index else {
            return false;
        };
        if id_part != expected_wave_id {
            return false;
        }
        match idx_part.parse::<u32>() {
            Ok(parsed_idx) => parsed_idx == expected_slot_index,
            Err(_) => false,
        }
    }

    // Two paths are equivalent when their lexical forms match (after
    // dropping `.` / resolving `..`) OR when both canonicalize to the same
    // real path. The canonicalize branch exists for macOS, where `/var` is
    // a symlink to `/private/var`: an env var like `RALPH_EVENTS_FILE`
    // set by the parent process can land here as `/var/...` while the
    // loop's `workspace_root` (resolved from `current_dir()`) is
    // canonicalized to `/private/var/...`. Both strings point at the same
    // file; rejecting them as "not in the allowlist" would break every
    // macOS caller. Canonicalize may fail when the target file does not
    // exist yet; in that case we fall back to lexical comparison only.
    fn paths_equivalent(a: &Path, b: &Path) -> bool {
        if normalize_path(a) == normalize_path(b) {
            return true;
        }
        // Canonicalize may fail when the target file does not exist yet
        // (e.g. the first `ralph emit` call before events.jsonl is
        // created). Fall back to canonicalizing each path's existing
        // prefix (parent dir) and stitching the file name back on, so
        // macOS /var → /private/var symlinks resolve to the same real
        // path regardless of which form the caller used.
        fn canon_with_existing_parent(p: &Path) -> std::io::Result<PathBuf> {
            let file_name = p.file_name().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name")
            })?;
            let parent = p.parent().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "no parent")
            })?;
            let canon_parent = parent.canonicalize()?;
            Ok(canon_parent.join(file_name))
        }
        match (canon_with_existing_parent(a), canon_with_existing_parent(b)) {
            (Ok(ca), Ok(cb)) => ca == cb,
            _ => false,
        }
    }

    let ralph_dir = workspace_root.join(".ralph");
    let candidate_marker = ralph_dir.join("current-candidate-events");
    let current_marker = ralph_dir.join("current-events");
    let current_hat_marker = ralph_dir.join("current-hat-events");
    let default_path = ralph_dir.join("events.jsonl");

    // Build the allowlist of legitimate targets.
    let mut allowed: Vec<PathBuf> = Vec::new();
    if let Ok(value) = fs::read_to_string(&candidate_marker) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            allowed.push(resolve_marker_target(workspace_root, trimmed));
        }
    }
    if let Ok(value) = fs::read_to_string(&current_marker) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            allowed.push(resolve_marker_target(workspace_root, trimmed));
        }
    }
    // Phase 2: per-hat write channel marker (isolated mode only). The
    // event loop never reads from this file; the runner merges it back
    // to the main events file after the backend exits.
    if let Ok(value) = fs::read_to_string(&current_hat_marker) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            allowed.push(resolve_marker_target(workspace_root, trimmed));
        }
    }
    if allowed.is_empty() {
        allowed.push(default_path.clone());
    }

    // Determine the candidate path. Explicit env / --file targets are
    // honoured only when they match an allowlist entry — we never
    // silently rewrite to a marker. Without an explicit target, fall
    // through to candidate marker → current marker → default.
    //
    // The clap default for `--file` is `.ralph/events.jsonl` (relative)
    // and tests sometimes pass the absolute form, so we treat both as
    // the "no explicit file" case.
    let cli_file_is_default = {
        let rel_default = Path::new(".ralph/events.jsonl");
        cli_file == default_path || cli_file == rel_default
    };
    let explicit = env_events_file
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            if cli_file.as_os_str().is_empty() || cli_file_is_default {
                None
            } else {
                Some(cli_file.to_path_buf())
            }
        });

    // Canonicalize the workspace root once for prefix comparison (used by
    // the wave-channel shape check below and the outside-workspace guard).
    let workspace_canon = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());

    let candidate = if let Some(explicit_target) = explicit {
        let normalized_explicit = normalize_path(&explicit_target);
        if allowed
            .iter()
            .any(|entry| paths_equivalent(entry, &normalized_explicit))
        {
            explicit_target
        } else if isolated_mode
            && current_hat.is_some()
            && is_wave_channel_path(
                &normalized_explicit,
                workspace_root,
                &workspace_canon,
                wave_id,
                slot_index,
            )
        {
            // U2 (plan 2026-07-25-003): accept the dispatcher-signed
            // per-slot wave channel. The dispatcher creates
            // `…/.ralph/wave-<id>-<idx>.jsonl` and injects it as
            // `RALPH_EVENTS_FILE`; no marker lists it, so it is not in
            // `allowed` yet. Register it now so the symlink / orphan
            // guards in the final allowlist loop below still apply to
            // the resolved candidate.
            allowed.push(normalized_explicit.clone());
            explicit_target
        } else {
            bail!(
                "refusing to emit event to {}: not in this loop's events allowlist. \
                 Allowed targets: {}",
                explicit_target.display(),
                allowed
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
    } else if let Ok(value) = fs::read_to_string(&candidate_marker) {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            // U2 (R4): isolated + hat marker → fall through to channel,
            // NOT the legacy `events.jsonl` default. The legacy default
            // path is reserved for non-isolated callers (manual debug,
            // bootstrap, or `ralph events --events-source main`).
            if isolated_mode && current_hat.is_some() {
                let marker_value = fs::read_to_string(&current_hat_marker)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                if let Some(marker_value) = marker_value {
                    resolve_marker_target(workspace_root, &marker_value)
                } else {
                    default_path.clone()
                }
            } else {
                default_path.clone()
            }
        } else {
            resolve_marker_target(workspace_root, trimmed)
        }
    } else if let Ok(value) = fs::read_to_string(&current_marker) {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            // U2 (R4): isolated + hat marker → channel.
            if isolated_mode && current_hat.is_some() {
                let marker_value = fs::read_to_string(&current_hat_marker)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty());
                if let Some(marker_value) = marker_value {
                    resolve_marker_target(workspace_root, &marker_value)
                } else {
                    default_path.clone()
                }
            } else {
                default_path.clone()
            }
        } else {
            resolve_marker_target(workspace_root, trimmed)
        }
    } else if isolated_mode
        && current_hat.is_some()
        && let Ok(value) = fs::read_to_string(&current_hat_marker)
    {
        // U2 (R4): isolated mode + hat marker present + no
        // candidate/current marker → resolve the channel rather than
        // `workspace_root/.ralph/events.jsonl` default. The hat's
        // write channel is the legitimate destination for `ralph emit`
        // under isolated execution.
        let trimmed = value.trim();
        if trimmed.is_empty() {
            default_path.clone()
        } else {
            resolve_marker_target(workspace_root, trimmed)
        }
    } else {
        default_path.clone()
    };

    // Normalize the candidate: drop `.` and resolve `..` lexically.
    let normalized = normalize_path(&candidate);

    // Verify the candidate is in the allowlist. We compare normalized forms
    // so that `.ralph/foo` and `foo/../.ralph/foo` are recognized as the
    // same path. We also block path traversal that escapes the workspace
    // root (a symlink target outside the workspace is still allowed only
    // if it matches an allowlist entry, which by construction points back
    // into the workspace).
    for entry in &allowed {
        if paths_equivalent(entry, &normalized) {
            // Refuse to honor symlinks that alias the allowlist target to
            // an outside file: if the canonical target differs from the
            // normalized target, check whether the real path is still
            // inside the workspace. OS-level symlinks (e.g. macOS
            // /tmp → /private/tmp) are harmless and must be allowed;
            // only reject paths that actually escape the workspace.
            if let Ok(canon) = normalized.canonicalize()
                && canon != normalized
                && !canon.starts_with(&workspace_canon)
                && !canon.starts_with(workspace_root)
            {
                bail!(
                    "Refusing to emit event through symlink: {} resolves to {} (outside this loop).",
                    normalized.display(),
                    canon.display()
                );
            }

            // U2 (R2): orphan guard — when a hat is running inside the
            // loop (current_hat is set), the resolved target must NOT
            // point at a `subdir/.ralph/events*.jsonl` orphan file. We
            // accept `subtree=.ralph/...` only when the subtree is the
            // workspace root itself (i.e. the `agent/` hat-channel
            // directory under `.ralph/`) OR the path is the workspace
            // root `.ralph/events*.jsonl` default. Anything like
            // `sorts/.ralph/events.jsonl` (a nested PWD subtree that
            // `cwd = sorts/` resolves relative to) is rejected.
            if current_hat.is_some()
                && let Some(orphan_reason) =
                    classify_orphan_path(&normalized, workspace_root, &workspace_canon)
            {
                bail!(
                    "orphan_events_path: refusing to emit event to {} — {}. \
                     Emits from a hat context must land in the workspace root \
                     .ralph/ tree (current-events / current-candidate-events / \
                     current-hat-events / events.jsonl) or .ralph/agent/ hat-channel, \
                     never in a nested cwd subtree.",
                    normalized.display(),
                    orphan_reason
                );
            }

            return Ok(normalized);
        }
    }

    // Block paths that escape the workspace root.
    if !normalized.starts_with(&workspace_canon) && !normalized.starts_with(workspace_root) {
        bail!(
            "Refusing to emit event to path outside workspace: {}. \
             Set --file to a path under {} or run inside a Ralph loop with a current-events marker.",
            normalized.display(),
            workspace_root.display()
        );
    }

    // Fall through: path is in the workspace but not in the allowlist.
    bail!(
        "Refusing to emit event to {}. The active loop has not marked this path; \
         allowed targets are: {}. Use one of those, or run ralph emit inside a loop \
         that publishes a current-events marker.",
        normalized.display(),
        allowed
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
}
