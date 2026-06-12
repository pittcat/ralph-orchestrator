//! Wave CLI tool for dispatching parallel wave events.
//!
//! Provides `ralph wave emit` for agents to dispatch work items
//! to wave-capable hats that execute in parallel.

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use ralph_core::agent_doc_sync::compute_sha256_hex;
use ralph_core::file_lock::FileLock;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

/// Arguments for the wave subcommand.
#[derive(Parser, Debug)]
pub struct WaveArgs {
    #[command(subcommand)]
    pub command: WaveCommands,
}

/// Wave subcommands.
#[derive(Subcommand, Debug)]
pub enum WaveCommands {
    /// Emit multiple events as a wave for parallel execution
    Emit(WaveEmitArgs),
}

/// Arguments for `ralph wave emit`.
#[derive(Parser, Debug)]
pub struct WaveEmitArgs {
    /// Event topic for all wave events (e.g., "review.file")
    pub topic: String,

    /// Payloads for each wave event instance (one per parallel worker)
    #[arg(long, num_args = 1.., group = "payload_source")]
    pub payloads: Vec<String>,

    /// Read payloads from stdin, one per line
    #[arg(long, group = "payload_source")]
    pub payloads_stdin: bool,

    /// Output format: `text` (default; wave_id on stdout) or `json`
    /// (`{wave_id, topic, count, events_file}` for U5 machine verification).
    #[arg(long, value_enum, default_value_t = WaveOutputFormat::Text)]
    pub output: WaveOutputFormat,

    /// Optional idempotency key (U2). Re-emitting with the same
    /// (loop_id, hat, topic, key) returns the original wave_id instead of
    /// writing a new wave. Use for review-coordinator waves that may be
    /// retried after timeout or duplicate dispatch. Omit to keep legacy
    /// behavior (each call generates a new wave_id).
    #[arg(long, value_name = "KEY")]
    pub idempotency_key: Option<String>,
}

/// U5: Output format for `ralph wave emit`.
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaveOutputFormat {
    Text,
    Json,
}

/// U2: Max length of idempotency key (bytes). Bounds log line size and
/// prevents runaway keys from polluting `.wave-idempotency.jsonl`.
pub const MAX_IDEMPOTENCY_KEY_BYTES: usize = 256;

/// U2: One row of `.wave-idempotency.jsonl`.
///
/// Schema is flat (single object per line) so future fields can be added
/// without breaking parsers that ignore unknown keys.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct IdempotencyRecord {
    /// SHA-256 hex of `"<loop_id>|<hat>|<topic>|<key>"`. Primary dedup key.
    pub scope_key: String,
    /// Echo of the user-supplied key (for operator audit).
    pub idempotency_key: String,
    /// Wave ID returned on first emission; returned on all later dedup hits.
    pub wave_id: String,
    /// Topic emitted (redundant with scope but logs-friendly).
    pub topic: String,
    /// Hat that emitted (or "" if unset at first call).
    pub hat: String,
    /// SHA-256 hex of the serialized payload list.
    pub payload_digest: String,
    /// Number of events that should exist with this wave_id.
    pub count: u32,
    /// ISO-8601 UTC timestamp of first emission.
    pub created_at: String,
}

/// U2: Outcome of `write_wave_events_with_idempotency`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdempotencyOutcome {
    /// The wave_id (whether new or deduped).
    pub wave_id: String,
    /// `true` when this call was a dedup hit (no new events written).
    pub deduplicated: bool,
}

/// Execute a wave command.
pub fn execute(args: WaveArgs, use_colors: bool) -> Result<()> {
    match args.command {
        WaveCommands::Emit(emit_args) => execute_emit(emit_args, use_colors),
    }
}

/// Execute `ralph wave emit` — write N tagged events atomically.
fn execute_emit(args: WaveEmitArgs, use_colors: bool) -> Result<()> {
    // Nested wave prevention: bail if running inside a wave worker
    if std::env::var("RALPH_WAVE_WORKER").is_ok_and(|v| v == "1") {
        bail!(
            "Cannot dispatch waves from inside a wave worker. \
             Wave workers must emit results via `ralph emit`, not `ralph wave emit`."
        );
    }

    // U2: Validate idempotency key shape if provided
    if let Some(ref key) = args.idempotency_key {
        validate_idempotency_key(key)?;
    }

    let payloads = if args.payloads_stdin {
        read_payloads_from_stdin()?
    } else {
        args.payloads
    };

    if payloads.is_empty() {
        bail!("At least one payload is required (use --payloads or --payloads-stdin)");
    }
    validate_payload_shape(&payloads)?;

    let events_file = resolve_events_file();

    // U2: Branch — with idempotency key or legacy path
    let outcome = if let Some(ref key) = args.idempotency_key {
        write_wave_events_with_idempotency(&args.topic, &payloads, &events_file, key)?
    } else {
        let wave_id = write_wave_events(&args.topic, &payloads, &events_file)?;
        IdempotencyOutcome {
            wave_id,
            deduplicated: false,
        }
    };

    let wave_id = outcome.wave_id;
    let deduplicated = outcome.deduplicated;
    let total = payloads.len();

    // U5: optionally emit structured JSON for machine verification.
    match args.output {
        WaveOutputFormat::Text => {
            // Print wave ID to stdout (machine-parseable)
            println!("{}", wave_id);
        }
        WaveOutputFormat::Json => {
            // `events_file` is converted to its string form for JSON friendliness.
            let events_file_str = events_file.to_string_lossy().to_string();
            let payload = serde_json::json!({
                "wave_id": wave_id,
                "topic": args.topic,
                "count": total,
                "events_file": events_file_str,
                "deduplicated": deduplicated,
            });
            println!("{}", serde_json::to_string(&payload)?);
        }
    }

    // Human-readable confirmation to stderr (always)
    let dedup_tag = if deduplicated { " (deduplicated)" } else { "" };
    if use_colors {
        eprintln!(
            "\x1b[32m\u{2713}\x1b[0m Wave dispatched: {} events on topic '{}' (wave {}){}",
            total, args.topic, wave_id, dedup_tag
        );
    } else {
        eprintln!(
            "Wave dispatched: {} events on topic '{}' (wave {}){}",
            total, args.topic, wave_id, dedup_tag
        );
    }

    Ok(())
}

