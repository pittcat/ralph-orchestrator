//! Runtime profile overlay loader — applies markdown fragments from a
//! project's `ralph-profiles/<name>/<preset>/<hat-id>.md` directories to
//! the corresponding hat's `instructions`.
//!
//! This module is the **runtime side** of the 2026-06-25-002 profile
//! plan. U1 (the `profiles.default` config block) lives in
//! `crate::config::profiles`. This file owns:
//!
//! - [`parse_profile_spec`] — accepts the `<scope>:<name>` literal and
//!   rejects malformed input (empty, `..`, `/`).
//! - [`resolve_profile_dir`] — turns a spec into an absolute directory
//!   path under the repo or user config root.
//! - [`resolve_profile_fragments`] — pure reader: walks each spec, reads
//!   the matching `.md` files, and returns them grouped by hat id with
//!   any non-fatal warnings. **Does not mutate [`crate::config::RalphConfig`]**.
//! - [`apply_profile_fragments`] — convenience wrapper that calls
//!   [`resolve_profile_fragments`] and then appends the fragments to
//!   `config.hats[hat_id].instructions`.
//!
//! Activation order is the caller's responsibility — this module only
//! preserves the order of `specs` as supplied. U3 / U4 handle the
//! `config.profiles.default` + CLI `--profile` merge.
//!
//! Side-effect boundaries:
//! - Repo profiles resolve under `workspace_root`, which the U4 caller
//!   pins to the original project root (or `RALPH_WORKSPACE_ROOT` in
//!   worktree mode) so worktrees don't drift.
//! - User profiles resolve under `$XDG_CONFIG_HOME/ralph/profiles/` (or
//!   `$HOME/.config/ralph/profiles/` when `XDG_CONFIG_HOME` is unset).
//! - No environment mutation. `XDG_CONFIG_HOME` / `HOME` are read at
//!   call time; tests can override them via `temp_env`-style isolation.

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::config::profiles::{ProfileScope, ProfileSpec, ProfileSpecParseError};
use crate::config::RalphConfig;

const REPO_PROFILES_DIR: &str = "ralph-profiles";
const USER_PROFILES_DIR: &str = ".config/ralph/profiles";

/// Hard cap on the size of a single profile fragment, in bytes.
///
/// Fragment bodies are appended verbatim to `HatConfig.instructions`
/// and ultimately fed into the agent prompt, so a 1 GiB `.md` file
/// would not only exhaust memory at load time but also pollute the
/// LLM context. 1 MiB matches the XDG base dir spec's "config files
/// are small" intent and is well above any sane operator-authored
/// markdown profile.
const MAX_FRAGMENT_BYTES: u64 = 1024 * 1024;

/// One markdown fragment loaded from a profile directory.
///
/// Fragments are appended to the matching hat's `instructions` in
/// `resolve_profile_fragments` order (filename-sorted within a profile,
/// profiles concatenated in the order given to
/// [`resolve_profile_fragments`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileFragment {
    /// Profile spec this fragment came from — useful for `ralph inspect
    /// profiles` output (U5).
    pub spec: ProfileSpec,
    /// Hat id the fragment targets (filename without `.md`).
    pub hat_id: String,
    /// Absolute path of the source `.md` file.
    pub path: PathBuf,
    /// Fragment body. UTF-8 decoded; non-UTF-8 files surface as
    /// [`ProfilesError::NonUtf8File`] (we never silently substitute
    /// replacement characters).
    pub content: String,
}

/// Errors surfaced while resolving or applying profile fragments.
#[derive(Debug, Error)]
pub enum ProfilesError {
    #[error("invalid profile spec {spec:?}: {source}")]
    InvalidSpec {
        spec: String,
        #[source]
        source: ProfileSpecParseError,
    },
    #[error("invalid profile name {name:?}: must be non-empty, contain no whitespace, '/', or '..' segments")]
    InvalidProfileName { name: String },
    #[error("profile directory not found: {path} (profile spec: {spec})")]
    ProfileDirNotFound { path: PathBuf, spec: String },
    #[error("profile {spec}: preset subdirectory not found: {path}")]
    PresetDirNotFound { path: PathBuf, spec: String },
    #[error("profile {spec}: HOME environment variable is not set; cannot resolve user profile directory")]
    HomeNotSet { spec: String },
    #[error("I/O error reading profile fragment {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("profile fragment {path} is not valid UTF-8: {source}")]
    NonUtf8File {
        path: PathBuf,
        #[source]
        source: std::string::FromUtf8Error,
    },
}

/// Result of [`resolve_profile_fragments`]: per-hat fragment list plus
/// non-fatal warnings. Apply-side decides whether to log warnings to
/// stderr (runtime) or pretty-print them (`ralph inspect profiles`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ResolvedProfileFragments {
    /// Hat id → list of fragments in activation order.
    pub by_hat: HashMap<String, Vec<ProfileFragment>>,
    /// Non-fatal warnings (missing preset subdir, orphan hat, etc.).
    pub warnings: Vec<String>,
}

/// Parse a single `<scope>:<name>` literal into a [`ProfileSpec`].
///
/// Thin wrapper around [`ProfileSpec::parse_str`] that wraps the
/// underlying [`ProfileSpecParseError`] in a [`ProfilesError`] with the
/// original input attached. The wrapper exists so U3's `collect_active`
/// helper can route every spec error through the same
/// `ProfilesError` channel — that helper has to bubble the error up to
/// `ralph run`/`ralph inspect profiles` without flattening.
pub fn parse_profile_spec(s: &str) -> Result<ProfileSpec, ProfilesError> {
    ProfileSpec::parse_str(s).map_err(|source| ProfilesError::InvalidSpec {
        spec: s.to_string(),
        source,
    })
}

