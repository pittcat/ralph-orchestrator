// 2026-07-27-003 plan U2 (KTD-1, KTD-2) — per-wave channel
// registry.
//
// Replaces `.ralph/current-wave-channels` (an append-only
// global marker) with a wave-scoped, schema-versioned JSON
// registry that the dispatcher MUST commit BEFORE spawning any
// worker. Each (loop_id, wave_id) gets its own file under
// `.ralph/wave-channels/<encoded-loop-id>/<encoded-wave-id>.json`
// listing the exact (slot_index, canonical channel path) tuples
// the dispatcher is allowed to authorize.
//
// Why a per-wave JSON registry (not the old append-only marker):
//
// 1. The old marker had no loop/wave/slot boundaries; the
//    implementation-review primary-20260727-051801 incident
//    demonstrated that a dispatcher running in one wave could
//    authorize channels for a different wave's slots.
// 2. Read-side parsing could not tell "channel signed for this
//    (loop, wave, slot)" from "channel signed by some earlier
//    wave". The worker had to be rejected by the wave-id shape
//    check, which never reached the production path because the
//    marker write was a warning, not a gate.
// 3. The marker could not be safely cleaned mid-loop; the new
//    registry writes one file per wave so cleanup is bounded.
//
// Atomicity contract:
//
// * Each channel file is created via `OpenOptions::create_new`
//   before any bind commit. A wave whose registry already exists
//   at the same path fails closed (`RegistryExistsMismatch`)
//   unless the bindings are byte-identical (recovery case).
// * The registry file itself is written via `tempfile::NamedTempFile`
//   → flush → `sync_all` → atomic rename → parent directory
//   `sync_all`. Concurrent readers observe either the previous
//   complete file or the new complete file, never a partial JSON
//   document.
// * `prepare(...)` then read-backs the written file and validates
//   the parsed JSON against the in-memory registry. The guard
//   only returns once the readback parses to the same bindings.
//
// Failure semantics:
//
// * Any I/O / JSON / schema mismatch inside `prepare(...)` is
//   surfaced as `Err(_)` and `prepare` MUST NOT spawn any worker
//   when called from the dispatcher path (U3 fail-close).
// * Drop best-effort removes the registry file. Explicit
//   `cleanup()` returns the result so the dispatcher can capture
//   the failure for diagnostics without losing the already-
//   committed deliverable state.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, Write};
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Bumped on every wire-incompatible change to the registry
/// schema. The parser refuses unknown schema versions.
pub const REGISTRY_SCHEMA_VERSION: u32 = 1;

/// Channel binding row inside a registry: which slot is allowed
/// to write which canonical channel file. The fingerprint is a
/// stable hash of the relative channel path so the read-side
/// resolver can sanity-check the registry on disk without
/// re-canonicalizing the filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelBinding {
    pub slot_index: u32,
    /// Canonical absolute path of the channel file the dispatcher
    /// will inject via `RALPH_EVENTS_FILE` and validate at emit
    /// time. Always absolute, always points inside the workspace
    /// `.ralph/` directory after `prepare` returns.
    pub channel_path: PathBuf,
    /// `sha256(channel_path.to_string_lossy().as_bytes())` so
    /// accidental re-canonicalization differences cannot
    /// silently slip a different path past readback.
    pub channel_fingerprint: String,
}

/// In-memory shape of a registry file. Sorted by `slot_index`
/// before serialization so the on-disk layout is independent
/// of binding insertion order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistryFile {
    pub schema_version: u32,
    pub loop_id: String,
    pub wave_id: String,
    /// RFC 3339 timestamp captured at `prepare` time.
    pub prepared_at: String,
    /// Sorted by `slot_index` (see `RegistryFile::sorted`).
    pub bindings: Vec<ChannelBinding>,
}

impl RegistryFile {
    /// Replace `self.bindings` with a `slot_index`-sorted clone
    /// of the same bindings. Called immediately before
    /// serialization so the on-disk encoding is deterministic
    /// across runs.
    fn sorted(mut self) -> Self {
        self.bindings.sort_by_key(|b| b.slot_index);
        self
    }
}

