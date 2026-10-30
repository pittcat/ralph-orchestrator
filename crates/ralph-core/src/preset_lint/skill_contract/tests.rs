//! Tests for the skill dependency contract evaluator.
//!
//! Coverage targets the spec's acceptance criteria:
//! missing/disabled/hidden/backend-filtered required deps block;
//! optional deps warn; built-in skills satisfy without install;
//! filesystem deps need an unattended verdict pinned to content;
//! instruction references must be declared.

use super::*;
use crate::config::RalphConfig;
use std::io::Write;

fn findings(config: &RalphConfig) -> Vec<LintFinding> {
    check_skill_contract(config, LintStrictness::Default)
}

fn has_finding(findings: &[LintFinding], id: &str) -> bool {
    findings.iter().any(|f| f.id == id)
}

fn errors(findings: &[LintFinding]) -> Vec<&LintFinding> {
    findings
        .iter()
        .filter(|f| f.severity == LintSeverity::Error)
        .collect()
}

/// Minimal two-hat preset fragment (a -> b linear flow).
const HATS_YAML: &str = r#"
hats:
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["loop.complete"]
event_loop:
  starting_event: "work.start"
  completion_promise: "loop.complete"
tasks:
  enabled: false
memories:
  enabled: false
"#;

/// Parse `HATS_YAML + extra` into a config. `core.workspace_root`
/// is `#[serde(skip)]`, so `config_rooted` sets it programmatically.
fn config(yaml_extra: &str) -> RalphConfig {
    serde_yaml::from_str(&format!("{HATS_YAML}\n{yaml_extra}")).unwrap()
}

/// Same as `config` but points `core.workspace_root` at `root`
/// (mirroring what the runtime loader does post-parse).
fn config_rooted(yaml_extra: &str, root: &std::path::Path) -> RalphConfig {
    let mut cfg = config(yaml_extra);
    cfg.core.workspace_root = root.to_path_buf();
    cfg
}

/// Write a skill file into `dir`. `frontmatter` (if Some) is emitted
/// between `---` fences; `body` is written verbatim after it.
fn write_skill(
    dir: &std::path::Path,
    name: &str,
    frontmatter: Option<&str>,
    body: &str,
) -> std::path::PathBuf {
    let path = dir.join(format!("{name}.md"));
    let mut file = std::fs::File::create(&path).unwrap();
    if let Some(fm) = frontmatter {
        writeln!(file, "---\n{fm}\n---\n").unwrap();
    }
    file.write_all(body.as_bytes()).unwrap();
    path
}

// ── 1. Required dep missing → Error (blocks startup) ──────────────

#[test]
fn missing_required_skill_blocks() {
    let cfg = config(
        r#"
skills:
  contract:
    - skill: definitely-not-a-skill
      hats: [a]
      purpose: "test"
      install_hint: "vendor it under .claude/skills/"
"#,
    );
    let f = findings(&cfg);
    assert!(has_finding(&f, FINDING_SKILL_REQUIRED_MISSING), "{f:?}");
    assert!(
        errors(&f)
            .iter()
            .any(|x| x.id == FINDING_SKILL_REQUIRED_MISSING)
    );
    let msg = f
        .iter()
        .find(|x| x.id == FINDING_SKILL_REQUIRED_MISSING)
        .unwrap();
    assert!(msg.message.contains("definitely-not-a-skill"));
    assert!(msg.action_hint.is_some());
}

// ── 2. Built-in skills satisfy without install ────────────────────

#[test]
fn builtin_skill_satisfies_contract() {
    let cfg = config(
        r#"
skills:
  contract:
    - skill: ralph-tools-emit
      hats: [a, b]
      purpose: "emit precheck discipline"
      source: builtin
"#,
    );
    let f = findings(&cfg);
    assert!(
        f.is_empty(),
        "builtin dep must resolve without findings: {f:?}"
    );
}

// ── 3. Disabled (overridden off) skill → missing ──────────────────