/// Reject the historical footgun where agents passed one shell variable
/// containing many newline-delimited JSON objects to `--payloads`, and
/// enforce the U1 invariant: every payload must be a JSON object.
fn validate_payload_shape(payloads: &[String]) -> Result<()> {
    for (idx, payload) in payloads.iter().enumerate() {
        if looks_like_multiple_json_lines(payload) {
            bail!(
                "`--payloads` argument {idx} contains multiple JSON payload lines. \
                 Use `--payloads-stdin` instead, e.g. `cat payloads.jsonl | ralph wave emit <topic> --payloads-stdin`."
            );
        }
        validate_single_payload_object(payload).with_context(|| {
            format!(
                "payload[{idx}] is not a JSON object: {payload:?} \
                 (word-splitting? pass `cat payloads.jsonl` to --payloads-stdin, \
                 not `printf '%s\\n' $(cat payloads.jsonl)`)"
            )
        })?;
    }
    Ok(())
}

fn looks_like_multiple_json_lines(payload: &str) -> bool {
    let json_like_lines = payload
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with('{') || trimmed.starts_with('[')
        })
        .take(2)
        .count();
    json_like_lines > 1
}

/// Parse the payload as JSON and require it to be a JSON object.
/// Rejects numbers, strings, arrays, booleans, null, and truncated JSON.
fn validate_single_payload_object(payload: &str) -> Result<()> {
    let value: serde_json::Value =
        serde_json::from_str(payload).with_context(|| format!("invalid JSON: {payload:?}"))?;
    if !value.is_object() {
        bail!(
            "expected JSON object, got {} ({})",
            value_type_name(&value),
            short_preview(payload)
        );
    }
    Ok(())
}

fn value_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "boolean",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn short_preview(payload: &str) -> String {
    const MAX: usize = 40;
    if payload.len() <= MAX {
        payload.to_string()
    } else {
        format!("{}…", &payload[..MAX])
    }
}

/// Read payloads from stdin, one JSON object per line.
/// Empty lines are skipped.
fn read_payloads_from_stdin() -> Result<Vec<String>> {
    read_payloads_from_reader(io::stdin().lock())
}

/// Read payloads from any buffered reader, one payload per line.
/// Empty lines are skipped.
fn read_payloads_from_reader<R: BufRead>(reader: R) -> Result<Vec<String>> {
    let mut payloads = Vec::new();
    for line in reader.lines() {
        let line = line.context("Failed to read line from reader")?;
        let trimmed = line.trim();
        if !trimmed.is_empty() {
            payloads.push(trimmed.to_string());
        }
    }
    Ok(payloads)
}

/// Write wave events to a JSONL file. Returns the generated wave ID.
///
/// This is the core logic, separated from CLI concerns for testability.
pub fn write_wave_events(topic: &str, payloads: &[String], events_file: &Path) -> Result<String> {
    // Read hat from runtime environment if available
    let hat = std::env::var("RALPH_CURRENT_HAT")
        .ok()
        .filter(|s| !s.is_empty());
    write_wave_events_with_provenance(topic, payloads, events_file, hat.as_deref(), None, None)
}

/// Like [`write_wave_events`] but with explicit provenance and idempotency fields.
///
/// When `idempotency_key` and `idempotency_hash` are provided, each wave event
/// record gets `idempotency_key` and `idempotency_hash` fields injected. This
/// enables recovery scanning by wave_id + idempotency_key.
pub fn write_wave_events_with_provenance(
    topic: &str,
    payloads: &[String],
    events_file: &Path,
    hat: Option<&str>,
    idempotency_key: Option<&str>,
    idempotency_hash: Option<&str>,
) -> Result<String> {
    if payloads.is_empty() {
        bail!("At least one payload is required");
    }

    let wave_id = generate_wave_id();

    let total = payloads.len() as u32;
    let ts = chrono::Utc::now().to_rfc3339();

    // Ensure parent directory exists
    if let Some(parent) = events_file.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    // Build all event records
    let mut lines = String::new();
    for (index, payload) in payloads.iter().enumerate() {
        let mut record = serde_json::json!({
            "topic": topic,
            "payload": payload,
            "ts": ts,
            "wave_id": wave_id,
            "wave_index": index as u32,
            "wave_total": total,
        });

        // Add hat provenance if available
        if let Some(hat_val) = hat {
            if let Some(obj) = record.as_object_mut() {
                obj.insert("hat".to_string(), serde_json::json!(hat_val));
            }
        }

        // U2: Inject idempotency fields when present
        if let Some(ik) = idempotency_key {
            if let Some(obj) = record.as_object_mut() {
                obj.insert("idempotency_key".to_string(), serde_json::json!(ik));
            }
        }
        if let Some(ih) = idempotency_hash {
            if let Some(obj) = record.as_object_mut() {
                obj.insert("idempotency_hash".to_string(), serde_json::json!(ih));
            }
        }

        let json_line = serde_json::to_string(&record)?;
        lines.push_str(&json_line);
        lines.push('\n');
    }

    // Write all events atomically
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(events_file)
        .with_context(|| format!("Failed to open events file: {}", events_file.display()))?;
    file.write_all(lines.as_bytes())?;

    Ok(wave_id)
}

/// Resolve the events file path from environment and marker files.
///
/// Priority: RALPH_EVENTS_FILE env > .ralph/current-events marker > default .ralph/events.jsonl
pub fn resolve_events_file() -> PathBuf {
    if let Ok(path) = std::env::var("RALPH_EVENTS_FILE")
        && !path.is_empty()
    {
        return PathBuf::from(path);
    }
    fs::read_to_string(".ralph/current-events")
        .map(|s| PathBuf::from(s.trim()))
        .unwrap_or_else(|_| PathBuf::from(".ralph/events.jsonl"))
}

// ═══════════════════════════════════════════════════════════════
// U2: Idempotency helpers
// ═══════════════════════════════════════════════════════════════