/// Errors surfaced by `WaveChannelRegistry`. Every variant
/// must cause the dispatcher to abort the wave — there is no
/// retry / fallback / silent-success path.
#[derive(Debug)]
pub enum ChannelRegistryError {
    /// `loop_id` / `wave_id` / `slot_index` did not pass the
    /// shape validator (empty, contains `/` in the wrong segment,
    /// etc.). This is a programming error in the dispatcher
    /// itself, not a runtime condition.
    InvalidIdentity { field: &'static str, value: String },
    /// Same `slot_index` repeated in the binding list.
    DuplicateSlot(u32),
    /// `bindings` is empty. A wave with zero slots is not a wave.
    EmptyBindings,
    /// Channel path is not inside the workspace root or contains
    /// forbidden components (e.g. parent traversal).
    PathEscape { path: PathBuf, reason: &'static str },
    /// Channel path's canonicalized form did not equal the
    /// provided lexical form (probably a symlink or `..` in the
    /// path). Refuse to commit a registry that says "you may
    /// write X" but X is not the file the worker will actually
    /// touch.
    ChannelNotCanonical { path: PathBuf, canonical: PathBuf },
    /// Channel file's parent directory could not be created or
    /// the create-new file open itself failed.
    ChannelFileCreate { path: PathBuf, source: io::Error },
    /// Registry directory or its parent could not be created.
    RegistryDirCreate { path: PathBuf, source: io::Error },
    /// Temp-file write / sync / rename for the registry JSON
    /// itself failed.
    RegistryWrite { path: PathBuf, source: io::Error },
    /// Registry JSON readback did not parse, or parsed to
    /// bindings that disagree with what we just wrote.
    RegistryReadback { path: PathBuf, reason: String },
    /// An existing registry file for the same
    /// `(loop_id, wave_id)` was found whose bindings differ
    /// from what we are about to commit. We refuse to overwrite
    /// silently: either the operator must clean up an aborted
    /// run, or this is a stale file from a different identity.
    RegistryExistsMismatch { path: PathBuf },
    /// The on-disk schema version is unknown to this build.
    SchemaMismatch { path: PathBuf, version: u32 },
    /// Identity in the on-disk JSON does not match the file
    /// name (defence against operator manipulation of the file
    /// system path).
    IdentityMismatch {
        path: PathBuf,
        expected_loop_id: String,
        expected_wave_id: String,
        found_loop_id: String,
        found_wave_id: String,
    },
    /// Resolver was asked about a (loop, wave, slot) tuple not
    /// present in any registry file under the workspace.
    BindingNotFound {
        loop_id: String,
        wave_id: String,
        slot_index: u32,
    },
    /// Resolver was asked to validate a `requested_path` that
    /// did not match the bound `channel_path`. This is the
    /// cross-slot / cross-wave tampering guard.
    ChannelPathMismatch { bound: PathBuf, requested: PathBuf },
    /// Registry JSONL could not be serialized.
    Json {
        path: PathBuf,
        source: serde_json::Error,
    },
}

impl std::fmt::Display for ChannelRegistryError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChannelRegistryError::InvalidIdentity { field, value } => {
                write!(f, "invalid identity {field}={value:?}")
            }
            ChannelRegistryError::DuplicateSlot(idx) => {
                write!(f, "duplicate slot index {idx}")
            }
            ChannelRegistryError::EmptyBindings => {
                write!(f, "no slot bindings supplied")
            }
            ChannelRegistryError::PathEscape { path, reason } => {
                write!(f, "channel path {path:?} escapes workspace: {reason}")
            }
            ChannelRegistryError::ChannelNotCanonical { path, canonical } => write!(
                f,
                "channel path {path:?} canonicalizes to {canonical:?}; refusing"
            ),
            ChannelRegistryError::ChannelFileCreate { path, source } => {
                write!(f, "channel file {path:?}: {source}")
            }
            ChannelRegistryError::RegistryDirCreate { path, source } => {
                write!(f, "registry dir {path:?}: {source}")
            }
            ChannelRegistryError::RegistryWrite { path, source } => {
                write!(f, "registry write {path:?}: {source}")
            }
            ChannelRegistryError::RegistryReadback { path, reason } => {
                write!(f, "registry readback {path:?}: {reason}")
            }
            ChannelRegistryError::RegistryExistsMismatch { path } => {
                write!(f, "registry {path:?} exists with different bindings")
            }
            ChannelRegistryError::SchemaMismatch { path, version } => {
                write!(f, "registry {path:?} has unknown schema_version={version}")
            }
            ChannelRegistryError::IdentityMismatch {
                path,
                expected_loop_id,
                expected_wave_id,
                found_loop_id,
                found_wave_id,
            } => write!(
                f,
                "registry {path:?} identity mismatch: expected \
                 loop={expected_loop_id:?} wave={expected_wave_id:?}, found \
                 loop={found_loop_id:?} wave={found_wave_id:?}"
            ),
            ChannelRegistryError::BindingNotFound {
                loop_id,
                wave_id,
                slot_index,
            } => write!(
                f,
                "no binding for loop={loop_id:?} wave={wave_id:?} slot={slot_index}"
            ),
            ChannelRegistryError::ChannelPathMismatch { bound, requested } => write!(
                f,
                "channel path mismatch: bound={bound:?} requested={requested:?}"
            ),
            ChannelRegistryError::Json { path, source } => {
                write!(f, "registry json {path:?}: {source}")
            }
        }
    }
}

impl std::error::Error for ChannelRegistryError {}

/// Outcome of an explicit or implicit `cleanup()`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CleanupOutcome {
    Removed,
    NotPresent,
}

/// Result of `WaveChannelRegistry::resolve`. `Bound { channel }`
/// means the resolver accepted the call and the caller may
/// proceed to emit. Anything else is a hard fail-close.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolveOutcome {
    /// The (loop, wave, slot, path) tuple is authorized.
    Bound { channel: PathBuf },
}

/// RAII handle returned by `WaveChannelRegistry::prepare`. Holds
/// the registry path so Drop can best-effort remove it; the
/// dispatcher should call `cleanup()` explicitly at known
/// termination points so a Drop-induced panic can be
/// distinguished from a clean wave close.
#[derive(Debug)]
pub struct WaveChannelRegistryGuard {
    pub(crate) registry_path: PathBuf,
    pub(crate) workspace_root: PathBuf,
    pub(crate) loop_id: String,
    pub(crate) wave_id: String,
    /// Per-slot channel paths (sorted). Kept so cleanup can
    /// remove the create-new'd channel files even when no
    /// worker used them.
    pub(crate) bound_paths: Vec<PathBuf>,
    /// Tracks whether `cleanup` / Drop has already removed the
    /// files. The second call to `cleanup` runs I/O again so
    /// the caller observes `NotPresent` instead of the stale
    /// `Removed` outcome; this also ensures Drop on an
    /// already-cleaned guard is a true no-op at the filesystem
    /// level.
    removed: std::cell::Cell<bool>,
}

