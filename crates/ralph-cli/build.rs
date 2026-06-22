// build.rs for ralph-cli
//
// Copies canonical preset yml files (the single source of truth under
// `presets/en/` in the repo root) into Cargo's $OUT_DIR at compile time,
// per the explicit allow-list in `presets/manifest.yml`. The Rust code
// embeds the copies via:
//
//     include_str!(concat!(env!("OUT_DIR"), "/presets/<name>.yml"))
//
// Why a manifest: `include_str!` content is baked into the binary, so
// the file must exist on the build host's filesystem during compilation,
// but the source location is otherwise free. That lets us delete the
// crate-internal mirror directory entirely. A manifest in addition
// keeps the embedded set auditable: adding a new preset requires an
// explicit opt-in in `presets/manifest.yml`, and a new entry in
// `crates/ralph-cli/src/presets.rs` (the two sides cross-check each
// other at compile time).
//
// Authoring rules live at the top of `presets/manifest.yml`.
//
// Schema SSOT merge (plan 2026-06-16-002 Unit 1):
// For every preset listed in the manifest, this script ALSO consults
// `presets/schemas/<name>.yml` if present. That file is the authoring
// single source of truth (SSOT) for `event_policy.schemas`. The script
// deep-merges the SSOT `schemas` block into the preset YAML's
// `event_loop.event_policy.schemas` block, with SSOT providing the base
// and any inline `schemas` block in the preset acting as a per-key
// override layer (kept during the transition so a curated override can
// hotfix a topic without round-tripping through SSOT). The merged YAML
// is what the binary actually embeds — the four consumer chains
// (CLI precheck, loop gate, drift engine, prompt builder) read the
// embedded copy and therefore see one canonical schema set. When the
// `presets/schemas/` file is absent, the merge is skipped and the
// preset YAML is copied verbatim (preserves the historical behaviour
// for presets that have no SSOT file).
//
// Directories that are NEVER read by this script:
//   * `presets/zh/`         — Chinese reference copies, not embedded.
//   * `presets/extras/`     — Orphan / demo files, not embedded.
//   * `presets/minimal/`    — Used by the smoke-test fixture loader,
//                              read from the canonical path at runtime.

use std::fs;
use std::path::PathBuf;