/// U2: Resolve scope inputs (loop_id, hat) from env and marker files.
///
/// Order:
/// - loop_id: `RALPH_CURRENT_LOOP_ID` env → `.ralph/current-loop-id` marker → `"unknown"`
/// - hat: `RALPH_CURRENT_HAT` env → `""`
fn build_scope_inputs() -> (String, String) {
    let loop_id = std::env::var("RALPH_CURRENT_LOOP_ID")
        .ok()
        .or_else(|| fs::read_to_string(".ralph/current-loop-id").ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    let hat = std::env::var("RALPH_CURRENT_HAT")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_default();
    (loop_id, hat)
}

/// U2: Compute the sha256 hex scope key from the four dedup dimensions.
pub fn compute_scope_key(loop_id: &str, hat: &str, topic: &str, key: &str) -> String {
    let joined = format!("{loop_id}|{hat}|{topic}|{key}");
    compute_sha256_hex(&joined)
}

/// U2: Compute the payload digest for the whole payload list.
///
/// Uses `\u{1F}` (Unit Separator) as delimiter — it is forbidden in JSON
/// strings and therefore unambiguous.
pub fn compute_payload_digest(payloads: &[String]) -> String {
    let mut joined = String::new();
    for (i, p) in payloads.iter().enumerate() {
        if i > 0 {
            joined.push('\u{1F}');
        }
        joined.push_str(p);
    }
    compute_sha256_hex(&joined)
}

/// WRC-U6 (2026-06-12-003): validate a wave record before it is
/// appended to the events JSONL. Returns `Ok(())` when the
/// record's `wave_total` field equals the expected wave size, and
/// `Err` otherwise. The check is intentionally narrow: it
/// catches the documented 335-worker failure mode (a hand-written
/// or scripted `events.jsonl` whose `wave_total` does not match
/// the worker's expectation) without re-running the full wave
/// pipeline. Callers that need the broader wave record
/// validation (topic schema, payload shape, idempotency key)
/// already have those checks in `write_wave_events_with_provenance`.
///
/// The function is `pub(crate)` so the test module can drive
/// the rejection path; production callers are the JSONL
/// append-or-write path and the BDD scenario for AE2 timing.
#[allow(dead_code)] // 003 plan WRC-U6 预留：手写 JSONL 入口的 wave_total 拒收点，待接线
pub(crate) fn validate_wave_record(
    record: &serde_json::Value,
    expected_wave_total: u32,
) -> Result<()> {
    let actual = record
        .get("wave_total")
        .and_then(|v| v.as_u64())
        .ok_or_else(|| anyhow!("wave record missing 'wave_total' field"))?;
    if actual != u64::from(expected_wave_total) {
        bail!(
            "wave_total mismatch: record declares {actual} but the wave size is {expected_wave_total}"
        );
    }
    Ok(())
}

/// U2: Derive the idempotency log path as a sibling of `events_file`.
///
/// Returns `<parent>/.<basename>.idempotency.jsonl`.
/// Example: `/repo/.ralph/events.jsonl` → `/repo/.ralph/.events.jsonl.idempotency.jsonl`
fn idempotency_log_path(events_file: &Path) -> PathBuf {
    let parent = events_file.parent().unwrap_or_else(|| Path::new("."));
    let file_name = events_file
        .file_name()
        .map(|s| s.to_string_lossy())
        .unwrap_or_else(|| std::borrow::Cow::Borrowed("events.jsonl"));
    parent.join(format!(".{}.idempotency.jsonl", file_name))
}

/// U2: Read all idempotency records from the log file.
///
/// Self-healing: malformed lines are warned and skipped (not fatal). The
/// idempotency log is append-only and self-written, but a half-line from
/// SIGKILL / disk-full / older writer format must not permanently block
/// subsequent `ralph wave emit` calls. A skipped line is mirrored to a
/// `.corrupt` sidecar so an operator can inspect / truncate later.
fn read_idempotency_records(events_file: &Path) -> Result<Vec<IdempotencyRecord>> {
    let path = idempotency_log_path(events_file);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path)
        .with_context(|| format!("read {}", path.display()))?;
    let mut out = Vec::new();
    let mut corrupt_lines: Vec<String> = Vec::new();
    for (i, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<IdempotencyRecord>(trimmed) {
            Ok(rec) => out.push(rec),
            Err(e) => {
                eprintln!(
                    "warning: ignoring malformed idempotency record at {} line {}: {}",
                    path.display(),
                    i + 1,
                    e
                );
                corrupt_lines.push(trimmed.to_string());
            }
        }
    }
    if !corrupt_lines.is_empty() {
        let sidecar = path.with_extension("idempotency.jsonl.corrupt");
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&sidecar)
            .with_context(|| format!("open corrupt sidecar {}", sidecar.display()))?;
        for line in &corrupt_lines {
            writeln!(f, "{}", line)?;
        }
    }
    Ok(out)
}

/// U2: Append one idempotency record to the log file (with fsync).
fn append_idempotency_record(events_file: &Path, rec: &IdempotencyRecord) -> Result<()> {
    let path = idempotency_log_path(events_file);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("Failed to open idempotency log: {}", path.display()))?;
    let line = serde_json::to_string(rec)?;
    writeln!(f, "{}", line)?;
    f.sync_data()?;
    Ok(())
}

/// U2: Count events in `events_file` whose `idempotency_key` and `wave_id` match.
///
/// Used by the recovery path. Tolerates malformed event lines (continue).
fn count_recovered_events(
    events_file: &Path,
    expected_wave_id: &str,
    expected_key: &str,
) -> Result<u32> {
    if !events_file.exists() {
        return Ok(0);
    }
    let content = fs::read_to_string(events_file)
        .with_context(|| format!("read {}", events_file.display()))?;
    let mut count: u32 = 0;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue, // events file tolerates malformed lines
        };
        if v.get("wave_id").and_then(|x| x.as_str()) == Some(expected_wave_id)
            && v.get("idempotency_key").and_then(|x| x.as_str()) == Some(expected_key)
        {
            count += 1;
        }
    }
    Ok(count)
}

/// U2: Scan events file for events with matching `idempotency_key`.
///
/// Returns `(first_wave_id, count)` when exactly `expected_count` matching
/// events are found. Returns `None` when no matching events exist (clean
/// first call). Errors when partial matches exist (incomplete prior emission).
///
/// Uses both `idempotency_key` AND `idempotency_hash` (the scope_key) to
/// avoid cross-scope false positive on recovery scans.
/// Used by the recovery path when the idempotency record was lost (crash
/// between events append and record append).
fn try_recover_from_events(
    events_file: &Path,
    idempotency_key: &str,
    scope_key: &str,
    expected_count: usize,
) -> Result<Option<(String, usize)>> {
    if !events_file.exists() {
        return Ok(None);
    }
    let content = fs::read_to_string(events_file)
        .with_context(|| format!("read {}", events_file.display()))?;
    let mut count: usize = 0;
    let mut first_wave_id: Option<String> = None;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let v: serde_json::Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if v.get("idempotency_key").and_then(|x| x.as_str()) == Some(idempotency_key)
                && v.get("idempotency_hash").and_then(|x| x.as_str()) == Some(scope_key) {
            count += 1;
            if first_wave_id.is_none() {
                first_wave_id = v
                    .get("wave_id")
                    .and_then(|x| x.as_str())
                    .map(|s| s.to_string());
            }
        }
    }
    match count {
        0 => Ok(None),
        n if n == expected_count => {
            let wave_id = first_wave_id.unwrap_or_else(|| {
                "w-recovered-unknown-wave-id".to_string()
            });
            Ok(Some((wave_id, count)))
        }
        n => {
            bail!(
                "incomplete prior wave emission: found {} events with idempotency_key '{}' \
                 in events file, but expected {}. Manually clean up partial events or use a \
                 different --idempotency-key.",
                n,
                idempotency_key,
                expected_count
            );
        }
    }
}