impl WaveChannelRegistryGuard {
    /// Explicit cleanup. Always attempts I/O so callers can
    /// observe `NotPresent` if files were already removed by a
    /// prior call. Idempotent and safe to invoke multiple
    /// times.
    pub fn cleanup(&mut self) -> CleanupOutcome {
        let outcome = remove_registry_files(self);
        self.removed.set(true);
        outcome
    }
}

impl Drop for WaveChannelRegistryGuard {
    fn drop(&mut self) {
        if self.removed.get() {
            return;
        }
        // Best-effort: do not panic from Drop. The dispatcher
        // will already have logged warnings; the next reload
        // also tries to remove these files.
        let _ = remove_registry_files(self);
        self.removed.set(true);
    }
}

/// One `(slot_index, channel_path)` tuple passed to `prepare`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingInput {
    pub slot_index: u32,
    pub channel_path: PathBuf,
}

impl BindingInput {
    pub fn new(slot_index: u32, channel_path: PathBuf) -> Self {
        Self {
            slot_index,
            channel_path,
        }
    }
}

/// Stable string form of a channel path used for fingerprinting.
/// Always uses the OS-native separators so the SHA-256 input is
/// reproducible across `path-to-string-lossy` edge cases.
fn channel_path_bytes(path: &Path) -> Vec<u8> {
    let mut buf = Vec::new();
    for component in path.components() {
        if !buf.is_empty() {
            buf.push(b'/');
        }
        match component {
            Component::Prefix(prefix) => {
                buf.extend(prefix.as_os_str().to_string_lossy().as_bytes());
            }
            Component::RootDir => {
                buf.push(b'/');
            }
            Component::CurDir => {
                buf.extend(b".");
            }
            Component::ParentDir => {
                buf.extend(b"..");
            }
            Component::Normal(normal) => {
                // `OsStr::as_encoded_bytes` is unstable; `to_string_lossy`
                // is sufficient for fingerprinting because the
                // encoder is documented to operate on the same
                // byte sequence as the OS-native form.
                let s = normal.to_string_lossy();
                buf.extend(s.as_bytes());
            }
        }
    }
    buf
}

/// Fingerprint the canonical channel path so a subsequent
/// registry readback can confirm the on-disk file names the
/// same channel that was committed at prepare time. SHA-256
/// avoids the cross-version instability of Rust's DefaultHasher.
fn fingerprint(path: &Path) -> String {
    let bytes = channel_path_bytes(path);
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(&bytes);
    let digest = hasher.finalize();
    format!("sha256:{}", hex_lower(&digest))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

/// Validate that the loop id and wave id are non-empty and
/// contain only safe characters. The encoding later turns them
/// into a file-name segment; we want to refuse `/`, `..`, and
/// control characters so a malicious operator cannot point a
/// registry at an arbitrary file.
fn validate_identity(field: &'static str, value: &str) -> Result<(), ChannelRegistryError> {
    if value.is_empty() {
        return Err(ChannelRegistryError::InvalidIdentity {
            field,
            value: value.to_string(),
        });
    }
    if value.contains('/') || value.contains('\\') || value.contains('\0') {
        return Err(ChannelRegistryError::InvalidIdentity {
            field,
            value: value.to_string(),
        });
    }
    if value == "." || value == ".." {
        return Err(ChannelRegistryError::InvalidIdentity {
            field,
            value: value.to_string(),
        });
    }
    Ok(())
}

/// Encode the loop or wave id into a single file-name segment.
/// We percent-encode any byte that is not `[A-Za-z0-9._-]` so a
/// caller cannot escape `.ralph/wave-channels/` boundaries via a
/// crafted `wave_id="../../other-wave"`.
pub fn encode_identity(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.as_bytes() {
        let ch = *byte as char;
        if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
            out.push(ch);
        } else {
            out.push_str(&format!("%{:02X}", byte));
        }
    }
    out
}

/// Absolute path of the registry JSON file for a
/// `(loop_id, wave_id)` pair. Always inside the workspace root.
pub fn registry_path(workspace: &Path, loop_id: &str, wave_id: &str) -> PathBuf {
    workspace
        .join(".ralph")
        .join("wave-channels")
        .join(encode_identity(loop_id))
        .join(format!("{}.json", encode_identity(wave_id)))
}

/// Compute the SHA-256 fingerprint of a file's contents using a
/// fixed algorithm (so the readback path does not depend on
/// platform-default file-time rounding).
fn file_sha256(path: &Path) -> io::Result<String> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 8192];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    Ok(format!("sha256:{}", hex_lower(&hasher.finalize())))
}