#[test]
fn disabled_required_skill_blocks() {
    let cfg = config(
        r#"
skills:
  overrides:
    ralph-tools-emit:
      enabled: false
  contract:
    - skill: ralph-tools-emit
      hats: [a]
      purpose: "emit discipline"
"#,
    );
    let f = findings(&cfg);
    assert!(has_finding(&f, FINDING_SKILL_REQUIRED_MISSING), "{f:?}");
}

// ── 4. Optional dep missing → Warn, never silently passes ────────

#[test]
fn optional_missing_warns_not_errors() {
    let cfg = config(
        r#"
skills:
  contract:
    - skill: not-installed-optional
      hats: [a]
      purpose: "nice to have"
      required: false
      missing_behavior: "report notes the absence and continues"
"#,
    );
    let f = findings(&cfg);
    assert!(has_finding(&f, FINDING_SKILL_OPTIONAL_MISSING), "{f:?}");
    assert!(
        !has_finding(&f, FINDING_SKILL_REQUIRED_MISSING),
        "optional dep must not produce required-missing: {f:?}"
    );
    assert!(errors(&f).is_empty(), "optional dep must not block: {f:?}");
}

// ── 5. Skill hidden from the declared hat → Error ────────────────

#[test]
fn hat_invisible_skill_blocks() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(
        tmp.path(),
        "hidden-skill",
        Some("name: hidden-skill\nhats: [other-hat]"),
        "body",
    );
    let cfg = config_rooted(
        r#"
skills:
  dirs: ["."]
  contract:
    - skill: hidden-skill
      hats: [a]
      purpose: "hat a needs it"
"#,
        tmp.path(),
    );
    let f = findings(&cfg);
    assert!(has_finding(&f, FINDING_SKILL_NOT_VISIBLE_TO_HAT), "{f:?}");
    assert!(
        !has_finding(&f, FINDING_SKILL_REQUIRED_MISSING),
        "skill resolves — the failure is visibility, not presence: {f:?}"
    );
}

// ── 6. Backend-incompatible skill → visibility failure ───────────

#[test]
fn backend_incompatible_skill_blocks() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(
        tmp.path(),
        "gemini-only",
        Some("name: gemini-only\nbackends: [gemini]"),
        "body",
    );
    let cfg = config_rooted(
        r#"
cli:
  backend: claude
skills:
  dirs: ["."]
  contract:
    - skill: gemini-only
      hats: [a]
      purpose: "needs gemini"
"#,
        tmp.path(),
    );
    let f = findings(&cfg);
    assert!(has_finding(&f, FINDING_SKILL_NOT_VISIBLE_TO_HAT), "{f:?}");
}

// ── 7. Filesystem dep without unattended block → undeclared ──────

#[test]
fn filesystem_skill_requires_unattended_declaration() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(tmp.path(), "local-skill", None, "plain body");
    let cfg = config_rooted(
        r#"
skills:
  dirs: ["."]
  contract:
    - skill: local-skill
      hats: [a]
      purpose: "local helper"
"#,
        tmp.path(),
    );
    let f = findings(&cfg);
    assert!(
        has_finding(&f, FINDING_SKILL_UNATTENDED_UNDECLARED),
        "{f:?}"
    );
}

// ── 8. Interactive markers + needs_review → unverified ───────────

#[test]
fn interactive_content_flags_unattended() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(
        tmp.path(),
        "interactive-skill",
        None,
        "Step 1: ask the user for confirmation. Step 2: wait for user input.",
    );
    let cfg = config_rooted(
        r#"
skills:
  dirs: ["."]
  contract:
    - skill: interactive-skill
      hats: [a]
      purpose: "needs review"
      unattended:
        status: needs_review
"#,
        tmp.path(),
    );
    let f = findings(&cfg);
    assert!(
        has_finding(&f, FINDING_SKILL_UNATTENDED_UNVERIFIED),
        "{f:?}"
    );

    let outcome = evaluate_skill_contract(&cfg, LintStrictness::Default);
    let status = outcome
        .statuses
        .iter()
        .find(|s| s.skill == "interactive-skill")
        .unwrap();
    assert!(!status.builtin);
    assert!(status.risk_markers.contains(&"ask the user".to_string()));
}

