//! Integration tests for the project's top-level `ralph.yml` config.
//!
//! These tests pin the contract that `dimension-reviewer` is bound to the
//! `claude` backend (not `pi`), matching the active `builtin:ce-executor-serial`
//! preset. See plan
//! `docs/plans/2026-06-17-004-fix-ce-executor-serial-recovery-and-reviewer-scope-plan.md`
//! (U3 / R4).
//!
//! The project `ralph.yml` is loaded as a partial overlay over a builtin
//! preset, so its `hats:` block intentionally omits fields like `name` (those
//! come from the preset). We therefore parse it as a YAML `Value` tree and
//! assert against the relevant scalar instead of trying to deserialize the
//! full `RalphConfig` (which would require a complete `HatConfig`).

use ralph_core::config::HatBackend;
use std::path::PathBuf;

/// Resolve the workspace root (the directory that contains `Cargo.toml` at
/// the top level and `ralph.yml` next to it).
fn workspace_root() -> PathBuf {
    // `CARGO_MANIFEST_DIR` points at `crates/ralph-cli`; the workspace root is
    // two levels up.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .ancestors()
        .nth(2)
        .expect("CARGO_MANIFEST_DIR has at least two ancestors")
        .to_path_buf()
}

fn load_project_yaml() -> serde_yaml::Value {
    let path = workspace_root().join("ralph.yml");
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_yaml::from_str(&raw).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

#[test]
fn ralph_yml_dimension_reviewer_backend_is_claude() {
    let yaml = load_project_yaml();

    let backend_value = yaml
        .get("hats")
        .and_then(|h| h.get("dimension-reviewer"))
        .and_then(|h| h.get("backend"))
        .unwrap_or_else(|| {
            panic!("ralph.yml must override `hats.dimension-reviewer.backend`; yaml = {yaml:#?}")
        });

    // Either a plain string ("claude") or a structured mapping
    // (e.g. `{type: claude, args: [...]}`).
    let backend: HatBackend = serde_yaml::from_value(backend_value.clone())
        .expect("dimension-reviewer.backend must deserialize as HatBackend");

    assert_eq!(
        backend.to_cli_backend(),
        "claude",
        "dimension-reviewer must be bound to `claude` (was `pi` per 2026-06-17-004 U3 / R4)"
    );
}

#[test]
fn ralph_yml_no_hat_uses_pi_backend() {
    // Regression: only `review-synthesizer` is allowed to opt into the
    // `pi` backend (added in `f146e621` for `builtin:ce-executor-isolated`).
    // Every other hat must use a non-`pi` backend. The
    // `dimension-reviewer` case is also pinned by
    // `ralph_yml_dimension_reviewer_backend_is_claude`; this test is the
    // broader net for any *other* hat that tries to follow suit.
    let yaml = load_project_yaml();

    let hats = yaml
        .get("hats")
        .and_then(|h| h.as_mapping())
        .expect("ralph.yml must have a `hats:` mapping");

    for (hat_id, hat_cfg) in hats {
        let Some(backend_value) = hat_cfg.get("backend") else {
            continue;
        };
        let backend: HatBackend = serde_yaml::from_value(backend_value.clone())
            .expect("per-hat backend must deserialize as HatBackend");
        let name = backend.to_cli_backend();
        let hat_id = hat_id
            .as_str()
            .expect("hat id under `hats:` must be string");

        if hat_id == "review-synthesizer" {
            assert_eq!(
                name, "pi",
                "review-synthesizer is the only hat allowed to use `pi` \
                 (see f146e621 / builtin:ce-executor-isolated)"
            );
            continue;
        }

        assert_ne!(
            name, "pi",
            "hat `{hat_id}` must not use the `pi` backend; \
             only `review-synthesizer` is exempted \
             (dimension-reviewer is pinned to `claude` per 2026-06-17-004 U3 / R4)"
        );
    }
}

#[test]
fn ralph_yml_default_cli_backend_is_claude() {
    // Sanity: even if the per-hat override were dropped, the inherited
    // `cli.backend` is `claude`, so the regression is closed two ways.
    let yaml = load_project_yaml();
    let backend = yaml
        .get("cli")
        .and_then(|c| c.get("backend"))
        .and_then(|b| b.as_str())
        .expect("ralph.yml must set `cli.backend`");
    assert_eq!(backend, "claude");
}