fn main() {
    let manifest_dir = match std::env::var("CARGO_MANIFEST_DIR") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("build.rs: CARGO_MANIFEST_DIR not set; skipping preset copy");
            return;
        }
    };
    let out_dir = match std::env::var("OUT_DIR") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("build.rs: OUT_DIR not set; skipping preset copy");
            return;
        }
    };

    let manifest_path = PathBuf::from(&manifest_dir)
        .join("..")
        .join("..")
        .join("presets")
        .join("manifest.yml");
    let en_dir = PathBuf::from(&manifest_dir)
        .join("..")
        .join("..")
        .join("presets")
        .join("en");
    let schemas_dir = PathBuf::from(&manifest_dir)
        .join("..")
        .join("..")
        .join("presets")
        .join("schemas");
    let dest = PathBuf::from(&out_dir).join("presets");

    println!("cargo:rerun-if-changed={}", manifest_path.display());
    // Rerun when any schemas/ file changes: a new SSOT file should
    // re-trigger the merge without the operator having to touch a
    // preset or the manifest. The directory itself is also rerun-if-
    // changed so adding new files is picked up on the next build.
    if schemas_dir.is_dir() {
        println!("cargo:rerun-if-changed={}", schemas_dir.display());
    }

    let manifest_text = match fs::read_to_string(&manifest_path) {
        Ok(t) => t,
        Err(e) => {
            eprintln!(
                "build.rs: failed to read manifest at {}: {}; skipping preset copy. \
                 This is expected when building from a crates.io tarball — the embedded \
                 binary falls back to runtime path resolution in that case.",
                manifest_path.display(),
                e
            );
            return;
        }
    };

    let names: Vec<String> = match parse_embedded_names(&manifest_text) {
        Ok(v) => v,
        Err(e) => panic!(
            "build.rs: failed to parse `presets/manifest.yml` ({}). \
             Expected a top-level `embedded:` key whose value is a YAML list of \
             preset basenames (no `.yml` extension).",
            e
        ),
    };

    if !en_dir.is_dir() {
        eprintln!(
            "build.rs: canonical preset dir not found at {}; skipping preset copy. \
             This is expected when building from a crates.io tarball — the embedded \
             binary falls back to runtime path resolution in that case.",
            en_dir.display()
        );
        return;
    }

    fs::create_dir_all(&dest).expect("failed to create $OUT_DIR/presets");

    let mut copied = 0usize;
    for name in &names {
        if name.is_empty() {
            panic!("build.rs: manifest contains an empty preset name");
        }
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            panic!(
                "build.rs: manifest preset name `{}` is not a bare basename",
                name
            );
        }
        let src = en_dir.join(format!("{}.yml", name));
        let dest_file = dest.join(format!("{}.yml", name));
        if !src.is_file() {
            panic!(
                "build.rs: manifest lists `{}` but presets/en/{}.yml does not exist \
                 (looked at {}). Add the file or remove the name from manifest.yml.",
                name,
                name,
                src.display()
            );
        }
        println!("cargo:rerun-if-changed={}", src.display());

        // Schema SSOT merge (plan 2026-06-16-002 Unit 1). When a
        // matching `presets/schemas/<name>.yml` exists, deep-merge its
        // top-level `schemas` mapping into `event_loop.event_policy.schemas`
        // (SSOT base, inline override). Without SSOT, copy verbatim —
        // preserves the historical behaviour for presets that have no
        // SSOT file yet.
        let ssot_path = schemas_dir.join(format!("{}.yml", name));
        let merged_text = if ssot_path.is_file() {
            println!("cargo:rerun-if-changed={}", ssot_path.display());
            let preset_text = fs::read_to_string(&src).unwrap_or_else(|e| {
                panic!("build.rs: failed to read preset {}: {}", src.display(), e)
            });
            let ssot_text = fs::read_to_string(&ssot_path).unwrap_or_else(|e| {
                panic!(
                    "build.rs: failed to read schema SSOT {}: {}",
                    ssot_path.display(),
                    e
                )
            });
            match merge_preset_with_schema(&preset_text, &ssot_text, name) {
                Ok(s) => s,
                Err(e) => panic!(
                    "build.rs: failed to merge schema SSOT into preset `{}`: {} \
                     (preset={}, ssot={})",
                    name,
                    e,
                    src.display(),
                    ssot_path.display()
                ),
            }
        } else {
            // No SSOT file: fall back to byte-identical copy. This
            // keeps every preset that has no `presets/schemas/<n>.yml`
            // working without any change in behaviour.
            match fs::read_to_string(&src) {
                Ok(s) => s,
                Err(e) => panic!("build.rs: failed to read preset {}: {}", src.display(), e),
            }
        };

        if let Err(e) = fs::write(&dest_file, merged_text.as_bytes()) {
            panic!("build.rs: failed to write {}: {}", dest_file.display(), e);
        }
        copied += 1;
    }

    eprintln!(
        "build.rs: copied {} preset yml from manifest ({} declared)",
        copied,
        names.len()
    );
}

/// Minimal `presets/manifest.yml` parser.
///
/// We avoid pulling in a YAML dependency in build.rs. The manifest format
/// is a small, stable subset:
///
/// ```yaml
/// # comments are allowed
/// embedded:
///   - name1
///   - name2
/// ```
///
/// Lines outside the `embedded:` block are ignored. Indentation must
/// match the example (two spaces under `embedded:`).
fn parse_embedded_names(text: &str) -> Result<Vec<String>, String> {
    let mut names: Vec<String> = Vec::new();
    let mut in_embedded = false;
    let mut found_embedded_key = false;

    for (idx, raw_line) in text.lines().enumerate() {
        let line = raw_line.split('#').next().unwrap_or("").trim_end();
        if line.is_empty() {
            continue;
        }
        if !in_embedded {
            // Look for "embedded:" at the start of a line (allowing leading spaces).
            let stripped = line.trim_start();
            if stripped.starts_with("embedded:") {
                found_embedded_key = true;
                in_embedded = true;
                // Allow `embedded: [a, b, c]` flow-style too.
                let rest = stripped["embedded:".len()..].trim();
                if let Some(inner) = rest.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                    for n in inner.split(',') {
                        let n = n.trim().trim_matches('"').trim_matches('\'');
                        if !n.is_empty() {
                            names.push(n.to_string());
                        }
                    }
                    in_embedded = false;
                }
                continue;
            }
            // Any other top-level key terminates the search.
            if !line.starts_with(' ') && !line.starts_with('\t') && line.contains(':') {
                return Err(format!("line {}: unexpected key `{}`", idx + 1, line));
            }
        } else {
            let stripped = line.trim_start();
            if stripped.starts_with("- ") {
                let n = stripped[2..].trim().trim_matches('"').trim_matches('\'');
                if n.is_empty() {
                    return Err(format!("line {}: empty entry in `embedded:` list", idx + 1));
                }
                names.push(n.to_string());
            } else if !stripped.is_empty() {
                // Another top-level key — the embedded block has ended.
                in_embedded = false;
            }
        }
    }

    if !found_embedded_key {
        return Err("no `embedded:` key found".to_string());
    }
    Ok(names)
}