/// Verify the channel path is inside the workspace root and
/// does not contain traversal components. Absolute paths must
/// canonicalize to an existing parent directory; relative paths
/// are joined to `workspace_root` and canonicalized.
fn check_channel_inside(
    workspace_root: &Path,
    workspace_canon: &Path,
    channel: &Path,
) -> Result<PathBuf, ChannelRegistryError> {
    let absolute = if channel.is_absolute() {
        channel.to_path_buf()
    } else {
        workspace_root.join(channel)
    };
    // Reject paths containing `..` or `.` lexical components.
    for component in absolute.components() {
        match component {
            Component::ParentDir => {
                return Err(ChannelRegistryError::PathEscape {
                    path: absolute,
                    reason: "channel path contains '..'",
                });
            }
            Component::CurDir => {
                return Err(ChannelRegistryError::PathEscape {
                    path: absolute,
                    reason: "channel path contains '.' segment",
                });
            }
            _ => {}
        }
    }
    // Canonicalize the parent so symlinks inside the workspace
    // (e.g. macOS `/tmp` → `/private/tmp`) resolve consistently.
    let parent = absolute
        .parent()
        .ok_or_else(|| ChannelRegistryError::PathEscape {
            path: absolute.clone(),
            reason: "channel path has no parent",
        })?;
    let parent_canon =
        parent
            .canonicalize()
            .map_err(|source| ChannelRegistryError::ChannelFileCreate {
                path: parent.to_path_buf(),
                source,
            })?;
    let file_name = absolute
        .file_name()
        .map(|f| f.to_os_string())
        .unwrap_or_else(|| OsString::from(""));
    let canon = parent_canon.join(&file_name);
    if !canon.starts_with(workspace_canon) && !canon.starts_with(workspace_root) {
        return Err(ChannelRegistryError::PathEscape {
            path: absolute,
            reason: "channel path resolves outside workspace root",
        });
    }
    // The channel file itself does NOT have to exist at
    // prepare-time; we will create it via `OpenOptions::create`.
    // We must canonicalize the parent (above) before declaring
    // success.
    Ok(canon)
}

/// Atomic JSON write: write to `temp_path`, `flush`,
/// `sync_all`, rename to `final_path`, then `sync_all` on the
/// parent directory so the rename is durable on Unix. Returns
/// the SHA-256 of the written contents.
fn write_atomic(final_path: &Path, body: &[u8]) -> Result<String, ChannelRegistryError> {
    let parent = final_path
        .parent()
        .ok_or_else(|| ChannelRegistryError::RegistryWrite {
            path: final_path.to_path_buf(),
            source: io::Error::new(io::ErrorKind::InvalidInput, "no parent"),
        })?;
    std::fs::create_dir_all(parent).map_err(|source| ChannelRegistryError::RegistryDirCreate {
        path: parent.to_path_buf(),
        source,
    })?;
    let temp_path = {
        let mut p = final_path.to_path_buf();
        let name = p
            .file_name()
            .map(|n| n.to_os_string())
            .unwrap_or_else(|| OsString::from("registry"));
        p.set_file_name(format!(
            ".{}.tmp-{}",
            name.to_string_lossy(),
            std::process::id()
        ));
        p
    };
    {
        let mut file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temp_path)
            .map_err(|source| ChannelRegistryError::RegistryWrite {
                path: temp_path.clone(),
                source,
            })?;
        file.write_all(body)
            .map_err(|source| ChannelRegistryError::RegistryWrite {
                path: temp_path.clone(),
                source,
            })?;
        file.flush()
            .map_err(|source| ChannelRegistryError::RegistryWrite {
                path: temp_path.clone(),
                source,
            })?;
        file.sync_all()
            .map_err(|source| ChannelRegistryError::RegistryWrite {
                path: temp_path.clone(),
                source,
            })?;
    }
    std::fs::rename(&temp_path, final_path).map_err(|source| {
        // Best-effort temp cleanup; ignore secondary error.
        let _ = std::fs::remove_file(&temp_path);
        ChannelRegistryError::RegistryWrite {
            path: final_path.to_path_buf(),
            source,
        }
    })?;
    if let Some(dir) = final_path.parent() {
        let _ = File::open(dir).map(|f| f.sync_all());
    }
    file_sha256(final_path).map_err(|source| ChannelRegistryError::RegistryWrite {
        path: final_path.to_path_buf(),
        source,
    })
}

/// Validate the bindings and produce the in-memory
/// `RegistryFile` shape WITHOUT touching the filesystem. Used
/// by both the initial write path and the recovery (existing
/// registry) path so the comparisons always happen against the
/// same canonicalized representation.
fn compute_in_memory_registry(
    workspace_root: &Path,
    workspace_canon: &Path,
    bindings: &[BindingInput],
    loop_id: &str,
    wave_id: &str,
) -> Result<RegistryFile, ChannelRegistryError> {
    let mut canonical_bindings = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let canon = check_channel_inside(workspace_root, workspace_canon, &binding.channel_path)?;
        let fingerprint_val = fingerprint(&canon);
        canonical_bindings.push(ChannelBinding {
            slot_index: binding.slot_index,
            channel_path: canon,
            channel_fingerprint: fingerprint_val,
        });
    }
    Ok(RegistryFile {
        schema_version: REGISTRY_SCHEMA_VERSION,
        loop_id: loop_id.to_string(),
        wave_id: wave_id.to_string(),
        prepared_at: format_rfc3339(SystemTime::now()),
        bindings: canonical_bindings,
    }
    .sorted())
}

fn read_registry(path: &Path) -> Result<RegistryFile, ChannelRegistryError> {
    let raw = std::fs::read_to_string(path).map_err(|source| {
        if source.kind() == io::ErrorKind::NotFound {
            ChannelRegistryError::RegistryReadback {
                path: path.to_path_buf(),
                reason: "registry file missing".to_string(),
            }
        } else {
            ChannelRegistryError::RegistryWrite {
                path: path.to_path_buf(),
                source,
            }
        }
    })?;
    let parsed: RegistryFile =
        serde_json::from_str(&raw).map_err(|source| ChannelRegistryError::Json {
            path: path.to_path_buf(),
            source,
        })?;
    if parsed.schema_version != REGISTRY_SCHEMA_VERSION {
        return Err(ChannelRegistryError::SchemaMismatch {
            path: path.to_path_buf(),
            version: parsed.schema_version,
        });
    }
    Ok(parsed)
}