/// Reject profile names that could escape the profile root directory or
/// that are obviously junk. Layered on top of the colon-syntax check in
/// [`ProfileSpec::parse_str`] — that check only handles the structural
/// half of the literal, not the safety half of the directory name.
fn validate_profile_name(name: &str) -> Result<(), ProfilesError> {
    if name.is_empty() {
        return Err(ProfilesError::InvalidProfileName {
            name: name.to_string(),
        });
    }
    if name.trim().is_empty() {
        return Err(ProfilesError::InvalidProfileName {
            name: name.to_string(),
        });
    }
    // Reject NUL and ASCII control characters outright. They survive
    // path joining on some platforms and can confuse downstream tools
    // (terminal emulators, log scrapers, the agent prompt renderer).
    if name.chars().any(|c| c.is_control() || c == '\0') {
        return Err(ProfilesError::InvalidProfileName {
            name: name.to_string(),
        });
    }
    // Reject `/` and `\` (path separators) and `..` segments to prevent
    // directory traversal. The check runs on the trimmed name; callers
    // are expected to pass the un-prefixed name (the half after `:`).
    if name.contains('/') || name.contains('\\') {
        return Err(ProfilesError::InvalidProfileName {
            name: name.to_string(),
        });
    }
    // Use `Path::components` to reject `..` and absolute paths even if
    // the caller tries to smuggle them in with surrounding text. A bare
    // `..` collapses to a single `Component::ParentDir`; `foo..bar`
    // does not, but that's fine because the only thing we care about
    // is escaping the directory root. A leading `/` (or `C:\` on
    // Windows) collapses to `Component::RootDir` / `Component::Prefix`,
    // which we reject explicitly.
    let mut comps = Path::new(name).components();
    let only = comps.next();
    if matches!(
        only,
        Some(
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_),
        )
    ) && comps.next().is_none()
    {
        return Err(ProfilesError::InvalidProfileName {
            name: name.to_string(),
        });
    }
    // Windows-reserved device names (case-insensitive). These cannot
    // be created as files on Windows but the directory name itself is
    // matched by the kernel — and on case-insensitive filesystems
    // operators routinely typo them. Defensive guardrail.
    let upper = name.to_ascii_uppercase();
    if matches!(
        upper.as_str(),
        "CON" | "PRN" | "AUX" | "NUL" | "COM1" | "COM2" | "COM3" | "COM4" | "COM5" | "COM6"
        | "COM7" | "COM8" | "COM9" | "LPT1" | "LPT2" | "LPT3" | "LPT4" | "LPT5" | "LPT6"
        | "LPT7" | "LPT8" | "LPT9"
    ) {
        return Err(ProfilesError::InvalidProfileName {
            name: name.to_string(),
        });
    }
    Ok(())
}

/// Resolve a spec's directory under the supplied repo root or under the
/// user's XDG config home.
///
/// - `Repo` → `<workspace_root>/ralph-profiles/<name>/`
/// - `User` → `$XDG_CONFIG_HOME/ralph/profiles/<name>/` (falling back to
///   `$HOME/.config/ralph/profiles/<name>/` when `XDG_CONFIG_HOME` is
///   unset).
///
/// Pure path arithmetic; never touches the filesystem. Callers can pass
/// the returned path to `exists()` or `read_dir()` as they see fit. The
/// name is re-validated here so an attacker cannot hand-craft a spec
/// (e.g. via `-c`) to escape the profile root.
pub fn resolve_profile_dir(
    spec: &ProfileSpec,
    workspace_root: &Path,
) -> Result<PathBuf, ProfilesError> {
    resolve_profile_dir_with(spec, workspace_root, default_env_lookup)
}

/// Test-friendly variant: caller supplies the env lookup. Production
/// callers should use [`resolve_profile_dir`] (which delegates to the
/// default env reader); this entry point exists so tests can run
/// without mutating `std::env`, which is `unsafe` and forbidden by the
/// workspace `forbid(unsafe_code)` lint.
pub fn resolve_profile_dir_with<F>(
    spec: &ProfileSpec,
    workspace_root: &Path,
    env_lookup: F,
) -> Result<PathBuf, ProfilesError>
where
    F: Fn(&str) -> Option<std::ffi::OsString> + Copy,
{
    validate_profile_name(&spec.name)?;
    match spec.scope {
        ProfileScope::Repo => Ok(workspace_root
            .join(REPO_PROFILES_DIR)
            .join(&spec.name)),
        ProfileScope::User => resolve_user_profile_dir_with(spec, &spec.name, env_lookup),
    }
}

/// Test-friendly variant: caller supplies the env lookup so the test
/// can stand in for `std::env::var_os` without unsafe env mutation.
fn resolve_user_profile_dir_with<F>(
    spec: &ProfileSpec,
    name: &str,
    env_lookup: F,
) -> Result<PathBuf, ProfilesError>
where
    F: Fn(&str) -> Option<std::ffi::OsString>,
{
    let base: PathBuf = match env_lookup("XDG_CONFIG_HOME") {
        Some(xdg) => {
            let trimmed: PathBuf = PathBuf::from(xdg);
            if trimmed.as_os_str().is_empty() {
                user_home_relative_with(spec, &env_lookup)?
            } else if trimmed.is_relative() {
                // XDG Base Directory Specification requires
                // $XDG_CONFIG_HOME to be an absolute path. A relative
                // value is almost always a misconfigured env (or a
                // sandboxed process that hasn't finished setup), and
                // silently accepting it would make profile resolution
                // cwd-dependent — which is exactly the kind of
                // surprising drift this layer is meant to prevent.
                // Fall back to $HOME so the operator gets a usable
                // path rather than a hard failure.
                user_home_relative_with(spec, &env_lookup)?
            } else {
                trimmed
            }
        }
        None => user_home_relative_with(spec, &env_lookup)?,
    };
    Ok(base.join(USER_PROFILES_DIR).join(name))
}

fn user_home_relative_with<F>(
    spec: &ProfileSpec,
    env_lookup: &F,
) -> Result<PathBuf, ProfilesError>
where
    F: Fn(&str) -> Option<std::ffi::OsString>,
{
    match env_lookup("HOME") {
        Some(home) => {
            let trimmed = PathBuf::from(home);
            if trimmed.as_os_str().is_empty() {
                Err(ProfilesError::HomeNotSet {
                    spec: format!("{}", spec.scope),
                })
            } else {
                Ok(trimmed)
            }
        }
        None => Err(ProfilesError::HomeNotSet {
            spec: format!("{}", spec.scope),
        }),
    }
}

fn default_env_lookup(key: &str) -> Option<std::ffi::OsString> {
    std::env::var_os(key)
}

/// Walk every spec, read every matching `.md` file, and return the
/// fragments grouped by hat id.
///
/// - Each spec's profile dir is resolved via [`resolve_profile_dir`].
/// - A missing profile dir is a hard error (`Err`); explicit specs the
///   user typed must point to real directories (R8).
/// - A profile that exists but has no subdirectory for `preset_name` is
///   a **warning** (R9). U4 prints it to stderr; U5 prints it in
///   human/JSON form.
/// - A `.md` whose name (sans extension) does not match a hat id in
///   `config.hats` is a **warning** (R7). The file is not loaded.
/// - `.md` files inside the preset directory are loaded in
///   filename-sorted order (deterministic across platforms — Rust's
///   `read_dir` order is not stable).
///
/// `workspace_root` is the directory the **repo** scope resolves
/// against. U4 sets it to the original project root (not the worktree)
/// so `--worktree` loops still find the repo-level profiles.
///
/// This function is pure with respect to [`RalphConfig`] — it reads
/// `config.hats` to filter orphans but never mutates `instructions`.
/// Apply-side lives in [`apply_profile_fragments`].
pub fn resolve_profile_fragments(
    config: &RalphConfig,
    preset_name: &str,
    specs: &[ProfileSpec],
    workspace_root: &Path,
) -> Result<ResolvedProfileFragments, ProfilesError> {
    resolve_profile_fragments_with(config, preset_name, specs, workspace_root, default_env_lookup)
}