// ── 9. Approved + matching content_hash → pass ────────────────────

#[test]
fn approved_with_current_hash_passes() {
    let tmp = tempfile::tempdir().unwrap();
    let body = "unattended-safe instructions, no interaction needed";
    write_skill(tmp.path(), "reviewed-skill", None, body);
    let hash = skill_content_hash(body);
    let yaml = format!(
        r#"
skills:
  dirs: ["."]
  contract:
    - skill: reviewed-skill
      hats: [a]
      purpose: "reviewed"
      unattended:
        status: approved
        content_hash: "{hash}"
"#
    );
    let cfg = config_rooted(&yaml, tmp.path());
    let f = findings(&cfg);
    assert!(
        f.is_empty(),
        "approved dep with matching hash must pass: {f:?}"
    );
}

// ── 10. Content change voids the approval (stale hash) ────────────

#[test]
fn changed_content_invalidates_approval() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(tmp.path(), "mutating-skill", None, "version one");
    let stale = skill_content_hash("version one");
    // Overwrite with different content — the pinned hash is now stale.
    write_skill(tmp.path(), "mutating-skill", None, "version two changed");
    let yaml = format!(
        r#"
skills:
  dirs: ["."]
  contract:
    - skill: mutating-skill
      hats: [a]
      purpose: "changed since review"
      unattended:
        status: approved
        content_hash: "{stale}"
"#
    );
    let cfg = config_rooted(&yaml, tmp.path());
    let f = findings(&cfg);
    assert!(has_finding(&f, FINDING_SKILL_UNATTENDED_STALE), "{f:?}");
}

// ── 11. Interactive status is never start-ready ───────────────────

#[test]
fn interactive_status_blocks() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(tmp.path(), "manual-skill", None, "requires manual login");
    let cfg = config_rooted(
        r#"
skills:
  dirs: ["."]
  contract:
    - skill: manual-skill
      hats: [a]
      purpose: "interactive dep"
      unattended:
        status: interactive
        notes: "needs a human at the console"
"#,
        tmp.path(),
    );
    let f = findings(&cfg);
    assert!(
        has_finding(&f, FINDING_SKILL_UNATTENDED_INTERACTIVE),
        "{f:?}"
    );
}

// ── 12. Approved without content_hash on file skill → missing hash ─

#[test]
fn approved_without_hash_is_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(tmp.path(), "flag-only", None, "body");
    let cfg = config_rooted(
        r#"
skills:
  dirs: ["."]
  contract:
    - skill: flag-only
      hats: [a]
      purpose: "self-declared flag only"
      unattended:
        status: approved
"#,
        tmp.path(),
    );
    let f = findings(&cfg);
    assert!(
        has_finding(&f, FINDING_SKILL_UNATTENDED_MISSING_HASH),
        "{f:?}"
    );
}

// ── 13. Undeclared instruction reference → coverage finding ──────

fn instructions_config(a: &str, b: &str, extra: &str) -> RalphConfig {
    let hats = format!(
        r#"
hats:
  a:
    name: "A"
    description: "Producer"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    instructions: "{a}"
  b:
    name: "B"
    description: "Consumer"
    triggers: ["work.ready"]
    publishes: ["loop.complete"]
    instructions: "{b}"
event_loop:
  starting_event: "work.start"
  completion_promise: "loop.complete"
tasks:
  enabled: false
memories:
  enabled: false
{extra}
"#
    );
    serde_yaml::from_str(&hats).unwrap()
}

