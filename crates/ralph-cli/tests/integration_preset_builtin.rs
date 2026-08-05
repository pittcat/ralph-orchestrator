//! Integration tests for `ralph preset builtin list` (U01 of
//! `2026-08-05-001-feat-builtin-preset-introspection`).
//!
//! Closes the runtime/introspection gap: previously, the only way to
//! enumerate builtin presets compiled into the `ralph` binary was to
//! read `presets/` from a source checkout. U01 adds a stable,
//! read-only `ralph preset builtin list --format human|json` command
//! that surfaces the public-only inventory derived from
//! `EmbeddedPreset`, leaving the legacy `ralph preset list/show`
//! TemplateCatalog path untouched (S12 regression guard).
//!
//! Given/When/Then coverage (BDD-style behavior, real binary):
//! - S1 JSON envelope: `presets` array of {id, source, description,
//!   public}; `source` strictly equals `builtin:<id>`; `merge-loop`
//!   hidden; `parallel-forge` public; no workspace side-effects.
//! - S2 human format: stdout contains builtin ID and public flag.
//! - S12 old `ralph preset list --format json` continues to return the
//!   TemplateCatalog manifest array.
//! - Subcommand registration: `ralph preset --help` mentions `builtin`.

mod common;

use std::path::Path;
use std::process::Output;
use tempfile::TempDir;

fn run(cwd: &Path, args: &[&str]) -> Output {
    common::ralph_bin()
        .args(["--color", "never"])
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to spawn ralph preset builtin")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn assert_success(out: &Output) {
    assert!(
        out.status.success(),
        "expected success\nstdout:\n{}\nstderr:\n{}",
        stdout(out),
        stderr(out)
    );
}

// ── S1: JSON envelope ───────────────────────────────────────────────────────

#[test]
fn builtin_list_json_contains_public_only() {
    let tmp = TempDir::new().unwrap();
    let out = run(
        tmp.path(),
        &["preset", "builtin", "list", "--format", "json"],
    );
    assert_success(&out);

    let parsed: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("stdout is valid JSON");

    let arr = parsed
        .get("presets")
        .and_then(|v| v.as_array())
        .expect("top-level `presets` array");

    // Every item must have id, source, description, public.
    for item in arr {
        let obj = item.as_object().expect("each preset is an object");
        assert!(obj.contains_key("id"), "missing `id`: {:?}", obj);
        assert!(obj.contains_key("source"), "missing `source`: {:?}", obj);
        assert!(
            obj.contains_key("description"),
            "missing `description`: {:?}",
            obj
        );
        assert!(obj.contains_key("public"), "missing `public`: {:?}", obj);

        // `public` must be true.
        assert_eq!(
            obj["public"].as_bool(),
            Some(true),
            "non-public leaked into list: {:?}",
            obj
        );

        // `source` strictly equals `builtin:<id>`.
        let id = obj["id"].as_str().expect("id is string");
        let source = obj["source"].as_str().expect("source is string");
        assert_eq!(
            source,
            &format!("builtin:{id}"),
            "source must derive exactly from id"
        );
    }

    // `merge-loop` (hidden) MUST NOT appear.
    let ids: Vec<&str> = arr
        .iter()
        .filter_map(|v| v.get("id").and_then(|i| i.as_str()))
        .collect();
    assert!(
        !ids.contains(&"merge-loop"),
        "merge-loop (public=false) must not appear in `builtin list`"
    );

    // `parallel-forge` (public) MUST appear.
    assert!(
        ids.contains(&"parallel-forge"),
        "parallel-forge (public) must appear: {ids:?}"
    );

    // No workspace side effects.
    assert!(
        !tmp.path().join(".ralph").exists(),
        "builtin list must not create .ralph/"
    );
}

#[test]
fn builtin_list_json_workspace_has_no_new_files() {
    // Char-test guard: snapshot the workspace before and after.
    let tmp = TempDir::new().unwrap();
    let before = walkdir_snapshot(tmp.path());
    let out = run(
        tmp.path(),
        &["preset", "builtin", "list", "--format", "json"],
    );
    assert_success(&out);
    let after = walkdir_snapshot(tmp.path());
    assert_eq!(
        before, after,
        "builtin list must not touch the workspace filesystem"
    );
}

fn walkdir_snapshot(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_string_lossy().into_owned());
        }
        if path.is_dir() {
            walk(root, &path, out);
        }
    }
}

// ── S2: human format ────────────────────────────────────────────────────────

#[test]
fn builtin_list_human_names_source_and_visibility() {
    let tmp = TempDir::new().unwrap();
    let out = run(
        tmp.path(),
        &["preset", "builtin", "list", "--format", "human"],
    );
    assert_success(&out);
    let text = stdout(&out);

    assert!(
        text.contains("parallel-forge"),
        "human list must mention parallel-forge:\n{text}"
    );
    assert!(
        text.contains("public"),
        "human list must surface a `public` visibility label:\n{text}"
    );
}

// ── S12: old template path is unaffected ─────────────────────────────────────

#[test]
fn template_commands_remain_template_only() {
    let tmp = TempDir::new().unwrap();
    let out = run(tmp.path(), &["preset", "list", "--format", "json"]);
    assert_success(&out);

    let parsed: serde_json::Value =
        serde_json::from_str(&stdout(&out)).expect("template list is valid JSON");

    // The OLD path returns a JSON array (not an envelope object) of
    // TemplateManifest records. We don't depend on the exact contents
    // here — the regression guard is the SHAPE: the old call site
    // must not silently switch to the new envelope shape.
    assert!(
        parsed.is_array(),
        "old `preset list` must keep returning a JSON array of manifests, got: {parsed}"
    );
    let arr = parsed.as_array().unwrap();
    assert!(!arr.is_empty(), "template list must not be empty");
    for item in arr {
        let obj = item.as_object().expect("each manifest is an object");
        // The TemplateManifest shape (template + placeholders + ...) is
        // distinct from the builtin envelope (id + source + public).
        // The strongest invariants are the SHAPE DIFFERENCES:
        //   - TemplateManifest uses `name`, not `id`.
        //   - TemplateManifest does NOT carry a `public` flag.
        assert!(
            obj.contains_key("name"),
            "manifest must keep its template `name` shape: {obj:?}"
        );
        assert!(
            !obj.contains_key("id"),
            "builtin `id` field must not leak into the template list shape"
        );
        assert!(
            !obj.contains_key("public"),
            "builtin `public` field must not leak into the template list shape"
        );
    }
}

// ── Subcommand registration guard ────────────────────────────────────────────

#[test]
fn preset_help_lists_builtin_namespace() {
    let out = common::ralph_bin()
        .args(["--color", "never", "preset", "--help"])
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    assert_success(&out);
    let text = stdout(&out);
    assert!(
        text.contains("builtin"),
        "preset --help missing `builtin` namespace:\n{text}"
    );
}