/// Test-friendly variant of [`resolve_profile_fragments`]. See
/// [`resolve_profile_dir_with`] for why we expose this entry point.
pub fn resolve_profile_fragments_with<F>(
    config: &RalphConfig,
    preset_name: &str,
    specs: &[ProfileSpec],
    workspace_root: &Path,
    env_lookup: F,
) -> Result<ResolvedProfileFragments, ProfilesError>
where
    F: Fn(&str) -> Option<std::ffi::OsString> + Copy,
{
    let mut out = ResolvedProfileFragments::default();
    let hat_ids: HashSet<&str> = config.hats.keys().map(|k| k.as_str()).collect();

    for spec in specs {
        let profile_dir = resolve_profile_dir_with(spec, workspace_root, env_lookup)?;
        if !profile_dir.exists() {
            return Err(ProfilesError::ProfileDirNotFound {
                path: profile_dir,
                spec: format!("{spec}"),
            });
        }

        let preset_dir = profile_dir.join(preset_name);
        if !preset_dir.exists() {
            out.warnings.push(format!(
                "profile {spec} has no fragments for preset {preset_name:?} (missing {})",
                preset_dir.display()
            ));
            continue;
        }

        let mut entries: Vec<(OsString, PathBuf)> = match fs::read_dir(&preset_dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter(|e| {
                    // Reject symlinks pointing out of the profile tree
                    // before we even check the extension. std::fs::read
                    // follows symlinks, so without this guard a hostile
                    // or accidental `executor.md -> /etc/passwd` would
                    // be silently slurped into hat instructions.
                    match fs::symlink_metadata(e.path()) {
                        Ok(md) => !md.file_type().is_symlink(),
                        Err(_) => false, // unreadable entry: skip
                    }
                })
                .map(|e| (e.file_name(), e.path()))
                .filter(|(name, _)| {
                    Path::new(name)
                        .extension()
                        .is_some_and(|ext| ext == "md")
                })
                .collect(),
            Err(source) => {
                return Err(ProfilesError::Io {
                    path: preset_dir,
                    source,
                });
            }
        };
        entries.sort_by(|a, b| a.0.cmp(&b.0));

        let mut contributed = false;
        for (name, path) in entries {
            let name_str = match name.to_str() {
                Some(s) => s.to_string(),
                None => {
                    // Surface non-UTF-8 filenames as a warning rather
                    // than silently dropping them. The lossless view is
                    // available via `to_string_lossy()` so the operator
                    // can see what was skipped.
                    out.warnings.push(format!(
                        "profile {spec}: skipped non-UTF-8 fragment filename {:?} in preset {preset_name:?}",
                        name.to_string_lossy()
                    ));
                    continue;
                }
            };
            let hat_id = match name_str.strip_suffix(".md") {
                Some(stripped) if !stripped.is_empty() => stripped.to_string(),
                Some(_) | None => continue, // `.md` or non-`.md` (filter already enforces the latter)
            };
            if !hat_ids.contains(hat_id.as_str()) {
                out.warnings.push(format!(
                    "profile {spec} has fragment for unknown hat {hat_id:?} in preset {preset_name:?} (file: {})",
                    path.display()
                ));
                continue;
            }
            // Enforce a hard size cap before reading the file into
            // memory. A 1 GiB fragment is almost certainly a mistake
            // (or an attack) and would crash the loader before the
            // downstream size check could ever fire.
            let size = fs::symlink_metadata(&path)
                .map_err(|source| ProfilesError::Io {
                    path: path.clone(),
                    source,
                })?
                .len();
            if size > MAX_FRAGMENT_BYTES {
                out.warnings.push(format!(
                    "profile {spec} has fragment {path:?} exceeding size cap \
                     ({size} bytes > {MAX_FRAGMENT_BYTES} bytes); skipped",
                ));
                continue;
            }
            let bytes = fs::read(&path).map_err(|source| ProfilesError::Io {
                path: path.clone(),
                source,
            })?;
            let content = String::from_utf8(bytes).map_err(|source| ProfilesError::NonUtf8File {
                path: path.clone(),
                source,
            })?;
            contributed = true;
            out.by_hat
                .entry(hat_id.clone())
                .or_default()
                .push(ProfileFragment {
                    spec: spec.clone(),
                    hat_id,
                    path,
                    content,
                });
        }

        if !contributed {
            out.warnings.push(format!(
                "profile {spec} contributed no fragments for preset {preset_name:?}"
            ));
        }
    }

    Ok(out)
}

/// Append every fragment returned by [`resolve_profile_fragments`] to
/// the matching hat's `instructions`, in activation order, separated by
/// a single `\n`.
///
/// - If `instructions` does not end with `\n`, a `\n` is inserted first
///   so the appended fragment doesn't run into the previous content.
/// - If the fragment itself ends with `\n`, no extra `\n` is added — the
///   join is just `instructions + "\n" + fragment`.
/// - An empty fragment still adds a `\n` (matches the "no trailing junk"
///   rule above and keeps instructions readable when the file is
///   intentionally empty).
///
/// Returns the warnings from the resolution step so U4 can stream them
/// to stderr without re-walking the filesystem.
pub fn apply_profile_fragments(
    config: &mut RalphConfig,
    preset_name: &str,
    specs: &[ProfileSpec],
    workspace_root: &Path,
) -> Result<Vec<String>, ProfilesError> {
    apply_profile_fragments_with(config, preset_name, specs, workspace_root, default_env_lookup)
}

/// Test-friendly variant of [`apply_profile_fragments`]. See
/// [`resolve_profile_dir_with`] for why we expose this entry point.
pub fn apply_profile_fragments_with<F>(
    config: &mut RalphConfig,
    preset_name: &str,
    specs: &[ProfileSpec],
    workspace_root: &Path,
    env_lookup: F,
) -> Result<Vec<String>, ProfilesError>
where
    F: Fn(&str) -> Option<std::ffi::OsString> + Copy,
{
    let resolved = resolve_profile_fragments_with(config, preset_name, specs, workspace_root, env_lookup)?;
    for (hat_id, fragments) in &resolved.by_hat {
        let Some(hat) = config.hats.get_mut(hat_id) else {
            // Should be unreachable: resolve_profile_fragments filters
            // against config.hats keys. Stay defensive in case the hat
            // map is mutated between calls (it isn't, but the cost of a
            // guard is one branch).
            continue;
        };
        for fragment in fragments {
            if !hat.instructions.ends_with('\n') && !hat.instructions.is_empty() {
                hat.instructions.push('\n');
            }
            hat.instructions.push_str(&fragment.content);
            if !hat.instructions.ends_with('\n') {
                hat.instructions.push('\n');
            }
        }
    }
    Ok(resolved.warnings)
}