#[test]
fn undeclared_instruction_reference_flagged() {
    let cfg = instructions_config(
        "load it first: `ralph tools skill load ralph-tools-emit`",
        "consumer work",
        r#"
skills:
  contract:
    - skill: ralph-tools-emit
      hats: [b]
      purpose: "only declared for b"
"#,
    );
    let f = findings(&cfg);
    assert!(
        has_finding(&f, FINDING_SKILL_CONTRACT_UNDECLARED_REFERENCE),
        "{f:?}"
    );
    let finding = f
        .iter()
        .find(|x| x.id == FINDING_SKILL_CONTRACT_UNDECLARED_REFERENCE)
        .unwrap();
    assert_eq!(finding.hat.as_deref(), Some("a"));
}

#[test]
fn declared_instruction_reference_passes() {
    let cfg = instructions_config(
        "`ralph tools skill load ralph-tools-emit`",
        "consumer work",
        r#"
skills:
  contract:
    - skill: ralph-tools-emit
      hats: [a]
      purpose: "emit discipline"
"#,
    );
    let f = findings(&cfg);
    assert!(
        !has_finding(&f, FINDING_SKILL_CONTRACT_UNDECLARED_REFERENCE),
        "{f:?}"
    );
}

#[test]
fn undeclared_reference_warns_without_contract() {
    let cfg = instructions_config(
        "`ralph tools skill load ralph-tools-emit`",
        "consumer work",
        "",
    );
    let f = findings(&cfg);
    let finding = f
        .iter()
        .find(|x| x.id == FINDING_SKILL_CONTRACT_UNDECLARED_REFERENCE)
        .expect("no-contract preset still gets the audit signal");
    assert_eq!(finding.severity, LintSeverity::Warn);
}

#[test]
fn undeclared_reference_errors_in_strict_with_contract() {
    let cfg = instructions_config(
        "`ralph tools skill load stray-skill`",
        "consumer work",
        r#"
skills:
  contract:
    - skill: ralph-tools-emit
      hats: [a]
      purpose: "emit discipline"
"#,
    );
    let f = check_skill_contract(&cfg, LintStrictness::Strict);
    let finding = f
        .iter()
        .find(|x| x.id == FINDING_SKILL_CONTRACT_UNDECLARED_REFERENCE)
        .unwrap();
    assert_eq!(finding.severity, LintSeverity::Error);
}

// ── 14. Guardrails-level references need a declared entry ─────────

#[test]
fn guardrails_reference_covered_by_any_entry() {
    let cfg = config(
        r#"
core:
  guardrails:
    - "final report: `ralph tools skill load ralph-html`"
skills:
  contract:
    - skill: ralph-html
      hats: [b]
      purpose: "final report rendering"
"#,
    );
    let f = findings(&cfg);
    assert!(
        !has_finding(&f, FINDING_SKILL_CONTRACT_UNDECLARED_REFERENCE),
        "guardrails ref covered by an existing entry: {f:?}"
    );
}

#[test]
fn guardrails_reference_undeclared_flagged() {
    let cfg = config(
        r#"
core:
  guardrails:
    - "final report: `ralph tools skill load ralph-html`"
skills:
  contract:
    - skill: ralph-tools-emit
      hats: [a]
      purpose: "emit discipline"
"#,
    );
    let f = findings(&cfg);
    assert!(
        has_finding(&f, FINDING_SKILL_CONTRACT_UNDECLARED_REFERENCE),
        "{f:?}"
    );
}

// ── 15. skills.enabled: false → per-dep disabled finding ──────────

#[test]
fn skills_disabled_blocks_contract() {
    let cfg = config(
        r#"
skills:
  enabled: false
  contract:
    - skill: ralph-tools-emit
      hats: [a]
      purpose: "emit discipline"
"#,
    );
    let f = findings(&cfg);
    assert!(has_finding(&f, FINDING_SKILL_SKILLS_DISABLED), "{f:?}");
}

// ── 16. Declaration-shape validation ─────────────────────────────