/// Deep-merge a `presets/schemas/<name>.yml` (SSOT) into a
/// `presets/en/<name>.yml` (preset) and return the merged YAML text.
///
/// Merge semantics (plan 2026-06-16-002 Unit 1, plan 2026-06-20-001 U1):
///   * Parse both files as `serde_yaml::Value`.
///   * Read the SSOT's top-level `schemas` mapping and the preset's
///     `event_loop.event_policy.schemas` mapping.
///   * Build a merged mapping where SSOT is the base; for each topic
///     present in the preset's inline block, deep-merge the SSOT
///     entry with the inline entry (inline values win per-field).
///   * Replace the preset's `event_loop.event_policy.schemas` with the
///     merged mapping (or insert it if absent). Other preset sections
///     are left untouched.
///   * Re-emit the YAML using `serde_yaml::to_string`. Block/flow
///     style is not preserved — the output uses serde_yaml defaults.
///     This is acceptable because the embedded YAML is consumed by
///     serde_yaml on read; downstream comparison goes through
///     `serde_yaml::Value` (see `canonicalize_schemas` in presets.rs).
///
/// Plan 2026-06-20-001 U1 (KTD-1) extends the SSOT from `schemas` only
/// into the *full* serial protocol. The mapping table is:
///
///   SSOT key                      → event_loop.* target path
///   -----------------------------------------------------------
///   schemas                       → event_loop.event_policy.schemas
///   execution_contracts           → event_loop.execution_contracts
///   verdict_gate                  → event_loop.verdict_gate
///   workflow_contract             → event_loop.workflow_contract
///   state_projection              → event_loop.state_projection
///   hat_handoff                   → event_loop.hat_handoff
///
/// Inline preset blocks under these paths act as per-key override
/// layers during the transition (same semantics as `schemas`).
fn merge_preset_with_schema(
    preset_text: &str,
    ssot_text: &str,
    preset_name: &str,
) -> Result<String, String> {
    let mut preset: serde_yaml::Value =
        serde_yaml::from_str(preset_text).map_err(|e| format!("preset YAML: {e}"))?;
    let ssot: serde_yaml::Value =
        serde_yaml::from_str(ssot_text).map_err(|e| format!("SSOT YAML: {e}"))?;

    // 1) Top-level `schemas` → `event_loop.event_policy.schemas`
    //    (existing behaviour preserved verbatim).
    if let Some(ssot_schemas) = ssot.get("schemas") {
        let ssot_schemas = match ssot_schemas {
            serde_yaml::Value::Mapping(m) => m.clone(),
            other => {
                return Err(format!(
                    "SSOT `schemas` must be a mapping of topic → schema, found {}",
                    yaml_value_kind(other)
                ));
            }
        };
        let event_loop = ensure_mapping(&mut preset, &["event_loop"])?;
        let event_policy = ensure_mapping(event_loop, &["event_policy"])?;
        let inline_schemas_mapping = event_policy
            .get("schemas")
            .and_then(|v| v.as_mapping())
            .cloned()
            .unwrap_or_default();
        let merged = merge_schema_mappings(&ssot_schemas, &inline_schemas_mapping);
        let event_policy_mapping = event_policy
            .as_mapping_mut()
            .expect("ensure_mapping returned a non-mapping Value");
        event_policy_mapping.insert(
            serde_yaml::Value::String("schemas".to_string()),
            serde_yaml::Value::Mapping(merged),
        );
    }

    // 2) Multi-section protocol merge (plan 2026-06-20-001 U1, KTD-1).
    //    Each SSOT top-level key (other than `schemas`) maps to a
    //    target path under `event_loop.*`. Inline presets MAY carry
    //    override blocks; merge semantics are identical to schemas
    //    (SSOT base, inline per-key override). Targets are created
    //    on demand — a preset that has no `event_loop.<section>` yet
    //    gets one synthesised.
    //
    //    P2-6: the SSOT for this table is
    //    `src/preset_merge_table.rs` (shared with presets.rs).
    //    This array MUST match that const. If you add/rename a
    //    section here, update `preset_merge_table.rs` too; the
    //    runtime test `p2_6_ssot_section_targets_match_build_rs`
    //    in `presets.rs` catches drift at `cargo test` time.
    let section_targets: &[(&str, &[&str])] = &[
        (
            "execution_contracts",
            &["event_loop", "execution_contracts"],
        ),
        ("verdict_gate", &["event_loop", "verdict_gate"]),
        ("workflow_contract", &["event_loop", "workflow_contract"]),
        ("state_projection", &["event_loop", "state_projection"]),
        ("hat_handoff", &["event_loop", "hat_handoff"]),
    ];
    for (ssot_key, target_path) in section_targets {
        let ssot_value = match ssot.get(*ssot_key) {
            Some(v) => v,
            None => continue,
        };
        let ssot_mapping = match ssot_value {
            serde_yaml::Value::Mapping(m) => m.clone(),
            other => {
                return Err(format!(
                    "SSOT `{ssot_key}` must be a mapping, found {}",
                    yaml_value_kind(other)
                ));
            }
        };

        // Locate (or create) the target mapping in the preset.
        // `parent` is the parent of the leaf key (e.g.
        // `event_loop`), `leaf_key` is the section name (e.g.
        // `state_projection`). We capture the existing inline
        // mapping at `event_loop.<leaf_key>` for the per-key
        // override layer, then *replace* that key in `parent`
        // with the SSOT-base + inline-merged mapping. Doing the
        // replace at the parent level (not on `leaf`) avoids the
        // doubly-nested `<parent>.<leaf_key>.<leaf_key>` shape
        // an earlier draft produced (COR-004).
        let parent_path = &target_path[..target_path.len() - 1];
        let leaf_key = target_path[target_path.len() - 1];
        let parent = ensure_mapping(&mut preset, parent_path)?;
        let parent_mapping = parent
            .as_mapping_mut()
            .expect("ensure_mapping returned a non-mapping Value");
        let inline_mapping = parent_mapping
            .get(leaf_key)
            .and_then(|v| v.as_mapping())
            .cloned()
            .unwrap_or_default();
        let merged = merge_schema_mappings(&ssot_mapping, &inline_mapping);
        parent_mapping.insert(
            serde_yaml::Value::String(leaf_key.to_string()),
            serde_yaml::Value::Mapping(merged),
        );
    }

    serde_yaml::to_string(&preset)
        .map_err(|e| format!("re-serialise merged preset `{preset_name}`: {e}"))
}

