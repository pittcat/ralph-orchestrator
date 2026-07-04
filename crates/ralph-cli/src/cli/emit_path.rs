//! Allowlist-guarded resolution of the events file path used by `ralph emit`.
//!
//! This is the P6 emit-allowlist guard: the active loop's
//! `current-candidate-events` and `current-events` marker targets are the
//! only legitimate destinations, with `.ralph/events.jsonl` accepted only
//! when neither marker exists. Any explicit `RALPH_EVENTS_FILE` / `--file`
//! target must match an allowlist entry — no silent fallback to markers.
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
pub(crate) fn resolve_hat_channel_file(
    workspace_root: &Path,
) -> Option<(PathBuf, bool)> {
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
pub(crate) fn resolve_emit_path(
    workspace_root: &Path,
    cli_file: &Path,
    env_events_file: Option<&str>,
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

    let candidate = if let Some(explicit_target) = explicit {
        let normalized_explicit = normalize_path(&explicit_target);
        if allowed
            .iter()
            .any(|entry| paths_equivalent(entry, &normalized_explicit))
        {
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
        if !trimmed.is_empty() {
            resolve_marker_target(workspace_root, trimmed)
        } else {
            default_path.clone()
        }
    } else if let Ok(value) = fs::read_to_string(&current_marker) {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            resolve_marker_target(workspace_root, trimmed)
        } else {
            default_path.clone()
        }
    } else {
        default_path.clone()
    };

    // Normalize the candidate: drop `.` and resolve `..` lexically.
    let normalized = normalize_path(&candidate);
    // Canonicalize the workspace root once for prefix comparison.
    let workspace_canon = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());

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