#[test]
fn invalid_entries_flagged() {
    let cfg = config(
        r#"
skills:
  contract:
    - skill: ""
      hats: [a]
      purpose: "empty name"
    - skill: ralph-tools-emit
      hats: []
      purpose: "empty hats"
    - skill: ralph-tools-emit
      hats: [ghost]
      purpose: "unknown hat"
    - skill: ralph-tools-emit
      hats: [a]
      purpose: ""
    - skill: opt-no-fallback
      hats: [a]
      purpose: "optional without behavior"
      required: false
"#,
    );
    let f = findings(&cfg);
    let invalid_count = f
        .iter()
        .filter(|x| x.id == FINDING_SKILL_CONTRACT_INVALID)
        .count();
    assert!(
        invalid_count >= 5,
        "expected >=5 invalid findings, got {invalid_count}: {f:?}"
    );
}

#[test]
fn optional_without_missing_behavior_is_invalid() {
    let cfg = config(
        r#"
skills:
  contract:
    - skill: maybe-missing
      hats: [a]
      purpose: "optional"
      required: false
"#,
    );
    let f = findings(&cfg);
    assert!(has_finding(&f, FINDING_SKILL_CONTRACT_INVALID), "{f:?}");
}

// ── 17. Wildcard hats expand to every preset hat ──────────────────

#[test]
fn wildcard_hats_cover_all() {
    let cfg = instructions_config(
        "`ralph tools skill load ralph-tools-emit`",
        "`ralph tools skill load ralph-tools-emit`",
        r#"
skills:
  contract:
    - skill: ralph-tools-emit
      hats: ["*"]
      purpose: "every hat emits"
"#,
    );
    let outcome = evaluate_skill_contract(&cfg, LintStrictness::Default);
    let status = outcome
        .statuses
        .iter()
        .find(|s| s.skill == "ralph-tools-emit")
        .unwrap();
    assert_eq!(status.hats, vec!["a".to_string(), "b".to_string()]);
    assert!(status.invisible_hats.is_empty());
    assert!(
        !outcome
            .findings
            .iter()
            .any(|x| x.id == FINDING_SKILL_CONTRACT_UNDECLARED_REFERENCE)
    );
}

// ── 18. auto_inject wiring check ──────────────────────────────────

#[test]
fn autoinject_mismatch_flagged_then_fixed() {
    let cfg = config(
        r#"
skills:
  contract:
    - skill: ralph-tools-emit
      hats: [a]
      purpose: "inject into prompt"
      load: auto_inject
"#,
    );
    let f = findings(&cfg);
    assert!(has_finding(&f, FINDING_SKILL_AUTOINJECT_MISMATCH), "{f:?}");

    let cfg_fixed = config(
        r#"
skills:
  overrides:
    ralph-tools-emit:
      auto_inject: true
  contract:
    - skill: ralph-tools-emit
      hats: [a]
      purpose: "inject into prompt"
      load: auto_inject
"#,
    );
    let f = findings(&cfg_fixed);
    assert!(
        !has_finding(&f, FINDING_SKILL_AUTOINJECT_MISMATCH),
        "override enabling auto_inject must satisfy the contract: {f:?}"
    );
}

// ── 19. External requires: tools / env / files ────────────────────

#[test]
fn missing_required_tool_flagged() {
    let cfg = config(
        r#"
skills:
  contract:
    - skill: ralph-tools-emit
      hats: [a]
      purpose: "needs a tool"
      requires:
        tools: [definitely-not-a-real-binary-xyz123]
"#,
    );
    let f = findings(&cfg);
    assert!(
        has_finding(&f, FINDING_SKILL_REQUIRES_TOOL_MISSING),
        "{f:?}"
    );
}

#[test]
fn present_tool_passes() {
    let tool = if cfg!(windows) { "cmd" } else { "ls" };
    let yaml = format!(
        r#"
skills:
  contract:
    - skill: ralph-tools-emit
      hats: [a]
      purpose: "uses {tool}"
      requires:
        tools: [{tool}]
"#
    );
    let cfg = config(&yaml);
    let f = findings(&cfg);
    assert!(
        !has_finding(&f, FINDING_SKILL_REQUIRES_TOOL_MISSING),
        "{f:?}"
    );
}