/// Walk `path` from `root` creating empty mappings as needed, returning
/// the final `Value` (which is guaranteed to be a `Value::Mapping`).
/// Returning `&mut Value` rather than `&mut Mapping` keeps the borrow
/// checker happy when this helper is chained — callers can keep
/// re-entering the path with a fresh `&mut Value` for each level.
fn ensure_mapping<'a>(
    root: &'a mut serde_yaml::Value,
    path: &[&str],
) -> Result<&'a mut serde_yaml::Value, String> {
    let mut current = root;
    for key in path {
        let entry = current
            .as_mapping_mut()
            .ok_or_else(|| format!("`{key}` parent is not a mapping"))?;
        let key_value = serde_yaml::Value::String((*key).to_string());
        if !entry.contains_key(&key_value) {
            entry.insert(
                key_value.clone(),
                serde_yaml::Value::Mapping(serde_yaml::Mapping::new()),
            );
        }
        current = entry.get_mut(&key_value).expect("key just inserted above");
    }
    if !current.is_mapping() {
        return Err(format!(
            "path `{}` did not resolve to a mapping",
            path.join(".")
        ));
    }
    Ok(current)
}

/// Deep-merge two `serde_yaml::Mapping`s. `base` provides default
/// values; `override_` replaces values per-key, recursing into nested
/// mappings. Non-mapping values are replaced wholesale.
fn merge_schema_mappings(
    base: &serde_yaml::Mapping,
    override_: &serde_yaml::Mapping,
) -> serde_yaml::Mapping {
    let mut out = serde_yaml::Mapping::new();
    // Insert base keys first.
    for (k, v) in base {
        out.insert(k.clone(), v.clone());
    }
    // Apply override keys on top.
    for (k, override_v) in override_ {
        match (out.get(k), override_v) {
            (Some(existing), serde_yaml::Value::Mapping(override_map)) if existing.is_mapping() => {
                let merged = merge_schema_mappings(
                    existing.as_mapping().expect("checked is_mapping above"),
                    override_map,
                );
                out.insert(k.clone(), serde_yaml::Value::Mapping(merged));
            }
            _ => {
                out.insert(k.clone(), override_v.clone());
            }
        }
    }
    out
}

fn yaml_value_kind(value: &serde_yaml::Value) -> &'static str {
    match value {
        serde_yaml::Value::Null => "null",
        serde_yaml::Value::Bool(_) => "bool",
        serde_yaml::Value::Number(_) => "number",
        serde_yaml::Value::String(_) => "string",
        serde_yaml::Value::Sequence(_) => "sequence",
        serde_yaml::Value::Mapping(_) => "mapping",
        serde_yaml::Value::Tagged(_) => "tagged",
    }
}