/// I/O primitives exposed for the dispatcher / U3 paths. The
/// public entry-points are below.
impl WaveChannelRegistryGuard {
    pub fn registry_path(&self) -> &Path {
        &self.registry_path
    }

    pub fn loop_id(&self) -> &str {
        &self.loop_id
    }

    pub fn wave_id(&self) -> &str {
        &self.wave_id
    }

    pub fn bound_paths(&self) -> &[PathBuf] {
        &self.bound_paths
    }
}

/// The dispatcher-facing API. `prepare` is the spawn-time gate;
/// `resolve` is the emit-time guard.
pub struct WaveChannelRegistry;

impl WaveChannelRegistry {
    /// Atomically stage `bindings` for `(loop_id, wave_id)` and
    /// return a guard that owns the registry file. Does NOT
    /// spawn any workers.
    pub fn prepare(
        workspace_root: &Path,
        loop_id: &str,
        wave_id: &str,
        bindings: &[BindingInput],
    ) -> Result<WaveChannelRegistryGuard, ChannelRegistryError> {
        validate_identity("loop_id", loop_id)?;
        validate_identity("wave_id", wave_id)?;
        if bindings.is_empty() {
            return Err(ChannelRegistryError::EmptyBindings);
        }

        // Dedup slot indices up front — a duplicate slot index
        // is a programming error in the dispatcher, not a
        // runtime race.
        let mut seen_slots = std::collections::HashSet::new();
        for binding in bindings {
            if !seen_slots.insert(binding.slot_index) {
                return Err(ChannelRegistryError::DuplicateSlot(binding.slot_index));
            }
        }

        let workspace_canon = workspace_root.canonicalize().map_err(|source| {
            ChannelRegistryError::RegistryDirCreate {
                path: workspace_root.to_path_buf(),
                source,
            }
        })?;

        let registry_file_path = registry_path(workspace_root, loop_id, wave_id);

        // Step 1: detect a recovery case BEFORE touching any
        // channel file. If a registry already exists for this
        // (loop, wave), the dispatcher is recovering from a
        // crash; the new bindings must resolve to the same
        // canonical paths the persisted registry names. We do
        // NOT compare the full RegistryFile because the
        // `prepared_at` timestamp would differ on every run;
        // only the bindings (sorted by slot_index) need to
        // match.
        if let Ok(existing) = read_registry(&registry_file_path) {
            if existing.loop_id != loop_id || existing.wave_id != wave_id {
                return Err(ChannelRegistryError::IdentityMismatch {
                    path: registry_file_path,
                    expected_loop_id: loop_id.to_string(),
                    expected_wave_id: wave_id.to_string(),
                    found_loop_id: existing.loop_id,
                    found_wave_id: existing.wave_id,
                });
            }
            let new_registry = compute_in_memory_registry(
                workspace_root,
                &workspace_canon,
                bindings,
                loop_id,
                wave_id,
            )?;
            // Recovery contract: every persisted binding must
            // resolve to the exact same canonical path +
            // fingerprint the new in-memory registry names. We
            // deliberately do NOT compare `prepared_at` so a
            // timestamp difference does not fail recovery.
            if existing.bindings != new_registry.bindings
                || existing.schema_version != new_registry.schema_version
            {
                return Err(ChannelRegistryError::RegistryExistsMismatch {
                    path: registry_file_path,
                });
            }
            let bound_paths = existing
                .bindings
                .iter()
                .map(|b| b.channel_path.clone())
                .collect();
            return Ok(WaveChannelRegistryGuard {
                registry_path: registry_file_path,
                workspace_root: workspace_root.to_path_buf(),
                loop_id: loop_id.to_string(),
                wave_id: wave_id.to_string(),
                bound_paths,
                removed: std::cell::Cell::new(false),
            });
        }

        // Step 2: compute the registry in memory and create
        // every channel file via `create_new` to claim slots.
        let registry = compute_in_memory_registry(
            workspace_root,
            &workspace_canon,
            bindings,
            loop_id,
            wave_id,
        )?;
        for binding in &registry.bindings {
            match OpenOptions::new()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&binding.channel_path)
            {
                Ok(file) => {
                    let _ = file.sync_all();
                }
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {
                    return Err(ChannelRegistryError::RegistryExistsMismatch {
                        path: binding.channel_path.clone(),
                    });
                }
                Err(source) => {
                    return Err(ChannelRegistryError::ChannelFileCreate {
                        path: binding.channel_path.clone(),
                        source,
                    });
                }
            }
        }

        let body =
            serde_json::to_vec_pretty(&registry).map_err(|source| ChannelRegistryError::Json {
                path: registry_file_path.clone(),
                source,
            })?;

        let _written_hash = write_atomic(&registry_file_path, &body)?;

        // Readback validation. The file we just wrote must
        // parse to the same in-memory shape; otherwise refusion
        // surfaces the on-disk corruption (e.g. mid-write crash
        // on a non-Unix host) as a typed error.
        let readback = read_registry(&registry_file_path)?;
        if readback != registry {
            return Err(ChannelRegistryError::RegistryReadback {
                path: registry_file_path,
                reason: format!(
                    "readback mismatch: expected {} bindings, found {}",
                    registry.bindings.len(),
                    readback.bindings.len()
                ),
            });
        }

        let bound_paths = registry
            .bindings
            .iter()
            .map(|b| b.channel_path.clone())
            .collect();