#[test]
fn missing_required_env_flagged() {
    let cfg = config(
        r#"
skills:
  contract:
    - skill: ralph-tools-emit
      hats: [a]
      purpose: "needs a token"
      requires:
        env: [RALPH_TEST_DEFINITELY_UNSET_ENV_9f8e]
"#,
    );
    let f = findings(&cfg);
    assert!(has_finding(&f, FINDING_SKILL_REQUIRES_ENV_MISSING), "{f:?}");
}

#[test]
fn missing_required_file_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config_rooted(
        r#"
skills:
  contract:
    - skill: ralph-tools-emit
      hats: [a]
      purpose: "needs a companion"
      requires:
        files: [no-such-companion.toml]
"#,
        tmp.path(),
    );
    let f = findings(&cfg);
    assert!(
        has_finding(&f, FINDING_SKILL_REQUIRES_FILE_MISSING),
        "{f:?}"
    );

    // Create the file → recheck passes (spec: recheck after repair).
    std::fs::write(tmp.path().join("no-such-companion.toml"), "x = 1").unwrap();
    let f = findings(&cfg);
    assert!(
        !has_finding(&f, FINDING_SKILL_REQUIRES_FILE_MISSING),
        "recheck after repair must pass: {f:?}"
    );
}

// ── 20. contract_file external declaration ───────────────────────

#[test]
fn contract_file_entries_are_evaluated() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("contract.yml"),
        "contract:\n  - skill: not-real-file-skill\n    hats: [a]\n    purpose: external\n",
    )
    .unwrap();
    let cfg = config_rooted(
        r#"
skills:
  contract_file: contract.yml
"#,
        tmp.path(),
    );
    let f = findings(&cfg);
    assert!(
        has_finding(&f, FINDING_SKILL_REQUIRED_MISSING),
        "external contract entries must be evaluated: {f:?}"
    );
}

#[test]
fn missing_contract_file_flagged() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = config_rooted(
        r#"
skills:
  contract_file: no-such-contract.yml
"#,
        tmp.path(),
    );
    let f = findings(&cfg);
    assert!(has_finding(&f, FINDING_SKILL_CONTRACT_FILE_ERROR), "{f:?}");
}

// ── 21. Source constraint ─────────────────────────────────────────

#[test]
fn source_constraint_enforced() {
    // `source: project` but resolves to builtin → mismatch.
    let cfg = config(
        r#"
skills:
  contract:
    - skill: ralph-tools-emit
      hats: [a]
      purpose: "should be project file"
      source: project
"#,
    );
    let f = findings(&cfg);
    assert!(has_finding(&f, FINDING_SKILL_SOURCE_MISMATCH), "{f:?}");
}

#[test]
fn source_builtin_constraint_passes() {
    // `ralph-html` is hat-restricted by default; an override widens
    // visibility to hat `a` — the builtin source constraint still holds.
    let cfg = config(
        r#"
skills:
  overrides:
    ralph-html:
      hats: [a]
  contract:
    - skill: ralph-html
      hats: [a]
      purpose: "final report"
      source: builtin
"#,
    );
    let f = findings(&cfg);
    assert!(f.is_empty(), "builtin source constraint ok: {f:?}");
}

// ── 22. tasks disabled removes gated builtin → missing ────────────

#[test]
fn disabled_subsystem_skill_is_missing() {
    let cfg = config(
        r#"
skills:
  contract:
    - skill: ralph-tools-tasks
      hats: [a]
      purpose: "task commands"
"#,
    );
    // HATS_YAML sets `tasks.enabled: false`, so the runtime removes
    // `ralph-tools-tasks` from the effective registry.
    let f = findings(&cfg);
    assert!(has_finding(&f, FINDING_SKILL_REQUIRED_MISSING), "{f:?}");
}