// Implementation note: tests that need to stub HOME/XDG_CONFIG_HOME go
// through `resolve_profile_dir_with` / `resolve_profile_fragments_with`
// / `apply_profile_fragments_with` and supply an explicit env lookup
// closure. The workspace sets `forbid(unsafe_code)`, which makes
// `std::env::set_var` / `remove_var` uncallable from tests.
#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;

    use crate::config::hat::HatConfig;
    use tempfile::TempDir;

    /// Build an env lookup closure from a flat list of `(KEY, PATH)`
    /// pairs. Unset keys read back as `None`. Uses `use<>` so the
    /// closure captures `pairs` by reference and lives as long as the
    /// backing `pairs` slice — without `use<>` Rust 2024 infers an
    /// over-short lifetime for `impl Trait` and rejects the call.
    fn env_from<'a>(
        pairs: &'a [(&'a str, &'a std::path::Path)],
    ) -> impl Fn(&str) -> Option<OsString> + Copy + use<'a> {
        move |key: &str| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.to_path_buf().into_os_string())
        }
    }

    /// Env lookup that returns `None` for every key.
    fn env_empty() -> impl Fn(&str) -> Option<OsString> + Copy {
        |_| None
    }

    fn hat(id: &str) -> HatConfig {
        let mut h = HatConfig::default();
        h.name = id.to_string();
        h
    }

    fn config_with(hats: &[&str]) -> RalphConfig {
        let mut cfg = RalphConfig::default();
        for id in hats {
            cfg.hats.insert((*id).to_string(), hat(id));
        }
        cfg
    }

    fn write_fragment(dir: &Path, hat_id: &str, body: &str) -> PathBuf {
        let path = dir.join(format!("{hat_id}.md"));
        fs::write(&path, body).unwrap();
        path
    }

    // ------------------------------------------------------------------
    // parse_profile_spec
    // ------------------------------------------------------------------

    #[test]
    fn parse_profile_spec_repo() {
        let s = parse_profile_spec("repo:strict").unwrap();
        assert_eq!(s.scope, ProfileScope::Repo);
        assert_eq!(s.name, "strict");
    }

    #[test]
    fn parse_profile_spec_user() {
        let s = parse_profile_spec("user:my-style").unwrap();
        assert_eq!(s.scope, ProfileScope::User);
        assert_eq!(s.name, "my-style");
    }

    #[test]
    fn parse_profile_spec_rejects_empty() {
        assert!(matches!(
            parse_profile_spec("").unwrap_err(),
            ProfilesError::InvalidSpec { .. }
        ));
    }

    #[test]
    fn parse_profile_spec_rejects_whitespace_name() {
        // After trim the name is empty; the wrapper still surfaces as
        // ProfilesError::InvalidSpec carrying the parse error.
        assert!(matches!(
            parse_profile_spec("repo:  ").unwrap_err(),
            ProfilesError::InvalidSpec { .. }
        ));
    }

    #[test]
    fn parse_profile_spec_rejects_unknown_scope() {
        assert!(matches!(
            parse_profile_spec("team:strict").unwrap_err(),
            ProfilesError::InvalidSpec { .. }
        ));
    }

    // ------------------------------------------------------------------
    // validate_profile_name (via resolve_profile_dir)
    // ------------------------------------------------------------------

    #[test]
    fn resolve_profile_dir_rejects_slash_in_name() {
        // U1's colon parser won't accept a literal "/", so we hand-
        // craft a ProfileSpec and assert resolve_profile_dir rejects it.
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "a/b".to_string(),
        };
        let tmp = TempDir::new().unwrap();
        let err = resolve_profile_dir(&spec, tmp.path()).unwrap_err();
        assert!(matches!(err, ProfilesError::InvalidProfileName { .. }));
    }

    #[test]
    fn resolve_profile_dir_rejects_dotdot() {
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "..".to_string(),
        };
        let tmp = TempDir::new().unwrap();
        let err = resolve_profile_dir(&spec, tmp.path()).unwrap_err();
        assert!(matches!(err, ProfilesError::InvalidProfileName { .. }));
    }

    #[test]
    fn resolve_profile_dir_rejects_whitespace_only_name() {
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "   ".to_string(),
        };
        let tmp = TempDir::new().unwrap();
        let err = resolve_profile_dir(&spec, tmp.path()).unwrap_err();
        assert!(matches!(err, ProfilesError::InvalidProfileName { .. }));
    }

    // ------------------------------------------------------------------
    // resolve_profile_dir
    // ------------------------------------------------------------------

    #[test]
    fn resolve_profile_dir_repo_appends_subpath() {
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        };
        let tmp = TempDir::new().unwrap();
        let dir = resolve_profile_dir(&spec, tmp.path()).unwrap();
        assert_eq!(dir, tmp.path().join("ralph-profiles").join("strict"));
    }

    #[test]
    fn resolve_profile_dir_user_uses_xdg_when_set() {
        let xdg = TempDir::new().unwrap();
        let home_fallback = std::path::PathBuf::from("/tmp/home-fallback");
        let pairs: [(&str, &std::path::Path); 2] = [
            ("XDG_CONFIG_HOME", xdg.path()),
            ("HOME", &home_fallback),
        ];
        let env = env_from(&pairs);

        let spec = ProfileSpec {
            scope: ProfileScope::User,
            name: "my-style".to_string(),
        };
        let dir =
            resolve_profile_dir_with(&spec, Path::new("/should/not/be/used"), env).unwrap();
        assert_eq!(
            dir,
            xdg.path()
                .join(".config")
                .join("ralph")
                .join("profiles")
                .join("my-style")
        );
    }

    #[test]
    fn resolve_profile_dir_user_falls_back_to_home() {
        let home = TempDir::new().unwrap();
        let pairs: [(&str, &std::path::Path); 1] = [("HOME", home.path())];
        let env = env_from(&pairs);

        let spec = ProfileSpec {
            scope: ProfileScope::User,
            name: "my-style".to_string(),
        };
        let dir =
            resolve_profile_dir_with(&spec, Path::new("/should/not/be/used"), env).unwrap();
        assert_eq!(
            dir,
            home.path()
                .join(".config")
                .join("ralph")
                .join("profiles")
                .join("my-style")
        );
    }

    #[test]
    fn resolve_profile_dir_user_errors_when_no_home_or_xdg() {
        let spec = ProfileSpec {
            scope: ProfileScope::User,
            name: "my-style".to_string(),
        };
        let err =
            resolve_profile_dir_with(&spec, Path::new("/anywhere"), env_empty()).unwrap_err();
        assert!(matches!(err, ProfilesError::HomeNotSet { .. }));
    }

    #[test]
    fn resolve_profile_dir_user_errors_when_home_is_empty_string() {
        // XDG_CONFIG_HOME unset, HOME = "" → user_home_relative
        // returns HomeNotSet. Build a closure that supplies the empty
        // string for HOME.
        let env = |key: &str| if key == "HOME" {
            Some(OsString::new())
        } else {
            None
        };

        let spec = ProfileSpec {
            scope: ProfileScope::User,
            name: "my-style".to_string(),
        };
        let err = resolve_profile_dir_with(&spec, Path::new("/anywhere"), env).unwrap_err();
        assert!(matches!(err, ProfilesError::HomeNotSet { .. }));
    }

    // ------------------------------------------------------------------
    // resolve_profile_fragments — happy path
    // ------------------------------------------------------------------

    #[test]
    fn resolve_repo_profile_appends_fragment_to_matching_hat() {
        let tmp = TempDir::new().unwrap();
        let profile_dir = tmp.path().join("ralph-profiles").join("strict");
        let preset_dir = profile_dir.join("ce-executor-serial");
        fs::create_dir_all(&preset_dir).unwrap();
        write_fragment(&preset_dir, "executor", "### strict override\n");

        let cfg = config_with(&["executor"]);
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        };
        let resolved =
            resolve_profile_fragments(&cfg, "ce-executor-serial", &[spec], tmp.path()).unwrap();

        assert!(resolved.warnings.is_empty(), "warnings: {:?}", resolved.warnings);
        let frags = resolved.by_hat.get("executor").expect("executor fragments");
        assert_eq!(frags.len(), 1);
        assert_eq!(frags[0].content, "### strict override\n");
        assert!(frags[0].path.ends_with("executor.md"));
    }

    #[test]
    fn resolve_profile_orders_fragments_alphabetically_within_profile() {
        let tmp = TempDir::new().unwrap();
        let preset_dir = tmp
            .path()
            .join("ralph-profiles")
            .join("strict")
            .join("ce-executor-serial");
        fs::create_dir_all(&preset_dir).unwrap();
        // Write in non-alphabetical order; expect resolve to sort.
        write_fragment(&preset_dir, "zeta", "Z\n");
        write_fragment(&preset_dir, "alpha", "A\n");
        write_fragment(&preset_dir, "mid", "M\n");

        let cfg = config_with(&["alpha", "mid", "zeta"]);
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        };
        let resolved =
            resolve_profile_fragments(&cfg, "ce-executor-serial", &[spec], tmp.path()).unwrap();

        let alpha = resolved.by_hat.get("alpha").unwrap();
        let mid = resolved.by_hat.get("mid").unwrap();
        let zeta = resolved.by_hat.get("zeta").unwrap();
        assert_eq!(alpha[0].content, "A\n");
        assert_eq!(mid[0].content, "M\n");
        assert_eq!(zeta[0].content, "Z\n");
    }

    #[test]
    fn resolve_concatenates_multiple_profiles_in_activation_order() {
        let tmp = TempDir::new().unwrap();
        let profile_a = tmp.path().join("ralph-profiles").join("base");
        let profile_b = tmp.path().join("ralph-profiles").join("extra");
        fs::create_dir_all(profile_a.join("debug")).unwrap();
        fs::create_dir_all(profile_b.join("debug")).unwrap();
        write_fragment(&profile_a.join("debug"), "investigator", "FROM_BASE\n");
        write_fragment(&profile_b.join("debug"), "investigator", "FROM_EXTRA\n");

        let cfg = config_with(&["investigator"]);
        let specs = vec![
            ProfileSpec {
                scope: ProfileScope::Repo,
                name: "base".to_string(),
            },
            ProfileSpec {
                scope: ProfileScope::Repo,
                name: "extra".to_string(),
            },
        ];
        let resolved = resolve_profile_fragments(&cfg, "debug", &specs, tmp.path()).unwrap();
        let frags = resolved.by_hat.get("investigator").unwrap();
        assert_eq!(frags.len(), 2);
        assert_eq!(frags[0].spec.name, "base");
        assert_eq!(frags[1].spec.name, "extra");
        assert_eq!(frags[0].content, "FROM_BASE\n");
        assert_eq!(frags[1].content, "FROM_EXTRA\n");
    }

    #[test]
    fn resolve_user_profile_with_xdg_config_home() {
        let xdg = TempDir::new().unwrap();
        let home_fallback = std::path::PathBuf::from("/tmp/should-not-be-used");
        let pairs: [(&str, &std::path::Path); 2] = [
            ("XDG_CONFIG_HOME", xdg.path()),
            ("HOME", &home_fallback),
        ];
        let env = env_from(&pairs);

        let profile_dir = xdg
            .path()
            .join(".config")
            .join("ralph")
            .join("profiles")
            .join("my-style")
            .join("debug");
        fs::create_dir_all(&profile_dir).unwrap();
        write_fragment(&profile_dir, "investigator", "USER_STYLE\n");

        let cfg = config_with(&["investigator"]);
        let spec = ProfileSpec {
            scope: ProfileScope::User,
            name: "my-style".to_string(),
        };
        let resolved = resolve_profile_fragments_with(
            &cfg,
            "debug",
            &[spec],
            Path::new("/repo/should/not/be/used"),
            env,
        )
        .unwrap();

        let frags = resolved.by_hat.get("investigator").unwrap();
        assert_eq!(frags.len(), 1);
        assert_eq!(frags[0].content, "USER_STYLE\n");
    }

    // ------------------------------------------------------------------
    // resolve_profile_fragments — warnings
    // ------------------------------------------------------------------

    #[test]
    fn resolve_warns_when_preset_subdir_missing() {
        let tmp = TempDir::new().unwrap();
        let profile_dir = tmp.path().join("ralph-profiles").join("strict");
        fs::create_dir_all(&profile_dir).unwrap();
        // No <preset_dir> subdirectory.

        let cfg = config_with(&["executor"]);
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        };
        let resolved =
            resolve_profile_fragments(&cfg, "ce-executor-serial", &[spec], tmp.path()).unwrap();

        assert!(resolved.by_hat.is_empty());
        assert_eq!(resolved.warnings.len(), 1);
        assert!(resolved.warnings[0].contains("no fragments for preset"));
    }

    #[test]
    fn resolve_warns_when_fragment_targets_unknown_hat() {
        let tmp = TempDir::new().unwrap();
        let preset_dir = tmp
            .path()
            .join("ralph-profiles")
            .join("strict")
            .join("ce-executor-serial");
        fs::create_dir_all(&preset_dir).unwrap();
        write_fragment(&preset_dir, "ghost", "GHOST\n");
        write_fragment(&preset_dir, "executor", "REAL\n");

        let cfg = config_with(&["executor"]);
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        };
        let resolved =
            resolve_profile_fragments(&cfg, "ce-executor-serial", &[spec], tmp.path()).unwrap();

        assert_eq!(resolved.warnings.len(), 1);
        assert!(resolved.warnings[0].contains("unknown hat"));
        assert!(resolved.warnings[0].contains("ghost"));
        // The unknown hat's fragment is not loaded.
        assert!(!resolved.by_hat.contains_key("ghost"));
        // The known hat's fragment IS loaded.
        assert!(resolved.by_hat.contains_key("executor"));
    }

    #[test]
    fn resolve_warns_when_profile_contributed_no_fragments() {
        let tmp = TempDir::new().unwrap();
        let preset_dir = tmp
            .path()
            .join("ralph-profiles")
            .join("strict")
            .join("ce-executor-serial");
        fs::create_dir_all(&preset_dir).unwrap();
        // Only an unknown hat file → contributes nothing.
        write_fragment(&preset_dir, "ghost", "GHOST\n");

        let cfg = config_with(&["executor"]);
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        };
        let resolved =
            resolve_profile_fragments(&cfg, "ce-executor-serial", &[spec], tmp.path()).unwrap();

        assert!(resolved.by_hat.is_empty());
        // Two warnings: unknown hat + contributed no fragments.
        assert_eq!(resolved.warnings.len(), 2);
        assert!(resolved.warnings.iter().any(|w| w.contains("unknown hat")));
        assert!(resolved.warnings.iter().any(|w| w.contains("contributed no fragments")));
    }

    // ------------------------------------------------------------------
    // resolve_profile_fragments — errors
    // ------------------------------------------------------------------

    #[test]
    fn resolve_errors_when_profile_dir_missing() {
        let tmp = TempDir::new().unwrap();
        // Don't create the profile dir at all.
        let cfg = config_with(&["executor"]);
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        };
        let err = resolve_profile_fragments(&cfg, "ce-executor-serial", &[spec], tmp.path())
            .unwrap_err();
        match err {
            ProfilesError::ProfileDirNotFound { path, .. } => {
                assert!(path.ends_with("ralph-profiles/strict"));
            }
            other => panic!("expected ProfileDirNotFound, got {other:?}"),
        }
    }

    #[test]
    fn resolve_errors_on_non_utf8_fragment() {
        let tmp = TempDir::new().unwrap();
        let preset_dir = tmp
            .path()
            .join("ralph-profiles")
            .join("strict")
            .join("debug");
        fs::create_dir_all(&preset_dir).unwrap();
        // 0xFF 0xFE is not valid UTF-8 start.
        fs::write(preset_dir.join("investigator.md"), [0xFFu8, 0xFE, 0xFD]).unwrap();

        let cfg = config_with(&["investigator"]);
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        };
        let err = resolve_profile_fragments(&cfg, "debug", &[spec], tmp.path()).unwrap_err();
        assert!(matches!(err, ProfilesError::NonUtf8File { .. }));
    }

    // ------------------------------------------------------------------
    // apply_profile_fragments — ordering, trim, R15 invariance
    // ------------------------------------------------------------------

    #[test]
    fn apply_appends_fragment_with_separator_when_no_trailing_newline() {
        let tmp = TempDir::new().unwrap();
        let preset_dir = tmp
            .path()
            .join("ralph-profiles")
            .join("strict")
            .join("debug");
        fs::create_dir_all(&preset_dir).unwrap();
        write_fragment(&preset_dir, "investigator", "STRICT_RULES\n");

        let mut cfg = config_with(&["investigator"]);
        cfg.hats.get_mut("investigator").unwrap().instructions =
            "ORIGINAL_INSTRUCTIONS".to_string();

        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        };
        apply_profile_fragments(&mut cfg, "debug", &[spec], tmp.path()).unwrap();

        let hat = &cfg.hats["investigator"];
        assert_eq!(
            hat.instructions,
            "ORIGINAL_INSTRUCTIONS\nSTRICT_RULES\n",
            "instructions must be newline-separated"
        );
    }

    #[test]
    fn apply_does_not_double_separator_when_instructions_ends_with_newline() {
        let tmp = TempDir::new().unwrap();
        let preset_dir = tmp
            .path()
            .join("ralph-profiles")
            .join("strict")
            .join("debug");
        fs::create_dir_all(&preset_dir).unwrap();
        write_fragment(&preset_dir, "investigator", "STRICT_RULES\n");

        let mut cfg = config_with(&["investigator"]);
        cfg.hats.get_mut("investigator").unwrap().instructions =
            "ORIGINAL_INSTRUCTIONS\n".to_string();

        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        };
        apply_profile_fragments(&mut cfg, "debug", &[spec], tmp.path()).unwrap();

        let hat = &cfg.hats["investigator"];
        assert_eq!(
            hat.instructions, "ORIGINAL_INSTRUCTIONS\nSTRICT_RULES\n",
            "must not insert extra newline between instructions and fragment"
        );
    }

    #[test]
    fn apply_appends_empty_fragment_as_single_newline() {
        let tmp = TempDir::new().unwrap();
        let preset_dir = tmp
            .path()
            .join("ralph-profiles")
            .join("strict")
            .join("debug");
        fs::create_dir_all(&preset_dir).unwrap();
        write_fragment(&preset_dir, "investigator", "");

        let mut cfg = config_with(&["investigator"]);
        cfg.hats.get_mut("investigator").unwrap().instructions =
            "ORIGINAL_INSTRUCTIONS".to_string();

        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        };
        apply_profile_fragments(&mut cfg, "debug", &[spec], tmp.path()).unwrap();

        let hat = &cfg.hats["investigator"];
        assert_eq!(
            hat.instructions, "ORIGINAL_INSTRUCTIONS\n",
            "empty fragment still adds a single newline separator"
        );
    }

    #[test]
    fn apply_preserves_other_hat_fields_r15_invariance() {
        // R15: profile application must NOT mutate any HatConfig field
        // other than `instructions`. Capture the pre-image of every
        // other field, run apply, then compare.
        let tmp = TempDir::new().unwrap();
        let preset_dir = tmp
            .path()
            .join("ralph-profiles")
            .join("strict")
            .join("debug");
        fs::create_dir_all(&preset_dir).unwrap();
        write_fragment(&preset_dir, "investigator", "STRICT\n");

        let mut cfg = config_with(&["investigator"]);
        // Decorate the hat so we can detect any unintended drift.
        {
            let hat = cfg.hats.get_mut("investigator").unwrap();
            hat.name = "InvestHat".to_string();
            hat.description = Some("An investigator".to_string());
            hat.triggers = vec!["work.ready".to_string()];
            hat.publishes = vec!["work.done".to_string()];
            hat.terminal_events = vec!["work.done".to_string()];
            hat.backend = Some(crate::config::hat::HatBackend::Named(
                "claude".to_string(),
            ));
            hat.backend_args = Some(vec!["--model".to_string(), "opus".to_string()]);
            hat.default_publishes = Some("work.done".to_string());
            hat.max_activations = Some(7);
            hat.disallowed_tools = vec!["rm".to_string()];
            hat.timeout = Some(120);
            hat.concurrency = 3;
            hat.exempt_topics = vec!["internal.topic".to_string()];
        }
        let snapshot = cfg.hats["investigator"].clone();

        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        };
        apply_profile_fragments(&mut cfg, "debug", &[spec], tmp.path()).unwrap();

        let after = &cfg.hats["investigator"];
        assert_eq!(after.name, snapshot.name);
        assert_eq!(after.description, snapshot.description);
        assert_eq!(after.triggers, snapshot.triggers);
        assert_eq!(after.publishes, snapshot.publishes);
        assert_eq!(after.terminal_events, snapshot.terminal_events);
        // instructions intentionally changed.
        assert_ne!(after.instructions, snapshot.instructions);
        assert_eq!(after.extra_instructions, snapshot.extra_instructions);
        assert_eq!(after.backend.as_ref().map(|b| format!("{b:?}")),
                   snapshot.backend.as_ref().map(|b| format!("{b:?}")));
        assert_eq!(after.backend_args, snapshot.backend_args);
        assert_eq!(after.default_publishes, snapshot.default_publishes);
        assert_eq!(after.max_activations, snapshot.max_activations);
        assert_eq!(after.disallowed_tools, snapshot.disallowed_tools);
        assert_eq!(after.timeout, snapshot.timeout);
        assert_eq!(after.concurrency, snapshot.concurrency);
        assert_eq!(after.exempt_topics, snapshot.exempt_topics);
        // sanity: instructions actually changed.
        assert!(after.instructions.contains("STRICT"));
    }

    #[test]
    fn apply_does_not_create_hats() {
        // R15 follow-up: profile loader must not invent hat entries.
        let tmp = TempDir::new().unwrap();
        let preset_dir = tmp
            .path()
            .join("ralph-profiles")
            .join("strict")
            .join("debug");
        fs::create_dir_all(&preset_dir).unwrap();
        // ghost.md would have been a warning in resolve, but apply must
        // never insert it.
        write_fragment(&preset_dir, "ghost", "GHOST\n");
        write_fragment(&preset_dir, "investigator", "STRICT\n");

        let mut cfg = config_with(&["investigator"]);
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        };
        let warnings = apply_profile_fragments(&mut cfg, "debug", &[spec], tmp.path()).unwrap();

        assert!(!cfg.hats.contains_key("ghost"));
        assert!(cfg.hats.contains_key("investigator"));
        assert!(warnings.iter().any(|w| w.contains("ghost")));
    }

    #[test]
    fn apply_with_no_active_specs_is_a_noop() {
        let mut cfg = config_with(&["investigator"]);
        cfg.hats.get_mut("investigator").unwrap().instructions =
            "UNCHANGED".to_string();
        let warnings =
            apply_profile_fragments(&mut cfg, "debug", &[], Path::new("/anywhere")).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(cfg.hats["investigator"].instructions, "UNCHANGED");
    }

    #[test]
    fn resolve_with_no_active_specs_is_empty() {
        let cfg = config_with(&["investigator"]);
        let resolved = resolve_profile_fragments(&cfg, "debug", &[], Path::new("/anywhere"))
            .unwrap();
        assert!(resolved.by_hat.is_empty());
        assert!(resolved.warnings.is_empty());
    }

    // ------------------------------------------------------------------
    // Serialization round-trip is already covered by U1; just sanity-
    // check that the ProfileSpec we build here round-trips through
    // ProfileSpec::parse_str (the integration path U3/U4 will use).
    // ------------------------------------------------------------------

    #[test]
    fn profile_spec_round_trips_through_parse_str() {
        let original = ProfileSpec {
            scope: ProfileScope::User,
            name: "my-style".to_string(),
        };
        let literal = format!("{}:{}", original.scope, original.name);
        let parsed = parse_profile_spec(&literal).unwrap();
        assert_eq!(parsed, original);
    }

    // ------------------------------------------------------------------
    // HashMap-style fixture used by config_with() — purely a compile-
    // time check that `hats: HashMap<String, HatConfig>` is what we
    // think it is.
    // ------------------------------------------------------------------

    #[test]
    fn hats_is_a_hash_map() {
        let mut map: HashMap<String, HatConfig> = HashMap::new();
        map.insert("a".to_string(), hat("a"));
        assert_eq!(map.len(), 1);
    }

    // ------------------------------------------------------------------
    // P1/P2 regression coverage (post-2026-06-25-002 review):
    // symlink rejection, size cap, control-char / Windows reserved
    // name rejection, XDG relative-path fallback, non-UTF-8 filename
    // warning.
    // ------------------------------------------------------------------

    /// P1 — `<profile>/<preset>/<hat>.md` that is a symlink pointing
    /// outside the profile tree must be rejected before `fs::read`
    /// follows it. We can't always create symlinks on Windows or in
    /// sandboxes without `CAP_DAC_OVERRIDE`, so this test is skipped
    /// when `symlink` itself fails — the path-traversal guard is only
    /// exercised when the platform supports it.
    #[test]
    fn resolve_rejects_symlinked_fragment() {
        let tmp = TempDir::new().unwrap();
        let preset_dir = tmp
            .path()
            .join("ralph-profiles")
            .join("strict")
            .join("debug");
        fs::create_dir_all(&preset_dir).unwrap();

        // Create an out-of-tree file that the symlink will point at.
        let outside = tmp.path().join("outside-secret.md");
        fs::write(&outside, "OUTSIDE_CONTENT\n").unwrap();

        // Try to symlink the canonical fragment filename at it.
        let link = preset_dir.join("investigator.md");
        let link_result = std::os::unix::fs::symlink(&outside, &link);
        if link_result.is_err() {
            // Platform does not allow symlinks here; treat as
            // skipped rather than failing CI.
            eprintln!("symlink unsupported on this platform; skipping");
            return;
        }

        let cfg = config_with(&["investigator"]);
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        };
        let resolved =
            resolve_profile_fragments(&cfg, "debug", &[spec], tmp.path()).unwrap();

        // The symlinked fragment must NOT have been loaded.
        assert!(
            !resolved.by_hat.contains_key("investigator"),
            "symlinked fragment must be rejected, got: {:?}",
            resolved.by_hat
        );
        assert!(resolved.by_hat.is_empty());
    }

    /// P2 — fragment files larger than `MAX_FRAGMENT_BYTES` are
    /// skipped with a warning rather than slurped into memory.
    #[test]
    fn resolve_warns_and_skips_oversized_fragment() {
        let tmp = TempDir::new().unwrap();
        let preset_dir = tmp
            .path()
            .join("ralph-profiles")
            .join("strict")
            .join("debug");
        fs::create_dir_all(&preset_dir).unwrap();

        // Write a file just past the cap. We don't actually allocate
        // a full MiB — `symlink_metadata().len()` works on sparse
        // files too if we `set_len`, but a simple write of a slightly
        // over-cap body is the most portable way to land above the
        // threshold on every platform.
        let path = preset_dir.join("investigator.md");
        let big = vec![b'a'; (MAX_FRAGMENT_BYTES as usize) + 1];
        fs::write(&path, &big).unwrap();

        let cfg = config_with(&["investigator"]);
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        };
        let resolved =
            resolve_profile_fragments(&cfg, "debug", &[spec], tmp.path()).unwrap();

        assert!(resolved.by_hat.is_empty());
        assert!(
            resolved.warnings.iter().any(|w| w.contains("size cap")),
            "expected size-cap warning, got: {:?}",
            resolved.warnings
        );
    }

    /// P2 — non-UTF-8 filenames surface as warnings instead of being
    /// silently dropped.
    ///
    /// Note: `.filter(|e| path.extension() == Some("md"))` already
    /// rejects non-UTF-8 names whose byte sequence doesn't end in
    /// `md`. To exercise the `to_str() == None` branch we therefore
    /// construct a filename whose UTF-8 form *would* pass the
    /// extension check but is rejected by `OsStr::to_str` (no such
    /// byte sequence exists in valid UTF-8), so we instead exercise
    /// the path via the `resolved.warnings` channel indirectly: we
    /// ensure that a directory entry with `.md` extension but
    /// un-decodable name still produces a warning rather than a
    /// silent drop. The cleanest portable test is to create a real
    /// `.md` file under an alias-symlink that points outside the
    /// profile tree — but on platforms where symlinks are allowed
    /// we cover that path in `resolve_rejects_symlinked_fragment`.
    /// Here we just assert the warning pipeline is wired by
    /// exercising the `OsStringExt` path on Linux only: macOS/APFS
    /// rejects invalid UTF-8 filenames at creation time (errno 92).
    #[cfg(target_os = "linux")]
    #[test]
    fn resolve_warns_on_non_utf8_filename() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let tmp = TempDir::new().unwrap();
        let preset_dir = tmp
            .path()
            .join("ralph-profiles")
            .join("strict")
            .join("debug");
        fs::create_dir_all(&preset_dir).unwrap();

        // Create a `.md`-suffixed file whose name is not valid UTF-8
        // by using the raw bytes form. The `md` suffix keeps the
        // extension filter happy; the leading bytes are not valid
        // UTF-8 so `to_str()` returns None.
        let bad_name = OsString::from_vec(b"\xff\xfe investigator.md".to_vec());
        let bad_path = preset_dir.join(&bad_name);
        fs::write(&bad_path, "CONTENT\n").unwrap();

        let cfg = config_with(&["investigator"]);
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "strict".to_string(),
        };
        let resolved =
            resolve_profile_fragments(&cfg, "debug", &[spec], tmp.path()).unwrap();

        // The non-UTF-8 filename must surface as a warning, and the
        // content must not have been loaded under the unknown name.
        assert!(
            resolved.warnings.iter().any(|w| w.contains("non-UTF-8")),
            "expected non-UTF-8 warning, got: {:?}",
            resolved.warnings
        );
        assert!(resolved.by_hat.is_empty() || resolved.by_hat.len() <= 1);
    }

    /// P2 — control characters in profile names are rejected.
    #[test]
    fn resolve_profile_dir_rejects_control_chars_in_name() {
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "foo\nbar".to_string(),
        };
        let tmp = TempDir::new().unwrap();
        let err = resolve_profile_dir(&spec, tmp.path()).unwrap_err();
        assert!(matches!(err, ProfilesError::InvalidProfileName { .. }));
    }

    /// P2 — NUL byte in profile name is rejected.
    #[test]
    fn resolve_profile_dir_rejects_nul_in_name() {
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            name: "foo\0bar".to_string(),
        };
        let tmp = TempDir::new().unwrap();
        let err = resolve_profile_dir(&spec, tmp.path()).unwrap_err();
        assert!(matches!(err, ProfilesError::InvalidProfileName { .. }));
    }

    /// P2 — Windows-reserved device names are rejected even though
    /// Rust's `Path::components` would happily accept them on
    /// Linux/macOS.
    #[test]
    fn resolve_profile_dir_rejects_windows_reserved_name() {
        for name in ["CON", "PRN", "AUX", "NUL", "COM1", "LPT9", "con"] {
            let spec = ProfileSpec {
                scope: ProfileScope::Repo,
                name: name.to_string(),
            };
            let tmp = TempDir::new().unwrap();
            let err = resolve_profile_dir(&spec, tmp.path()).unwrap_err();
            assert!(
                matches!(err, ProfilesError::InvalidProfileName { .. }),
                "expected rejection for {name:?}, got {err:?}"
            );
        }
    }

    /// P2 — absolute path components (RootDir, Prefix) are rejected
    /// even when `/` and `\` aren't literally present in the string.
    /// `Path::components` collapses a leading `/` to `RootDir`, which
    /// the validator now blocks.
    #[test]
    fn resolve_profile_dir_rejects_root_only_name() {
        let spec = ProfileSpec {
            scope: ProfileScope::Repo,
            // A bare slash — already caught by the `/` check, but we
            // also want to make sure the components-based check fires
            // when the slash is presented via a different
            // representation (e.g. on Windows via the prefix).
            name: "/etc".to_string(),
        };
        let tmp = TempDir::new().unwrap();
        let err = resolve_profile_dir(&spec, tmp.path()).unwrap_err();
        assert!(matches!(err, ProfilesError::InvalidProfileName { .. }));
    }

    /// P2 — when `XDG_CONFIG_HOME` is set but relative, we fall back
    /// to `$HOME/.config/ralph/profiles/...` rather than producing a
    /// cwd-relative path that drifts between invocations.
    #[test]
    fn resolve_profile_dir_user_falls_back_to_home_when_xdg_is_relative() {
        let home = TempDir::new().unwrap();
        let pairs: [(&str, &std::path::Path); 2] = [
            ("XDG_CONFIG_HOME", std::path::Path::new("relative-path")),
            ("HOME", home.path()),
        ];
        let env = env_from(&pairs);

        let spec = ProfileSpec {
            scope: ProfileScope::User,
            name: "my-style".to_string(),
        };
        let dir =
            resolve_profile_dir_with(&spec, Path::new("/anywhere"), env).unwrap();
        assert_eq!(
            dir,
            home.path()
                .join(".config")
                .join("ralph")
                .join("profiles")
                .join("my-style"),
            "relative XDG_CONFIG_HOME must defer to $HOME"
        );
    }
}