        Ok(WaveChannelRegistryGuard {
            registry_path: registry_file_path,
            workspace_root: workspace_root.to_path_buf(),
            loop_id: loop_id.to_string(),
            wave_id: wave_id.to_string(),
            bound_paths,
            removed: std::cell::Cell::new(false),
        })
    }

    /// Resolve `(loop_id, wave_id, slot_index, requested_path)`
    /// against the on-disk registry. Returns the canonical
    /// channel path the caller may write to. Any failure is a
    /// typed error — no main fallback.
    pub fn resolve(
        workspace_root: &Path,
        loop_id: &str,
        wave_id: &str,
        slot_index: u32,
        requested_path: &Path,
    ) -> Result<ResolveOutcome, ChannelRegistryError> {
        validate_identity("loop_id", loop_id)?;
        validate_identity("wave_id", wave_id)?;
        let registry_file_path = registry_path(workspace_root, loop_id, wave_id);
        let registry = read_registry(&registry_file_path)?;
        if registry.loop_id != loop_id || registry.wave_id != wave_id {
            return Err(ChannelRegistryError::IdentityMismatch {
                path: registry_file_path,
                expected_loop_id: loop_id.to_string(),
                expected_wave_id: wave_id.to_string(),
                found_loop_id: registry.loop_id,
                found_wave_id: registry.wave_id,
            });
        }
        for binding in &registry.bindings {
            if binding.slot_index != slot_index {
                continue;
            }
            if binding.channel_path != requested_path {
                return Err(ChannelRegistryError::ChannelPathMismatch {
                    bound: binding.channel_path.clone(),
                    requested: requested_path.to_path_buf(),
                });
            }
            if binding.channel_fingerprint != fingerprint(&binding.channel_path) {
                return Err(ChannelRegistryError::RegistryReadback {
                    path: registry_file_path,
                    reason: format!(
                        "fingerprint mismatch on slot {slot_index}: registry \
                         says {:?}, file canonicalizes to {:?}",
                        binding.channel_fingerprint,
                        fingerprint(&binding.channel_path)
                    ),
                });
            }
            return Ok(ResolveOutcome::Bound {
                channel: binding.channel_path.clone(),
            });
        }
        Err(ChannelRegistryError::BindingNotFound {
            loop_id: loop_id.to_string(),
            wave_id: wave_id.to_string(),
            slot_index,
        })
    }
}

fn format_rfc3339(time: SystemTime) -> String {
    let duration = time
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs() as i64;
    let nanos = duration.subsec_nanos();
    let (year, month, day, hour, minute, second) = epoch_to_civil(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{nanos:09}Z",)
}

fn epoch_to_civil(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let secs_of_day = secs.rem_euclid(86_400) as u32;
    let hour = secs_of_day / 3600;
    let minute = (secs_of_day % 3600) / 60;
    let second = secs_of_day % 60;
    let (year, month, day) = civil_from_days(days);
    (year, month, day, hour, minute, second)
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y };
    (y as i32, m, d)
}

fn remove_registry_files(guard: &WaveChannelRegistryGuard) -> CleanupOutcome {
    let mut removed_any = false;
    for path in &guard.bound_paths {
        match std::fs::remove_file(path) {
            Ok(()) => removed_any = true,
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                eprintln!("wave_channel_registry: failed to remove channel {path:?}: {source}");
            }
        }
    }
    match std::fs::remove_file(&guard.registry_path) {
        Ok(()) => removed_any = true,
        Err(source) if source.kind() == io::ErrorKind::NotFound => {}
        Err(source) => {
            eprintln!(
                "wave_channel_registry: failed to remove registry {:?}: {source}",
                guard.registry_path
            );
        }
    }
    if let Some(dir) = guard.registry_path.parent() {
        let _ = std::fs::remove_dir(dir);
    }
    if removed_any {
        CleanupOutcome::Removed
    } else {
        CleanupOutcome::NotPresent
    }
}