// ── 23. Solo / hatless preset path ────────────────────────────────

#[test]
fn hatless_preset_solo_visibility() {
    let cfg: RalphConfig = serde_yaml::from_str(
        r#"
skills:
  contract:
    - skill: ralph-tools-emit
      hats: ["*"]
      purpose: "solo emits"
"#,
    )
    .unwrap();
    let f = findings(&cfg);
    assert!(
        f.is_empty(),
        "hatless preset + wildcard must pass solo check: {f:?}"
    );
}

// ── 24. statuses surface per-dep detail for `skill check` ─────────

#[test]
fn statuses_report_resolution_details() {
    let tmp = tempfile::tempdir().unwrap();
    write_skill(tmp.path(), "local-one", None, "body text");
    let cfg = config_rooted(
        r#"
skills:
  dirs: ["."]
  contract:
    - skill: local-one
      hats: [a]
      purpose: "file dep"
      unattended:
        status: approved
    - skill: ralph-tools-emit
      hats: [b]
      purpose: "builtin dep"
"#,
        tmp.path(),
    );
    let outcome = evaluate_skill_contract(&cfg, LintStrictness::Default);
    assert_eq!(outcome.statuses.len(), 2);

    let local = outcome
        .statuses
        .iter()
        .find(|s| s.skill == "local-one")
        .unwrap();
    assert!(local.resolved);
    assert!(!local.builtin);
    assert!(local.source_path.is_some());
    assert!(
        local
            .content_hash
            .as_deref()
            .unwrap()
            .starts_with("sha256:")
    );
    // approved without hash → not ok (Error finding attached)
    assert!(!local.ok);

    let builtin = outcome
        .statuses
        .iter()
        .find(|s| s.skill == "ralph-tools-emit")
        .unwrap();
    assert!(builtin.resolved);
    assert!(builtin.builtin);
    assert!(builtin.ok);
}

// ── 26. Duplicate declarations: identical collapse, conflicts error ─

#[test]
fn identical_duplicate_entries_collapse() {
    // The same entry declared both inline and via `contract_file`
    // (or twice by preset + operator in the `-H` union) collapses to
    // one — the merged list stays the single source of truth without
    // noise or double findings.
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(
        tmp.path().join("contract.yml"),
        "contract:\n  - skill: ralph-tools-emit\n    hats: [a]\n    purpose: emit discipline\n    required: true\n    load: on_demand\n    source: builtin\n",
    )
    .unwrap();
    let cfg = config_rooted(
        r#"
skills:
  contract_file: contract.yml
  contract:
    - skill: ralph-tools-emit
      hats: [a]
      purpose: emit discipline
      required: true
      load: on_demand
      source: builtin
"#,
        tmp.path(),
    );
    let outcome = evaluate_skill_contract(&cfg, LintStrictness::Strict);
    assert_eq!(
        outcome.statuses.len(),
        1,
        "identical duplicate must collapse to a single dep row: {:?}",
        outcome.statuses
    );
    assert!(
        !outcome
            .findings
            .iter()
            .any(|f| f.id == FINDING_SKILL_CONTRACT_INVALID),
        "identical dupes are not conflicts: {:?}",
        outcome.findings
    );
}

#[test]
fn conflicting_duplicate_entries_flagged_invalid() {
    // Same skill + same hat-set but different `required` — two
    // different verdicts could apply, so the ambiguity is an Error.
    let cfg = config(
        r#"
skills:
  contract:
    - skill: ralph-tools-emit
      hats: [a]
      purpose: first
      required: true
      load: on_demand
      source: builtin
    - skill: ralph-tools-emit
      hats: [a]
      purpose: second
      required: false
      load: on_demand
      source: builtin
"#,
    );
    let f = findings(&cfg);
    assert!(has_finding(&f, FINDING_SKILL_CONTRACT_INVALID), "{f:?}");
    let invalid = f
        .iter()
        .find(|x| x.id == FINDING_SKILL_CONTRACT_INVALID)
        .unwrap();
    assert!(invalid.message.contains("ralph-tools-emit"));
    // First declaration wins deterministically.
    let outcome = evaluate_skill_contract(&cfg, LintStrictness::Strict);
    assert_eq!(outcome.statuses.len(), 1);
    assert!(outcome.statuses[0].required);
}

