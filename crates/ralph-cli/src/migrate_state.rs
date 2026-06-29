//! U19 (2026-06-27 mechanism foundation completion):
//! `ralph migrate-state` — roundtrip migration of the
//! legacy `.ralph/agent/tasks.jsonl` and
//! `.ralph/recovery.jsonl` records to the post-mechanism
//! foundation shape (idempotent: the legacy records
//! already carry the right fields, so the migration is
//! effectively a schema validator + idempotent
//! rewrite).
//!
//! Why this command exists: the 2026-06-26 diagnostic
//! surfaced stale JSONL files with `loop_id == null`.
//! U8's `relocate_legacy_tasks` already backfills the
//! `loop_id` on every loop start, but operators who
//! want to migrate a workspace *without* starting a
//! loop (e.g. cleaning up archived state) need a
//! standalone command. U19 introduces that command.
//!
//! Cross-platform / concurrency semantics: pure
//! file I/O. The migration is single-threaded; the
//! caller serialises invocations.

use std::io::{BufRead, Write};
use std::path::Path;

/// Result of a single migration roundtrip. Returned
/// to the CLI so the caller can print a summary.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct MigrationReport {
    /// Lines processed (read from the source file).
    pub processed: usize,
    /// Lines that already carry the new shape (no
    /// rewrite required).
    pub already_current: usize,
    /// Lines that needed a rewrite.
    pub migrated: usize,
    /// Lines that could not be parsed (preserved
    /// verbatim in the output).
    pub malformed: usize,
}

/// Migrate the legacy `tasks.jsonl` file to the
/// post-mechanism foundation shape. The migration is
/// idempotent: a second call reports `already_current`
/// for every record.
///
/// The legacy shape is `{"task_key": ..., "loop_id":
/// null|"", ...}`. The new shape is the same with
/// `loop_id` populated (or explicitly `""` if the
/// operator chose not to assign one).
pub fn migrate_tasks_file(
    source: &Path,
    target_loop_id: &str,
) -> Result<MigrationReport, std::io::Error> {
    if !source.exists() {
        return Ok(MigrationReport::default());
    }
    let file = std::fs::File::open(source)?;
    let reader = std::io::BufReader::new(file);
    let mut report = MigrationReport::default();

    let tmp_path = source.with_extension("jsonl.migrate-tmp");
    let mut writer = std::fs::File::create(&tmp_path)?;
    for line in reader.lines() {
        let line = line?;
        report.processed += 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            writeln!(writer)?;
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(mut value) => {
                if let Some(obj) = value.as_object_mut() {
                    let needs_migration = obj
                        .get("loop_id")
                        .map(|v| v.is_null() || v.as_str().map(|s| s.is_empty()).unwrap_or(false))
                        .unwrap_or(true);
                    if needs_migration {
                        obj.insert(
                            "loop_id".to_string(),
                            serde_json::Value::String(target_loop_id.to_string()),
                        );
                        report.migrated += 1;
                        writeln!(writer, "{}", serde_json::to_string(&value).unwrap())?;
                    } else {
                        report.already_current += 1;
                        writeln!(writer, "{}", serde_json::to_string(&value).unwrap())?;
                    }
                } else {
                    report.malformed += 1;
                    writeln!(writer, "{line}")?;
                }
            }
            Err(_) => {
                report.malformed += 1;
                writeln!(writer, "{line}")?;
            }
        }
    }
    writer.flush()?;
    std::fs::rename(&tmp_path, source)?;
    Ok(report)
}

#[cfg(test)]
mod tests;