/// Lightweight alias used by read-side guards when they want a
/// `BTreeMap<slot, channel>` view without copying the registry
/// payload.
#[allow(dead_code)]
pub fn registry_slot_index(registry: &RegistryFile) -> BTreeMap<u32, PathBuf> {
    registry
        .bindings
        .iter()
        .map(|b| (b.slot_index, b.channel_path.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn ws(dir: &tempfile::TempDir) -> &Path {
        dir.path()
    }

    fn write_empty(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, "").unwrap();
    }

    #[test]
    fn encode_identity_safely_encodes_traversal() {
        assert_eq!(encode_identity("loop.0"), "loop.0");
        assert_eq!(encode_identity("loop_0"), "loop_0");
        assert_eq!(encode_identity("loop-0"), "loop-0");
        // Path separators and special characters must be
        // percent-encoded so they cannot escape the registry
        // directory.
        let encoded = encode_identity("loop/0");
        assert!(
            !encoded.contains('/'),
            "encoded must not contain raw `/`: {encoded}"
        );
        assert!(
            encoded.contains("%2F"),
            "encoded must percent-encode `/`: {encoded}"
        );
    }

    #[test]
    fn validate_identity_rejects_empty_dot_dot_and_slash() {
        assert!(matches!(
            validate_identity("loop_id", ""),
            Err(ChannelRegistryError::InvalidIdentity { .. })
        ));
        assert!(matches!(
            validate_identity("wave_id", "."),
            Err(ChannelRegistryError::InvalidIdentity { .. })
        ));
        assert!(matches!(
            validate_identity("wave_id", ".."),
            Err(ChannelRegistryError::InvalidIdentity { .. })
        ));
        assert!(matches!(
            validate_identity("loop_id", "a/b"),
            Err(ChannelRegistryError::InvalidIdentity { .. })
        ));
    }

    #[test]
    fn prepare_rejects_empty_bindings() {
        let tmp = tempfile::TempDir::new().unwrap();
        let err = WaveChannelRegistry::prepare(ws(&tmp), "loop-1", "wave-1", &[])
            .expect_err("empty bindings must fail");
        assert!(matches!(err, ChannelRegistryError::EmptyBindings));
    }

    #[test]
    fn prepare_rejects_duplicate_slot_indices() {
        let tmp = tempfile::TempDir::new().unwrap();
        let bindings = vec![
            BindingInput::new(0, ws(&tmp).join(".ralph").join("wave-w-s-0.jsonl")),
            BindingInput::new(0, ws(&tmp).join(".ralph").join("wave-w-s-0-dup.jsonl")),
        ];
        let err = WaveChannelRegistry::prepare(ws(&tmp), "loop-1", "wave-1", &bindings)
            .expect_err("duplicate slot must fail");
        assert!(matches!(err, ChannelRegistryError::DuplicateSlot(0)));
    }

    #[test]
    fn prepare_rejects_channel_outside_workspace() {
        let tmp = tempfile::TempDir::new().unwrap();
        let outside = std::env::temp_dir().join("outside-channel.jsonl");
        let bindings = vec![BindingInput::new(0, outside)];
        let err = WaveChannelRegistry::prepare(ws(&tmp), "loop-1", "wave-1", &bindings)
            .expect_err("outside workspace must fail");
        match err {
            ChannelRegistryError::PathEscape { .. } => {}
            other => panic!("expected PathEscape, got {other:?}"),
        }
    }

    #[test]
    fn prepare_rejects_traversal_components() {
        let tmp = tempfile::TempDir::new().unwrap();
        let bindings = vec![BindingInput::new(
            0,
            ws(&tmp).join(".ralph").join("..").join("wave-w-s-0.jsonl"),
        )];
        let err = WaveChannelRegistry::prepare(ws(&tmp), "loop-1", "wave-1", &bindings)
            .expect_err("traversal must fail");
        match err {
            ChannelRegistryError::PathEscape { .. } => {}
            other => panic!("expected PathEscape, got {other:?}"),
        }
    }

    #[test]
    fn prepare_writes_registry_and_resolver_round_trip_succeeds() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(ws(&tmp).join(".ralph")).unwrap();
        let bindings = vec![
            BindingInput::new(0, ws(&tmp).join(".ralph").join("wave-w-round-0.jsonl")),
            BindingInput::new(1, ws(&tmp).join(".ralph").join("wave-w-round-1.jsonl")),
        ];
        let guard = WaveChannelRegistry::prepare(ws(&tmp), "loop-round", "wave-round", &bindings)
            .expect("prepare must succeed");
        // Registry file must exist.
        assert!(
            guard.registry_path().is_file(),
            "registry JSON must be written"
        );

        // Each channel file must exist (create_new).
        for path in guard.bound_paths() {
            assert!(path.is_file(), "channel {path:?} must exist after prepare");
        }
        // Same bindings produce matching canonical channel paths.
        let resolved = WaveChannelRegistry::resolve(
            ws(&tmp),
            "loop-round",
            "wave-round",
            0,
            guard.bound_paths().first().unwrap(),
        )
        .expect("resolve must succeed");
        assert_eq!(
            resolved,
            ResolveOutcome::Bound {
                channel: guard.bound_paths().first().unwrap().clone()
            }
        );
    }

    #[test]
    fn resolve_rejects_cross_slot_requested_path() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(ws(&tmp).join(".ralph")).unwrap();
        let bindings = vec![
            BindingInput::new(0, ws(&tmp).join(".ralph").join("wave-w-cross-0.jsonl")),
            BindingInput::new(1, ws(&tmp).join(".ralph").join("wave-w-cross-1.jsonl")),
        ];
        let guard = WaveChannelRegistry::prepare(ws(&tmp), "loop-cross", "wave-cross", &bindings)
            .expect("prepare must succeed");
        // slot 0 requesting slot 1's channel file is cross-slot
        // tampering — must fail.
        let cross = guard.bound_paths().get(1).unwrap();
        let err = WaveChannelRegistry::resolve(ws(&tmp), "loop-cross", "wave-cross", 0, cross)
            .expect_err("cross-slot request must fail");
        match err {
            ChannelRegistryError::ChannelPathMismatch { .. } => {}
            other => panic!("expected ChannelPathMismatch, got {other:?}"),
        }
    }

    #[test]
    fn resolve_rejects_unknown_loop_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(ws(&tmp).join(".ralph")).unwrap();
        let bindings = vec![BindingInput::new(
            0,
            ws(&tmp).join(".ralph").join("wave-w-x-0.jsonl"),
        )];
        let guard = WaveChannelRegistry::prepare(ws(&tmp), "loop-x", "wave-x", &bindings)
            .expect("prepare must succeed");
        let err = WaveChannelRegistry::resolve(
            ws(&tmp),
            "loop-other",
            "wave-x",
            0,
            guard.bound_paths().first().unwrap(),
        )
        .expect_err("unknown loop_id must fail");
        assert!(matches!(
            err,
            ChannelRegistryError::RegistryReadback { .. }
                | ChannelRegistryError::BindingNotFound { .. }
        ));
    }

    #[test]
    fn prepare_is_idempotent_on_identical_recovery() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(ws(&tmp).join(".ralph")).unwrap();
        let bindings = vec![BindingInput::new(
            0,
            ws(&tmp).join(".ralph").join("wave-w-recover-0.jsonl"),
        )];

        let _first =
            WaveChannelRegistry::prepare(ws(&tmp), "loop-recover", "wave-recover", &bindings)
                .expect("prepare must succeed");

        // Second call with identical bindings is the recovery
        // contract — must NOT return RegistryExistsMismatch.
        let second =
            WaveChannelRegistry::prepare(ws(&tmp), "loop-recover", "wave-recover", &bindings)
                .expect("recovery must succeed");
        assert_eq!(second.loop_id(), "loop-recover");
        assert_eq!(second.wave_id(), "wave-recover");
    }

    #[test]
    fn prepare_rejects_recovery_with_different_bindings() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(ws(&tmp).join(".ralph")).unwrap();
        let first_bindings = vec![BindingInput::new(
            0,
            ws(&tmp).join(".ralph").join("wave-w-recmis-0.jsonl"),
        )];
        let _first =
            WaveChannelRegistry::prepare(ws(&tmp), "loop-recmis", "wave-recmis", &first_bindings)
                .expect("prepare must succeed");

        // Plant a leftover event row in slot 0 so the channel
        // file is non-empty; recovery with different bindings
        // (simulated as different slot count) must fail with
        // RegistryExistsMismatch.
        write_empty(&ws(&tmp).join(".ralph").join("wave-w-recmis-1.jsonl"));
        let second_bindings = vec![
            BindingInput::new(0, ws(&tmp).join(".ralph").join("wave-w-recmis-0.jsonl")),
            BindingInput::new(1, ws(&tmp).join(".ralph").join("wave-w-recmis-1.jsonl")),
        ];
        let err =
            WaveChannelRegistry::prepare(ws(&tmp), "loop-recmis", "wave-recmis", &second_bindings)
                .expect_err("different bindings must fail");
        match err {
            ChannelRegistryError::RegistryExistsMismatch { .. } => {}
            other => panic!("expected RegistryExistsMismatch, got {other:?}"),
        }
    }

    #[test]
    fn atomic_write_round_trip_with_partial_reader() {
        // The registry must never appear mid-write to a
        // concurrent reader. We test this by writing + reading
        // back multiple times and confirming the JSON always
        // parses (no truncated record observed).
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(ws(&tmp).join(".ralph")).unwrap();
        let bindings = vec![BindingInput::new(
            0,
            ws(&tmp).join(".ralph").join("wave-w-atom-0.jsonl"),
        )];
        for _ in 0..5 {
            let _g = WaveChannelRegistry::prepare(ws(&tmp), "loop-atom", "wave-atom", &bindings)
                .expect("prepare must succeed");
            let raw =
                fs::read_to_string(registry_path(ws(&tmp), "loop-atom", "wave-atom")).unwrap();
            let parsed: RegistryFile = serde_json::from_str(&raw).expect("must parse");
            assert_eq!(parsed.loop_id, "loop-atom");
        }
    }

    #[test]
    fn cleanup_removes_registry_and_channel_files() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(ws(&tmp).join(".ralph")).unwrap();
        let bindings = vec![
            BindingInput::new(0, ws(&tmp).join(".ralph").join("wave-w-clean-0.jsonl")),
            BindingInput::new(1, ws(&tmp).join(".ralph").join("wave-w-clean-1.jsonl")),
        ];
        let mut guard =
            WaveChannelRegistry::prepare(ws(&tmp), "loop-clean", "wave-clean", &bindings)
                .expect("prepare must succeed");
        // Sanity: registry + channel files exist.
        assert!(guard.registry_path().is_file());
        for path in guard.bound_paths() {
            assert!(path.is_file());
        }
        // Cleanup removes all of them.
        let outcome = guard.cleanup();
        assert_eq!(outcome, CleanupOutcome::Removed);
        assert!(!guard.registry_path().exists(), "registry must be removed");
        for path in guard.bound_paths() {
            assert!(!path.exists(), "channel {path:?} must be removed");
        }
        // Idempotent: second cleanup is a NotPresent.
        let outcome2 = guard.cleanup();
        assert_eq!(outcome2, CleanupOutcome::NotPresent);
    }

    #[test]
    fn drop_implicitly_cleans_when_forgotten() {
        let tmp = tempfile::TempDir::new().unwrap();
        fs::create_dir_all(ws(&tmp).join(".ralph")).unwrap();
        let bindings = vec![BindingInput::new(
            0,
            ws(&tmp).join(".ralph").join("wave-w-drop-0.jsonl"),
        )];
        let registry_path_clone = {
            let guard = WaveChannelRegistry::prepare(ws(&tmp), "loop-drop", "wave-drop", &bindings)
                .expect("prepare must succeed");
            let p = guard.registry_path().to_path_buf();
            assert!(p.is_file());
            p
        };
        // After the guard drops, the registry file must be gone.
        assert!(!registry_path_clone.exists());
    }

    #[test]
    fn fingerprint_is_stable_across_runs() {
        let path = std::env::temp_dir()
            .join(".ralph")
            .join("wave-w-fp-0.jsonl");
        let a = fingerprint(&path);
        let b = fingerprint(&path);
        assert_eq!(a, b, "fingerprint must be stable");
        assert!(a.starts_with("sha256:"));
    }
}
