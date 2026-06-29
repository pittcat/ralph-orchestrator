//! Legacy task loop_id backfill (U3).
//!
//! Why this exists: the 2026-06-26 incident left multiple
//! `tasks.jsonl` records with `loop_id` set to `null` (or empty
//! string) because the pre-mechanism-foundation runtime emitted
//! `work.ready` / `work.done` for tasks created before
//! `loop_scoped: true` was enforced. The execution contract then
//! rejected the completion events with
//! `TaskWrongLoop { actual_loop: None }` and the loop spun
//! forever in `stall_recovery_counts`.
//!
//! This module reads the JSONL file, writes the `loop_id` field
//! for every record that lacks one, and persists the file
//! atomically (write to temp + rename). Subsequent calls are
//! idempotent — the second invocation reports zero backfills and
//! leaves the file untouched.
//!
//! Cross-platform / concurrency semantics: relies on
//! `std::fs::rename` for the atomic swap. On macOS and Linux this
//! is an atomic POSIX rename. On Windows the rename is not
//! atomic; the caller is responsible for any inter-process
//! coordination. (U4 introduces a `IdempotentLog` writer for the
//! higher-priority concurrent-final guarantee.)
//!
//! # Example
//!
//! ```no_run
//! use std::path::Path;
//! use ralph_core::event_loop::legacy_task_relocate::relocate_legacy_tasks;
//!
//! let path = Path::new(".ralph/agent/tasks.jsonl");
//! let backfilled = relocate_legacy_tasks(path, "loop-abc-123").unwrap();
//! assert!(backfilled <= /* file line count */ 0);
//! ```

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use serde_json::Value;
use thiserror::Error;

/// Errors that `relocate_legacy_tasks` may surface to the caller.
#[derive(Debug, Error)]
pub enum RelocateError {
    /// I/O failure while reading or writing the tasks file.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// A line in the JSONL file is not valid JSON.
    #[error("malformed JSON on line {line}: {source}")]
    MalformedJson {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
}

/// Backfill `loop_id` on every record in `tasks_path` whose
/// `loop_id` is missing or empty.
///
/// Returns the number of records that were rewritten. The file
/// is rewritten in place (via a sibling `.tmp` file and a
/// `rename`) so partial writes never corrupt the on-disk ledger.
pub fn relocate_legacy_tasks(
    tasks_path: &Path,
    current_loop_id: &str,
) -> Result<usize, RelocateError> {
    if !tasks_path.exists() {
        return Err(RelocateError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            format!("tasks file does not exist: {}", tasks_path.display()),
        )));
    }

    let content = fs::read_to_string(tasks_path)?;
    let mut rewritten: Vec<String> = Vec::with_capacity(content.lines().count());
    let mut backfilled = 0usize;

    for (idx, line) in content.lines().enumerate() {
        if line.trim().is_empty() {
            rewritten.push(line.to_string());
            continue;
        }

        let mut value: Value =
            serde_json::from_str(line).map_err(|e| RelocateError::MalformedJson {
                line: idx + 1,
                source: e,
            })?;

        let needs_fill = match value.get("loop_id") {
            None => true,
            Some(Value::Null) => true,
            Some(Value::String(s)) => s.is_empty(),
            // A non-string loop_id (e.g. accidentally numeric) is
            // treated as "needs fill" — we replace it rather than
            // touch a non-string value in place, which avoids
            // surprising downstream consumers.
            Some(_) => true,
        };

        if needs_fill {
            if let Some(obj) = value.as_object_mut() {
                obj.insert(
                    "loop_id".to_string(),
                    Value::String(current_loop_id.to_string()),
                );
            }
            rewritten.push(serde_json::to_string(&value).map_err(|e| {
                RelocateError::Io(io::Error::other(format!(
                    "failed to serialise line {}: {e}",
                    idx + 1
                )))
            })?);
            backfilled += 1;
        } else {
            rewritten.push(line.to_string());
        }
    }

    if backfilled == 0 {
        // Nothing to do; preserve the original byte-for-byte.
        return Ok(0);
    }

    let tmp_path = tasks_path.with_extension("jsonl.tmp");
    {
        let mut f = fs::File::create(&tmp_path)?;
        for line in &rewritten {
            writeln!(f, "{line}")?;
        }
        f.sync_all()?;
    }
    fs::rename(&tmp_path, tasks_path)?;

    Ok(backfilled)
}

#[cfg(test)]
mod tests;