#[test]
fn wildcard_and_explicit_same_expansion_conflict() {
    // `hats: ["*"]` expands to [a, b] in the fixture — an explicit
    // `hats: [a, b]` entry for the same skill with a different
    // `required` is a real conflict: hat a would otherwise get both a
    // blocking and an optional verdict.
    let cfg = config(
        r#"
skills:
  contract:
    - skill: ralph-tools-emit
      hats: ["*"]
      purpose: all hats
      required: true
      load: on_demand
      source: builtin
    - skill: ralph-tools-emit
      hats: [a, b]
      purpose: listed hats
      required: false
      load: on_demand
      source: builtin
      missing_behavior: "degraded"
"#,
    );
    let f = findings(&cfg);
    assert!(
        has_finding(&f, FINDING_SKILL_CONTRACT_INVALID),
        "wildcard + explicit same expansion must conflict: {f:?}"
    );
}

#[test]
fn same_skill_different_hat_sets_is_not_a_conflict() {
    // `hats: [a]` and `hats: [a, b]` for the same skill are distinct
    // coverage entries — allowed, both evaluated.
    let cfg = config(
        r#"
skills:
  contract:
    - skill: ralph-tools-emit
      hats: [a]
      purpose: hat a
      required: true
      load: on_demand
      source: builtin
    - skill: ralph-tools-emit
      hats: [a, b]
      purpose: hat a and b
      required: false
      load: on_demand
      source: builtin
      missing_behavior: "hat b proceeds without emit discipline docs"
"#,
    );
    let outcome = evaluate_skill_contract(&cfg, LintStrictness::Strict);
    assert_eq!(outcome.statuses.len(), 2);
    assert!(
        !outcome
            .findings
            .iter()
            .any(|f| f.id == FINDING_SKILL_CONTRACT_INVALID),
        "{:?}",
        outcome.findings
    );
}

// ── 25. Contract ids all registered in ALL_FINDING_IDS ────────────

#[test]
fn contract_finding_ids_registered() {
    use super::super::finding_id::ALL_FINDING_IDS;
    for id in [
        FINDING_SKILL_CONTRACT_INVALID,
        FINDING_SKILL_CONTRACT_FILE_ERROR,
        FINDING_SKILL_CONTRACT_REGISTRY_ERROR,
        FINDING_SKILL_CONTRACT_UNDECLARED_REFERENCE,
        FINDING_SKILL_REQUIRED_MISSING,
        FINDING_SKILL_OPTIONAL_MISSING,
        FINDING_SKILL_SKILLS_DISABLED,
        FINDING_SKILL_SOURCE_MISMATCH,
        FINDING_SKILL_NOT_VISIBLE_TO_HAT,
        FINDING_SKILL_AUTOINJECT_MISMATCH,
        FINDING_SKILL_REQUIRES_TOOL_MISSING,
        FINDING_SKILL_REQUIRES_ENV_MISSING,
        FINDING_SKILL_REQUIRES_FILE_MISSING,
        FINDING_SKILL_UNATTENDED_UNDECLARED,
        FINDING_SKILL_UNATTENDED_UNVERIFIED,
        FINDING_SKILL_UNATTENDED_INTERACTIVE,
        FINDING_SKILL_UNATTENDED_MISSING_HASH,
        FINDING_SKILL_UNATTENDED_STALE,
    ] {
        assert!(
            ALL_FINDING_IDS.contains(&id),
            "{id} must be registered in ALL_FINDING_IDS"
        );
    }
}