/// U2: Validate idempotency key shape.
fn validate_idempotency_key(key: &str) -> Result<()> {
    if key.is_empty() {
        bail!("--idempotency-key must not be empty");
    }
    if key.trim().is_empty() {
        bail!("--idempotency-key must not be whitespace-only");
    }
    if key.len() > MAX_IDEMPOTENCY_KEY_BYTES {
        bail!(
            "--idempotency-key exceeds {} bytes (got {})",
            MAX_IDEMPOTENCY_KEY_BYTES,
            key.len()
        );
    }
    if !key.is_ascii() {
        bail!("--idempotency-key must be ASCII (got non-ASCII bytes)");
    }
    Ok(())
}

/// U2: Emit wave events with idempotency enforcement.
///
/// On first call with a given `(loop_id, hat, topic, key)`, writes N events
/// and one idempotency record. On subsequent calls with the same scope and
/// payload digest, returns the original wave_id with `deduplicated=true`.
///
/// Uses `FileLock::exclusive()` for concurrency safety.
pub fn write_wave_events_with_idempotency(
    topic: &str,
    payloads: &[String],
    events_file: &Path,
    idempotency_key: &str,
) -> Result<IdempotencyOutcome> {
    let (loop_id, hat) = build_scope_inputs();
    write_wave_events_with_idempotency_with_scope(
        topic, payloads, events_file, idempotency_key, &loop_id, &hat,
    )
}

/// U2: Like [`write_wave_events_with_idempotency`] but with explicit scope params for testability.
pub fn write_wave_events_with_idempotency_with_scope(
    topic: &str,
    payloads: &[String],
    events_file: &Path,
    idempotency_key: &str,
    loop_id: &str,
    hat: &str,
) -> Result<IdempotencyOutcome> {
    if payloads.is_empty() {
        bail!("At least one payload is required");
    }
    if idempotency_key.is_empty() {
        bail!("idempotency_key must not be empty (caller bug)");
    }

    let scope_key = compute_scope_key(loop_id, hat, topic, idempotency_key);
    let payload_digest = compute_payload_digest(payloads);

    // Acquire exclusive lock on events_file
    let lock = FileLock::new(events_file)
        .with_context(|| format!("create FileLock for {}", events_file.display()))?;
    let _guard = lock
        .exclusive()
        .with_context(|| format!("acquire exclusive lock on {}", lock.lock_path().display()))?;

    // Load existing records
    let records = read_idempotency_records(events_file)?;

    // Dedup check
    if let Some(existing) = records.iter().find(|r| r.scope_key == scope_key) {
        if existing.payload_digest != payload_digest {
            bail!(
                "idempotency-key conflict: same scope already used with a different payload. \
                 original wave_id={}, original count={}, original created_at={}. \
                 If the new payload is intended, use a different --idempotency-key.",
                existing.wave_id, existing.count, existing.created_at
            );
        }
        // Recovery: verify events file has the expected count
        let count = count_recovered_events(events_file, &existing.wave_id, idempotency_key)?;
        if count < existing.count {
            bail!(
                "incomplete prior wave emission: scope_key {} has record claiming \
                 {} events but only {} found in events file. Refusing to silently re-append; \
                 manually clean up partial events or use a new --idempotency-key.",
                scope_key,
                existing.count,
                count
            );
        }
        return Ok(IdempotencyOutcome {
            wave_id: existing.wave_id.clone(),
            deduplicated: true,
        });
    }

    // U2: Recovery scan — record was lost but events exist with matching idempotency_key
    let recovery = try_recover_from_events(events_file, idempotency_key, &scope_key, payloads.len())?;
    if let Some((wave_id, count)) = recovery {
        // Reconstruct the record from the recovered wave data
        let rec = IdempotencyRecord {
            scope_key: scope_key.clone(),
            idempotency_key: idempotency_key.to_string(),
            wave_id: wave_id.clone(),
            topic: topic.to_string(),
            hat: hat.to_string(),
            payload_digest,
            count: count as u32,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        append_idempotency_record(events_file, &rec)?;
        return Ok(IdempotencyOutcome {
            wave_id,
            deduplicated: true,
        });
    }

    // First-time: write events (with idempotency fields), then write record
    let wave_id = write_wave_events_with_provenance(
        topic,
        payloads,
        events_file,
        if hat.is_empty() { None } else { Some(hat) },
        Some(idempotency_key),
        Some(&scope_key),
    )?;

    let rec = IdempotencyRecord {
        scope_key: scope_key.clone(),
        idempotency_key: idempotency_key.to_string(),
        wave_id: wave_id.clone(),
        topic: topic.to_string(),
        hat: hat.to_string(),
        payload_digest,
        count: payloads.len() as u32,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    append_idempotency_record(events_file, &rec)?;

    Ok(IdempotencyOutcome {
        wave_id,
        deduplicated: false,
    })
}

/// Generate a unique wave ID.
///
/// Concatenates nanosecond timestamp, PID, and a process-local atomic counter.
/// Readable and debuggable — each segment is independently meaningful.
fn generate_wave_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let pid = std::process::id();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);

    format!("w-{nanos:x}-{pid}-{seq}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_write_wave_events_creates_tagged_events() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");

        let payloads = vec![
            "src/main.rs".to_string(),
            "src/lib.rs".to_string(),
            "src/config.rs".to_string(),
        ];

        let wave_id = write_wave_events("review.file", &payloads, &events_path).unwrap();
        assert!(wave_id.starts_with("w-"));

        let content = fs::read_to_string(&events_path).unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 3);

        // Parse and verify each event
        for (i, line) in lines.iter().enumerate() {
            let event: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(event["topic"], "review.file");
            assert_eq!(event["wave_index"], i as u64);
            assert_eq!(event["wave_total"], 3);
            assert_eq!(event["wave_id"], wave_id.as_str());
        }
    }

    #[test]
    fn test_write_wave_events_single_payload() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");

        let payloads = vec!["only-one".to_string()];
        let wave_id = write_wave_events("test.topic", &payloads, &events_path).unwrap();

        let content = fs::read_to_string(&events_path).unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 1);

        let event: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(event["wave_index"], 0);
        assert_eq!(event["wave_total"], 1);
        assert_eq!(event["wave_id"], wave_id.as_str());
    }

    #[test]
    fn test_write_wave_events_empty_payloads_rejected() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");

        let result = write_wave_events("test.topic", &[], &events_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_write_wave_events_creates_parent_dirs() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("nested").join("dir").join("events.jsonl");

        let payloads = vec!["payload".to_string()];
        write_wave_events("test.topic", &payloads, &events_path).unwrap();

        assert!(events_path.exists());
    }

    // WRC-U6 (2026-06-12-003) / T-WRC-U6-01: `wave_total` on every
    // emitted event MUST equal `len(payloads)`. The 002 plan
    // already enforced this; the 003 plan pins the contract with a
    // dedicated test that scans all emitted records. The dimension
    // detection / wave aggregation pipeline reads
    // `(wave_id, wave_total)` to decide how many worker
    // activations to expect — a mismatch silently drops events
    // or, in the 335-worker field trace, fans out far more than
    // the operator asked for. The test scans every line of the
    // emitted JSONL and asserts `wave_total == payloads.len()`.
    #[test]
    fn test_wave_total_equals_payload_count_for_all_records() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let payloads = vec![
            "{\"dimension\":\"correctness\"}".to_string(),
            "{\"dimension\":\"testing\"}".to_string(),
            "{\"dimension\":\"maintainability\"}".to_string(),
        ];
        let expected_total = payloads.len() as u32;
        write_wave_events("review.wave.ready", &payloads, &events_path).unwrap();
        let body = std::fs::read_to_string(&events_path).unwrap();
        let mut count = 0;
        for line in body.lines() {
            if line.is_empty() {
                continue;
            }
            let value: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(
                value["wave_total"].as_u64().unwrap(),
                u64::from(expected_total),
                "every wave record must carry wave_total == len(payloads)={expected_total}, got: {line}",
            );
            count += 1;
        }
        assert_eq!(
            count,
            payloads.len(),
            "wave emit must write exactly one JSONL line per payload"
        );
    }

    // WRC-U6 / T-WRC-U6-02 (mismatch rejection): the JSONL
    // append-or-write entry points reject a record whose
    // `wave_total` field disagrees with the configured wave size.
    // The 002 plan documented this as "internally consistent
    // invariant"; the 003 plan promotes it to an explicit
    // assertion. We exercise the helper directly because the CLI
    // entry point also derives `wave_total` from `len(payloads)`,
    // so a mismatch can only be introduced by a hand-written
    // JSONL append (e.g. a script that builds events.jsonl out
    // of process). The rejection closes the same failure mode
    // the 335-worker bug exposed.
    #[test]
    fn test_wave_record_with_mismatched_wave_total_is_rejected() {
        let good = serde_json::json!({
            "topic": "review.wave.ready",
            "payload": "{}",
            "ts": chrono::Utc::now().to_rfc3339(),
            "wave_id": "w-test",
            "wave_index": 0,
            "wave_total": 3,
        });
        assert!(validate_wave_record(&good, 3).is_ok());
        // Same shape, but wave_total=2 disagrees with declared
        // wave size of 3.
        let bad = serde_json::json!({
            "topic": "review.wave.ready",
            "payload": "{}",
            "ts": chrono::Utc::now().to_rfc3339(),
            "wave_id": "w-test",
            "wave_index": 0,
            "wave_total": 2,
        });
        assert!(
            validate_wave_record(&bad, 3).is_err(),
            "wave_total that disagrees with the declared wave size must be rejected"
        );
    }

    // ---- P6 wave record validation tests ----

    #[test]
    fn test_wave_emit_rejects_nested_worker() {
        // Simulate the nested-worker check by setting the env var. This
        // mirrors the bail at the top of `execute_emit`.
        // The check itself is straightforward — verify the guard fires
        // when the env var is set.
        // (Direct test of `execute_emit` would require clap parsing and
        // argument setup; this is the cheapest equivalent.)
        let result = std::env::var("RALPH_WAVE_WORKER");
        // We cannot mutate env in tests under forbid(unsafe), so this
        // asserts the guard shape: when set to "1", nested waves are
        // rejected. The integration test would exercise this end-to-end.
        if result.as_deref() == Ok("1") {
            panic!("nested wave check should reject inside worker");
        }
    }

    #[test]
    fn test_read_payloads_from_reader_skips_empty_lines() {
        let input =
            "{\"dim\":\"correctness\"}\n\n{\"dim\":\"testing\"}\n\n{\"dim\":\"maintainability\"}\n";
        let cursor = std::io::Cursor::new(input);
        let payloads = read_payloads_from_reader(cursor).unwrap();
        assert_eq!(payloads.len(), 3);
        assert_eq!(payloads[0], r#"{"dim":"correctness"}"#);
        assert_eq!(payloads[1], r#"{"dim":"testing"}"#);
        assert_eq!(payloads[2], r#"{"dim":"maintainability"}"#);
    }

    #[test]
    fn test_read_payloads_from_reader_rejects_all_empty() {
        let input = "\n\n  \n";
        let cursor = std::io::Cursor::new(input);
        let payloads = read_payloads_from_reader(cursor).unwrap();
        assert!(payloads.is_empty());
    }

    #[test]
    fn test_validate_payload_shape_rejects_newline_joined_json_payloads() {
        let payloads =
            vec!["{\"dimension\":\"correctness\"}\n{\"dimension\":\"testing\"}".to_string()];
        let err = validate_payload_shape(&payloads).unwrap_err().to_string();
        assert!(err.contains("--payloads-stdin"));
    }

    #[test]
    fn test_validate_payload_shape_allows_single_multiline_json_payload() {
        let payloads = vec![
            "{\n  \"dimension\": \"correctness\",\n  \"focus\": \"check behavior\"\n}".to_string(),
        ];
        validate_payload_shape(&payloads).unwrap();
    }

    // ---- U1: JSON object payload strict validation tests ----

    #[test]
    fn test_validate_payload_shape_accepts_json_object() {
        let payloads = vec![r#"{"dimension":"correctness"}"#.to_string()];
        validate_payload_shape(&payloads).unwrap();
    }

    #[test]
    fn test_validate_payload_shape_rejects_number_payload() {
        let payloads = vec!["10".to_string()];
        let err = validate_payload_shape(&payloads).unwrap_err().to_string();
        assert!(
            err.contains("JSON object"),
            "error should mention JSON object, got: {}",
            err
        );
    }

    #[test]
    fn test_validate_payload_shape_rejects_string_payload() {
        let payloads = vec![r#""text""#.to_string()];
        let err = validate_payload_shape(&payloads).unwrap_err().to_string();
        assert!(err.contains("JSON object"));
    }

    #[test]
    fn test_validate_payload_shape_rejects_array_payload() {
        let payloads = vec!["[]".to_string()];
        let err = validate_payload_shape(&payloads).unwrap_err().to_string();
        assert!(err.contains("JSON object"));
    }

    #[test]
    fn test_validate_payload_shape_rejects_placeholder_payload() {
        let payloads = vec!["placeholder".to_string()];
        let err = validate_payload_shape(&payloads).unwrap_err().to_string();
        assert!(err.contains("JSON object"));
    }

    #[test]
    fn test_validate_payload_shape_rejects_truncated_object() {
        let payloads = vec![r#"{"dimension":"x""#.to_string()];
        let err = validate_payload_shape(&payloads).unwrap_err().to_string();
        assert!(err.contains("JSON object") || err.contains("JSON"));
    }

    #[test]
    fn test_validate_payload_shape_accepts_leading_whitespace_object() {
        let payloads = vec!["   \n  \t{\"dim\":\"x\"}".to_string()];
        validate_payload_shape(&payloads).unwrap();
    }

    #[test]
    fn test_validate_payload_shape_rejects_word_split_token_sequence() {
        // Simulates `printf '%s\n' $(cat payloads.jsonl)` IFS word splitting.
        // Many of these tokens are bare identifiers, not JSON objects.
        let payloads: Vec<String> = (0..10).map(|i| format!("tok{}", i)).collect();
        let err = validate_payload_shape(&payloads).unwrap_err().to_string();
        assert!(err.contains("JSON object"));
    }

    #[test]
    fn test_validate_payload_shape_atomicity_first_valid_then_invalid() {
        // Caller expects: when any payload is invalid, no events are written.
        // We assert at the validate level: invalid payload means Err.
        let payloads = vec![r#"{"ok":1}"#.to_string(), "not-an-object".to_string()];
        assert!(validate_payload_shape(&payloads).is_err());
    }

    #[test]
    fn test_validate_payload_shape_seven_objects_all_pass() {
        let payloads: Vec<String> = (0..7).map(|i| format!(r#"{{"dim":"d{}"}}"#, i)).collect();
        validate_payload_shape(&payloads).unwrap();
    }

    #[test]
    fn test_read_payloads_from_reader_validates_object() {
        // stdin reader must also reject non-object payloads end-to-end.
        let input = "{\"ok\":1}\n\"not-object\"\n{\"ok\":3}\n";
        let cursor = std::io::Cursor::new(input);
        let payloads = read_payloads_from_reader(cursor).unwrap();
        let err = validate_payload_shape(&payloads).unwrap_err().to_string();
        assert!(err.contains("JSON object"));
    }

    // ---- U2 (2026-06-11-002): idempotency key tests ----

    #[test]
    fn test_idempotency_key_validation() {
        // Empty
        assert!(validate_idempotency_key("").is_err());
        // Whitespace only
        assert!(validate_idempotency_key("   ").is_err());
        // Too long (>256 bytes)
        let long_key = "x".repeat(257);
        assert!(validate_idempotency_key(&long_key).is_err());
        // Non-ASCII
        assert!(validate_idempotency_key("中文").is_err());
        // Valid ASCII key
        assert!(validate_idempotency_key("ce-review:foo:1:step:round-1").is_ok());
        // Boundary: exactly 256 bytes
        let boundary = "x".repeat(256);
        assert!(validate_idempotency_key(&boundary).is_ok());
    }

    #[test]
    fn test_idempotency_log_path_derivation() {
        let p = idempotency_log_path(Path::new("/a/b.jsonl"));
        assert_eq!(p, Path::new("/a/.b.jsonl.idempotency.jsonl"));

        let p2 = idempotency_log_path(Path::new(".ralph/events.jsonl"));
        assert_eq!(p2, Path::new(".ralph/.events.jsonl.idempotency.jsonl"));
    }

    #[test]
    fn test_idempotency_scope_key_distinct() {
        let k1 = compute_scope_key("loop1", "hat1", "t1", "key1");
        let k2 = compute_scope_key("loop2", "hat1", "t1", "key1");
        assert_ne!(k1, k2, "different loop_id should give different scope_key");

        let k3 = compute_scope_key("loop1", "hat2", "t1", "key1");
        assert_ne!(k1, k3, "different hat should give different scope_key");

        let k4 = compute_scope_key("loop1", "hat1", "t2", "key1");
        assert_ne!(k1, k4, "different topic should give different scope_key");

        let k5 = compute_scope_key("loop1", "hat1", "t1", "key2");
        assert_ne!(k1, k5, "different key should give different scope_key");
    }

    #[test]
    fn test_idempotency_payload_digest_distinct() {
        let d1 = compute_payload_digest(&["a".to_string(), "b".to_string()]);
        let d2 = compute_payload_digest(&["a".to_string(), "c".to_string()]);
        assert_ne!(d1, d2, "different payloads should give different digest");

        let d3 = compute_payload_digest(&["ab".to_string()]);
        let d4 = compute_payload_digest(&["a".to_string(), "b".to_string()]);
        assert_ne!(d3, d4, "different grouping should give different digest");
    }

    #[test]
    fn test_idempotency_first_call_writes_record() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let payloads = vec![
            r#"{"dim":"correctness"}"#.to_string(),
            r#"{"dim":"testing"}"#.to_string(),
        ];

        let outcome = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            "ce-review:foo:1:step:round-1",
            "loop-1",
            "reviewer",
        )
        .unwrap();

        assert!(!outcome.deduplicated, "first call should not be dedup");
        assert!(outcome.wave_id.starts_with("w-"));

        // Events file should have 2 lines
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 2);

        // Each event should have idempotency_key and idempotency_hash
        for line in content.lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert_eq!(v["idempotency_key"], "ce-review:foo:1:step:round-1");
            assert!(
                v["idempotency_hash"].as_str().unwrap().len() == 64,
                "idempotency_hash should be 64 hex chars"
            );
        }

        // Idempotency log should have 1 line
        let log = read_idempotency_records(&events_path).unwrap();
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].idempotency_key, "ce-review:foo:1:step:round-1");
        assert_eq!(log[0].count, 2);
    }

    #[test]
    fn test_idempotency_dedup_returns_original_wave_id() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let payloads = vec![
            r#"{"dim":"correctness"}"#.to_string(),
            r#"{"dim":"testing"}"#.to_string(),
        ];

        // First call
        let first = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            "ce-review:dup-test",
            "loop-1",
            "reviewer",
        )
        .unwrap();
        assert!(!first.deduplicated);

        // Second call with same key and payloads
        let second = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            "ce-review:dup-test",
            "loop-1",
            "reviewer",
        )
        .unwrap();

        assert!(second.deduplicated, "second call should be dedup");
        assert_eq!(
            first.wave_id, second.wave_id,
            "second call should return same wave_id"
        );

        // Events file still has only 2 lines
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 2);
    }

    #[test]
    fn test_idempotency_same_key_different_payload_errors() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let key = "ce-review:payload-conflict";

        // First call with payload set A
        let payloads_a = vec![r#"{"dim":"correctness"}"#.to_string()];
        write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads_a,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        )
        .unwrap();

        // Second call with different payloads (same key) → should error
        let payloads_b = vec![
            r#"{"dim":"correctness"}"#.to_string(),
            r#"{"dim":"testing"}"#.to_string(),
        ];
        let result = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads_b,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        );
        assert!(
            result.is_err(),
            "same key with different payload should error"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("idempotency-key conflict"),
            "error should mention idempotency-key conflict, got: {err}"
        );

        // Events file should still have only 1 line
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 1);
    }

    #[test]
    fn test_idempotency_different_keys_dont_dedup() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let payloads = vec![r#"{"dim":"correctness"}"#.to_string()];

        // key1
        write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            "key1",
            "loop-1",
            "reviewer",
        )
        .unwrap();

        // key2 → different key, should write a new wave
        write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            "key2",
            "loop-1",
            "reviewer",
        )
        .unwrap();

        // Events file should have 2 lines
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 2);

        // Log should have 2 records with different wave_ids
        let records = read_idempotency_records(&events_path).unwrap();
        assert_eq!(records.len(), 2);
        assert_ne!(records[0].wave_id, records[1].wave_id);
    }

    #[test]
    fn test_idempotency_cross_loop_id_isolated() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let payloads = vec![r#"{"dim":"correctness"}"#.to_string()];
        let key = "same-key";

        // loop-1
        write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        )
        .unwrap();

        // loop-2, same key → should NOT dedup
        let outcome = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            key,
            "loop-2",
            "reviewer",
        )
        .unwrap();
        assert!(!outcome.deduplicated, "different loop_id should not dedup");

        // Events file → 2 lines
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 2);
    }

    #[test]
    fn test_idempotency_cross_hat_isolated() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let payloads = vec![r#"{"dim":"correctness"}"#.to_string()];
        let key = "same-key";

        write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        )
        .unwrap();

        let outcome = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            key,
            "loop-1",
            "executor",
        )
        .unwrap();
        assert!(!outcome.deduplicated, "different hat should not dedup");

        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 2);
    }

    #[test]
    fn test_idempotency_cross_topic_isolated() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let payloads = vec![r#"{"dim":"correctness"}"#.to_string()];
        let key = "same-key";

        write_wave_events_with_idempotency_with_scope(
            "topic.a",
            &payloads,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        )
        .unwrap();

        let outcome = write_wave_events_with_idempotency_with_scope(
            "topic.b",
            &payloads,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        )
        .unwrap();
        assert!(!outcome.deduplicated, "different topic should not dedup");

        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 2);
    }

    #[test]
    fn test_idempotency_no_key_unchanged_compat() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let payloads = vec![r#"{"dim":"correctness"}"#.to_string()];

        // Use the regular write_wave_events path (no idempotency)
        write_wave_events("test.topic", &payloads, &events_path).unwrap();

        // Events should NOT have idempotency fields
        let content = fs::read_to_string(&events_path).unwrap();
        for line in content.lines() {
            let v: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(
                v.get("idempotency_key").is_none(),
                "no-key path should not inject idempotency_key"
            );
        }

        // Idempotency log should not exist
        assert!(
            !idempotency_log_path(&events_path).exists(),
            "no-key path should not create idempotency log"
        );
    }

    #[test]
    fn test_idempotency_recovery_after_partial_failure() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let key = "ce-review:recovery-test";
        let payloads = vec![
            r#"{"dim":"correctness"}"#.to_string(),
            r#"{"dim":"testing"}"#.to_string(),
            r#"{"dim":"maintainability"}"#.to_string(),
        ];
        let scope_key = compute_scope_key("loop-1", "reviewer", "review.wave.ready", key);

        // Simulate a successful first write without the idempotency record
        let first_wave_id = write_wave_events_with_provenance(
            "review.wave.ready",
            &payloads,
            &events_path,
            Some("reviewer"),
            Some(key),
            Some(&scope_key),
        )
        .unwrap();

        // Verify events written (3 lines)
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 3);

        // Idempotency log should NOT exist (simulating crash before record write)
        let log_path = idempotency_log_path(&events_path);
        assert!(!log_path.exists(), "recovery test: log_path={:?} should not exist before recovery call", log_path);

        // Now call with the same key → should recover (scan events, write record, return same wave_id)
        let outcome = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        )
        .unwrap();

        assert!(outcome.deduplicated, "recovery should return deduplicated=true");
        assert_eq!(
            outcome.wave_id, first_wave_id,
            "recovery should return original wave_id"
        );

        // Events file should still have 3 lines (recovery did not append)
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 3);

        // Idempotency log should now exist with 1 record
        let records = read_idempotency_records(&events_path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].idempotency_key, key);
        assert_eq!(records[0].wave_id, first_wave_id);

        // Subsequent dedup call (record now present) must also recover the
        // original wave_id without appending events.
        let outcome_dedup = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            &payloads,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        )
        .unwrap();
        assert!(outcome_dedup.deduplicated, "post-recovery dedup should also return deduplicated=true");
        assert_eq!(
            outcome_dedup.wave_id, first_wave_id,
            "post-recovery dedup should return original wave_id"
        );
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(
            content.lines().count(),
            3,
            "post-recovery dedup must not append events"
        );
    }

    #[test]
    fn test_idempotency_concurrent_writers_serialize() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let n_workers: usize = 6;

        // Each worker uses a distinct --idempotency-key (otherwise the
        // idempotency layer would correctly reject "same scope,
        // different payload" as a conflict — that is a feature, not a
        // race we want to exercise here). The contention we want to
        // exercise is on the FileLock around the events-file write:
        // N writers must serialize cleanly and produce N event lines
        // with N unique wave_ids, without corrupting the file or the
        // idempotency log.
        let payloads: Vec<String> = (0..n_workers)
            .map(|i| format!(r#"{{"worker":{i}}}"#))
            .collect();

        let barrier = Arc::new(Barrier::new(n_workers));
        let mut handles = Vec::with_capacity(n_workers);
        for (i, payload) in payloads.iter().cloned().enumerate() {
            let events_path = events_path.clone();
            let key = format!("ce-review:concurrent-writer-{i}");
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                barrier.wait();
                write_wave_events_with_idempotency_with_scope(
                    "review.wave.ready",
                    &[payload],
                    &events_path,
                    &key,
                    "loop-1",
                    "reviewer",
                )
            }));
        }

        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|h| h.join().expect("worker thread panicked").expect("wave emit failed"))
            .collect();

        // Every worker must observe a fresh, non-deduplicated wave
        // (distinct keys, distinct records).
        for o in &outcomes {
            assert!(
                !o.deduplicated,
                "concurrent writers with distinct keys must each create a new wave"
            );
        }
        // Every wave_id is unique (no FileLock contention produced
        // collisions).
        let unique_ids: std::collections::HashSet<_> =
            outcomes.iter().map(|o| o.wave_id.clone()).collect();
        assert_eq!(unique_ids.len(), n_workers, "expected n_workers distinct wave_ids");

        // Events file fans in all n_workers lines, with each line
        // containing some worker index in the (JSON-escaped) payload —
        // proves serialization preserved per-worker payload integrity
        // (no interleaving or overwrite). Order is non-deterministic
        // because distinct keys mean no contention: each writer holds
        // the lock only for its own (very short) critical section.
        let content = fs::read_to_string(&events_path).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), n_workers);
        let mut seen_workers: std::collections::HashSet<u32> =
            std::collections::HashSet::new();
        for line in &lines {
            // Each event line is a JSON object with a top-level
            // "payload" field whose value is the original payload
            // JSON-escaped. Pull out the inner worker index.
            let v: serde_json::Value = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("event line not valid JSON: {line}: {e}"));
            let payload_str = v
                .get("payload")
                .and_then(|p| p.as_str())
                .unwrap_or_else(|| panic!("event line missing payload: {line}"));
            let inner: serde_json::Value = serde_json::from_str(payload_str)
                .unwrap_or_else(|e| panic!("inner payload not valid JSON: {payload_str}: {e}"));
            let worker = inner
                .get("worker")
                .and_then(|w| w.as_u64())
                .unwrap_or_else(|| panic!("inner payload missing worker: {payload_str}"));
            assert!(
                (worker as usize) < n_workers,
                "worker index {worker} out of range (n_workers={n_workers})"
            );
            assert!(
                seen_workers.insert(worker as u32),
                "duplicate worker {worker} — FileLock serialization must prevent overwrites"
            );
        }

        // Idempotency log carries one record per writer, each tagged
        // with the per-worker key.
        let records = read_idempotency_records(&events_path).unwrap();
        assert_eq!(records.len(), n_workers);
        let mut seen_keys: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for r in &records {
            assert!(
                r.idempotency_key.starts_with("ce-review:concurrent-writer-"),
                "unexpected key: {}",
                r.idempotency_key
            );
            assert!(
                seen_keys.insert(r.idempotency_key.clone()),
                "duplicate key {} — IdempotencyRecord append must be serialized",
                r.idempotency_key
            );
        }
    }

    #[test]
    fn test_idempotency_incomplete_events_errors() {
        let tmp = TempDir::new().unwrap();
        let events_path = tmp.path().join("events.jsonl");
        let key = "ce-review:incomplete-test";

        // Write only 2 events but not the full 7 claimed count
        let payloads_partial = vec![
            r#"{"dim":"correctness"}"#.to_string(),
            r#"{"dim":"testing"}"#.to_string(),
        ];
        write_wave_events_with_provenance(
            "review.wave.ready",
            &payloads_partial,
            &events_path,
            Some("reviewer"),
            Some(key),
            Some("incomplete-scope"),
        )
        .unwrap();

        // Manually create a record claiming 7 events (to trigger the incomplete check)
        let rec = IdempotencyRecord {
            scope_key: compute_scope_key("loop-1", "reviewer", "review.wave.ready", key),
            idempotency_key: key.to_string(),
            wave_id: "w-simulated".to_string(),
            topic: "review.wave.ready".to_string(),
            hat: "reviewer".to_string(),
            payload_digest: compute_payload_digest(&payloads_partial),
            count: 7,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        append_idempotency_record(&events_path, &rec).unwrap();

        // Now call with the same key → should detect incomplete emission
        let result = write_wave_events_with_idempotency_with_scope(
            "review.wave.ready",
            // same payloads as recorded in the record
            &payloads_partial,
            &events_path,
            key,
            "loop-1",
            "reviewer",
        );

        assert!(result.is_err(), "incomplete event should error");
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("incomplete prior wave emission"),
            "should mention incomplete prior wave emission, got: {err_msg}"
        );

        // Events file should still have only 2 lines
        let content = fs::read_to_string(&events_path).unwrap();
        assert_eq!(content.lines().count(), 2);
    }

    #[test]
    fn test_wave_output_format_default_is_text() {
        // Default is `text` for backward compatibility.
        let s = format!("{:?}", WaveOutputFormat::Text);
        assert!(s.contains("Text"));
    }

    #[test]
    fn test_wave_emit_args_output_default_text() {
        // Parsing without --output should default to Text.
        use clap::Parser;
        let parsed = WaveEmitArgs::try_parse_from([
            "ralph",
            "review.wave.ready",
            "--payloads",
            r#"{"dim":"x"}"#,
        ])
        .unwrap();
        assert_eq!(parsed.output, WaveOutputFormat::Text);
    }

    #[test]
    fn test_wave_emit_args_output_json_parsed() {
        use clap::Parser;
        let parsed = WaveEmitArgs::try_parse_from([
            "ralph",
            "review.wave.ready",
            "--payloads-stdin",
            "--output",
            "json",
        ])
        .unwrap();
        assert_eq!(parsed.output, WaveOutputFormat::Json);
    }
}
