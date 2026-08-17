//! Plan 2026-08-17-1841 (U1): `recovery_guidance` rule lint family.
//!
//! Validates every `event_loop.precheck.rules.<X>.recovery_guidance` and
//! every `event_policy.payload_consistency.rules[].recovery_guidance`
//! block declared by a preset. The runtime renderer in `correction/mod.rs`
//! (U2/U3/U4) trusts the lint's verdict, so the lint must catch every
//! shape that would otherwise render empty / wrong / unsafe guidance at
//! the target hat prompt.
//!
//! Rules covered (R7 / S5 / D2 / D5):
//!
//! - [`FINDING_RECOVERY_GUIDANCE_UNKNOWN_CHECK`] — `by_check` key is
//!   not a 1-based positive decimal string in `1..=prompt.len()`
//!   (precheck) or is not equal to the rule's own `id` (consistency).
//!   Out-of-range / wrong-id keys silently render zero guidance.
//!
//! - [`FINDING_RECOVERY_GUIDANCE_EMPTY_ITEM`] — any `common[]` or
//!   `by_check[<key>][]` item is an empty string. Renders as a blank
//!   bullet and wastes guidance budget.
//!
//! - [`FINDING_RECOVERY_GUIDANCE_UNSAFE_ITEM`] — any item exceeds
//!   `safe_display::MAX_RULE_MESSAGE_BYTES` (1024 UTF-8 bytes) or
//!   contains ANSI escapes, C0/C1 control characters, or zero-width
//!   characters. Mirrors
//!   [`FINDING_PAYLOAD_CONSISTENCY_UNSAFE_MESSAGE`].
//!
//! Severity follows the warn-by-default / strict-error pattern
//! (`LintStrictness::ownership_severity`): Warn in default mode,
//! Error in strict. The finding id prefix `recovery_guidance` is
//! deliberately distinct from the `payload_consistency_*` family so
//! guidance-shape findings never collide with rule-shape findings.

use crate::config::{PayloadConsistencyRule, PrecheckRule, RalphConfig, RecoveryGuidance};
use crate::preset_lint::finding_id::{
    FINDING_DUPLICATE_RULE_ID, FINDING_RECOVERY_GUIDANCE_EMPTY_ITEM,
    FINDING_RECOVERY_GUIDANCE_UNKNOWN_CHECK, FINDING_RECOVERY_GUIDANCE_UNSAFE_ITEM,
};
use crate::preset_lint::{LintFinding, LintSeverity, LintStrictness};

/// Maximum number of items in any single guidance list (common or
/// by_check entry). Mirrors the soft ceiling the renderer uses so a
/// preset author can't accidentally bloat the target hat prompt with
/// thousands of bullet lines. Strict-mode error past this bound.
///
/// `pub` (plan 2026-08-17-1841 U2 / T3 / C2 / R7): the correction
/// renderer in `crate::correction::render_guidance_section` imports
/// this constant to apply the same cap at render time, so a preset
/// that bypasses strict lint (e.g. hand-edited YAML without `--strict`)
/// still cannot flood the target hat prompt.
pub const MAX_ITEMS_PER_LIST: usize = 32;

/// Validate every `recovery_guidance` block attached to a precheck rule
/// (per topic under `event_loop.precheck.rules`) or a payload
/// consistency rule (under `event_policy.payload_consistency.rules`).
///
/// A preset that has no guidance blocks (the common case) returns an
/// empty finding list — no behaviour change versus the pre-plan
/// baseline.
pub fn check_recovery_guidance(
    config: &RalphConfig,
    strictness: LintStrictness,
) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let severity = strictness.ownership_severity();

    if let Some(precheck) = config.event_loop.precheck.as_ref() {
        for (topic, rule) in &precheck.rules {
            if let Some(guidance) = &rule.recovery_guidance {
                findings.extend(check_precheck_rule_guidance(
                    topic, rule, guidance, severity,
                ));
            }
        }
    }

    if let Some(policy) = config.event_loop.event_policy.as_ref() {
        // U4 / A3 (plan 2026-08-17-1841): track rule ids that have
        // a `recovery_guidance` block so duplicate ids surface a
        // `FINDING_DUPLICATE_RULE_ID` finding. The runtime
        // `validation.rs` short-circuits on the first matching
        // rule id, so the second rule's guidance is silently
        // dropped — the lint catches the drift at preset-load time.
        let mut seen_with_guidance: std::collections::HashSet<String> =
            std::collections::HashSet::new();
        for rule in &policy.payload_consistency.rules {
            if let Some(guidance) = &rule.recovery_guidance {
                findings.extend(check_consistency_rule_guidance(rule, guidance, severity));
                if !seen_with_guidance.insert(rule.id.clone()) {
                    findings.push(duplicate_rule_id_finding(severity, &rule.id));
                }
            }
        }
    }

    findings
}

/// U4 / A3 finding constructor. Fires when two
/// `payload_consistency.rules[]` entries share an `id` AND both
/// declare a `recovery_guidance` block. Runtime `validation.rs`
/// `break`s on the first match, so the second rule's guidance
/// is silently dropped — the lint surfaces this at preset-load
/// time.
fn duplicate_rule_id_finding(severity: LintSeverity, rule_id: &str) -> LintFinding {
    LintFinding {
        id: FINDING_DUPLICATE_RULE_ID,
        severity,
        message: format!(
            "two `event_policy.payload_consistency.rules[]` entries share id \"{rule_id}\" \
             and both declare a `recovery_guidance` block; the runtime evaluator in \
             `event_policy/validation.rs` short-circuits on the first matching rule, so \
             the second rule's recovery_guidance is silently dropped at runtime"
        ),
        topic: None,
        hat: None,
        owner: None,
        action_hint: Some(format!(
            "rename one of the rules with id \"{rule_id}\" so the rule ids are unique; \
             duplicate ids without recovery_guidance are caught by \
             FINDING_PAYLOAD_CONSISTENCY_DUPLICATE_ID separately"
        )),
    }
}

/// Plan 2026-08-17-1841 U3 / M2 / R9: shared core loop for both
/// the precheck and the consistency rule guidance lints. The only
/// per-family difference is the predicate that decides whether a
/// `by_check` key is in scope. By parameterising on that predicate we
/// keep the common / safety / cap / iteration code in one place; the
/// two callers each supply a closure that returns a `(bool, detail)`
/// pair — `bool` is "key is in scope", `detail` is the human-readable
/// reason when the key is out of scope.
fn check_rule_guidance_with<F>(
    topic: &str,
    guidance: &RecoveryGuidance,
    severity: LintSeverity,
    key_in_scope: F,
) -> Vec<LintFinding>
where
    F: Fn(&str) -> (bool, String),
{
    let mut findings = Vec::new();

    // Common items.
    for item in &guidance.common {
        findings.extend(check_item_safety(topic, "common", item, severity));
    }
    if guidance.common.len() > MAX_ITEMS_PER_LIST {
        findings.push(oversized_list_finding(
            severity,
            topic,
            "common",
            guidance.common.len(),
        ));
    }

    // by_check keys.
    for (key, items) in &guidance.by_check {
        let (in_scope, detail) = key_in_scope(key);
        if !in_scope {
            findings.push(unknown_check_finding(severity, topic, key, &detail));
            continue;
        }
        for item in items {
            findings.extend(check_item_safety(
                topic,
                &format!("by_check[\"{key}\"]"),
                item,
                severity,
            ));
        }
        if items.len() > MAX_ITEMS_PER_LIST {
            findings.push(oversized_list_finding(
                severity,
                topic,
                &format!("by_check[\"{key}\"]"),
                items.len(),
            ));
        }
    }

    findings
}

/// Per-precheck-rule guidance lint. The precheck `by_check` key MUST
/// be a 1-based decimal string in `1..=prompt.len()`. `prompt`
/// index `n` is rendered with key `(n + 1).to_string()` (the gate
/// hat surfaces the checklist index that way per E3).
fn check_precheck_rule_guidance(
    topic: &str,
    rule: &PrecheckRule,
    guidance: &RecoveryGuidance,
    severity: LintSeverity,
) -> Vec<LintFinding> {
    let max_index = rule.prompt.len();
    check_rule_guidance_with(topic, guidance, severity, |key| {
        let in_scope = is_precheck_key_in_range(key, max_index);
        let detail = format!(
            "precheck key \"{key}\" must be a 1-based decimal string in 1..={max_index} \
             (the rule's prompt has {max_index} items)"
        );
        (in_scope, detail)
    })
}

/// Per-consistency-rule guidance lint. The `by_check` key MUST equal
/// the rule's stable `id`. Only the rule's own id may select a
/// specific item; every other key would silently render zero
/// guidance.
fn check_consistency_rule_guidance(
    rule: &PayloadConsistencyRule,
    guidance: &RecoveryGuidance,
    severity: LintSeverity,
) -> Vec<LintFinding> {
    let topic = rule.topic.as_str();
    let rule_id = rule.id.as_str();
    check_rule_guidance_with(topic, guidance, severity, |key| {
        let in_scope = key == rule_id;
        let detail = format!(
            "consistency by_check key \"{key}\" must equal the rule's id \
             \"{rule_id}\"; the runtime selects guidance by rule id, so any \
             other key would silently render zero guidance"
        );
        (in_scope, detail)
    })
}

/// Single-item safety check shared by both rule families. Mirrors
/// `safe_display::MAX_RULE_MESSAGE_BYTES` and the existing
/// `check_message_unsafe` rules in `payload_consistency.rs`. Returns
/// at most one finding per item — empty items go through
/// [`FINDING_RECOVERY_GUIDANCE_EMPTY_ITEM`], oversized / unsafe items
/// go through [`FINDING_RECOVERY_GUIDANCE_UNSAFE_ITEM`].
fn check_item_safety(
    topic: &str,
    surface: &str,
    item: &str,
    severity: LintSeverity,
) -> Vec<LintFinding> {
    if item.is_empty() {
        return vec![empty_item_finding(severity, topic, surface)];
    }
    match check_item_unsafe(item) {
        Some(reason) => vec![unsafe_item_finding(severity, topic, surface, reason)],
        None => Vec::new(),
    }
}

/// Predicate: is the item unsafe for prompt rendering? Plan
/// 2026-08-17-1841 U3 / M1 / R8: thin forward to the shared
/// `safe_display::is_unsafe_for_prompt` helper so the lint and the
/// renderer stay in lock-step on the byte / ANSI / C0 / C1 /
/// zero-width policy.
fn check_item_unsafe(item: &str) -> Option<&'static str> {
    crate::safe_display::is_unsafe_for_prompt(item)
}

/// Predicate: is `key` a positive decimal string in `1..=max`?
///
/// Plan 2026-08-17-1841 U2 / C1 / R5: the prior implementation
/// accepted `"01"` / `"001"` / `"0128"` because `key.parse::<usize>()`
/// silently dropped the leading zeros. The runtime
/// `serde_json::Value::Number(n).to_string()` always emits
/// no-leading-zero form, so any preset author who wrote
/// `by_check: { "01": [...] }` would see the runtime fail to match
/// the key (silent never-fire). This predicate now performs a
/// strict shape check before delegating to `parse`:
///
/// - rejects the empty string,
/// - rejects a leading `+` sign (`"+1"`),
/// - rejects any leading zero — `"0"`, `"01"`, `"001"`, `"0128"`,
///   etc. (only `"0"` is itself rejected downstream by the `value == 0`
///   check; `"00"` / `"01"` are caught here),
/// - rejects whitespace and negative signs at parse time.
fn is_precheck_key_in_range(key: &str, max: usize) -> bool {
    if key.is_empty() {
        return false;
    }
    let mut bytes = key.bytes();
    match bytes.next() {
        Some(b'0') => return false,
        Some(b'+') => return false,
        Some(c) if !c.is_ascii_digit() => return false,
        _ => {}
    }
    let Ok(value) = key.parse::<usize>() else {
        return false;
    };
    if value == 0 {
        return false;
    }
    value <= max
}

fn unknown_check_finding(
    severity: LintSeverity,
    topic: &str,
    key: &str,
    detail: &str,
) -> LintFinding {
    LintFinding {
        id: FINDING_RECOVERY_GUIDANCE_UNKNOWN_CHECK,
        severity,
        message: format!(
            "recovery_guidance on rule for topic \"{topic}\" carries by_check key \"{key}\" \
             that is not in scope: {detail}; the runtime would silently skip the item"
        ),
        topic: Some(topic.to_string()),
        hat: None,
        owner: None,
        action_hint: Some(format!(
            "remove the by_check key \"{key}\" or replace it with a valid precheck \
             1-based index (1..=prompt.len()) or the rule's own id (consistency)"
        )),
    }
}

fn empty_item_finding(severity: LintSeverity, topic: &str, surface: &str) -> LintFinding {
    LintFinding {
        id: FINDING_RECOVERY_GUIDANCE_EMPTY_ITEM,
        severity,
        message: format!(
            "recovery_guidance on rule for topic \"{topic}\" declares an empty {surface} item; \
             the renderer would emit a blank bullet and waste guidance budget"
        ),
        topic: Some(topic.to_string()),
        hat: None,
        owner: None,
        action_hint: Some(format!(
            "remove the empty entry from {surface} or replace it with non-empty diagnostic text"
        )),
    }
}

fn unsafe_item_finding(
    severity: LintSeverity,
    topic: &str,
    surface: &str,
    reason: &str,
) -> LintFinding {
    LintFinding {
        id: FINDING_RECOVERY_GUIDANCE_UNSAFE_ITEM,
        severity,
        message: format!(
            "recovery_guidance on rule for topic \"{topic}\" declares a {surface} item that \
             {reason}; the runtime safe_display would strip/truncate it, but the item should \
             be clean diagnostic text"
        ),
        topic: Some(topic.to_string()),
        hat: None,
        owner: None,
        action_hint: Some(format!(
            "rewrite the {surface} item as plain diagnostic text without ANSI escapes, \
             control characters, zero-width characters, or excessive length (≤ 1024 UTF-8 \
             bytes)"
        )),
    }
}

fn oversized_list_finding(
    severity: LintSeverity,
    topic: &str,
    surface: &str,
    count: usize,
) -> LintFinding {
    LintFinding {
        id: FINDING_RECOVERY_GUIDANCE_UNSAFE_ITEM,
        severity,
        message: format!(
            "recovery_guidance on rule for topic \"{topic}\" declares {count} items in \
             {surface}, exceeding the {MAX_ITEMS_PER_LIST}-item cap; oversized lists would \
             dominate the target hat prompt"
        ),
        topic: Some(topic.to_string()),
        hat: None,
        owner: None,
        action_hint: Some(format!(
            "split the {surface} list across multiple precheck rules or trim it to the \
             {MAX_ITEMS_PER_LIST}-item cap"
        )),
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Tests (U1 acceptance Red → Green contract)
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        EventPolicyConfig, PayloadConsistencyConfig, PayloadConsistencyRule, PrecheckConfig,
        PrecheckOnFail, PrecheckRule,
    };
    use std::collections::BTreeMap;

    fn precheck_config(
        topic: &str,
        prompt_len: usize,
        guidance: Option<RecoveryGuidance>,
    ) -> RalphConfig {
        let mut rule = PrecheckRule {
            prompt: (0..prompt_len).map(|i| format!("item {i}")).collect(),
            on_fail: PrecheckOnFail {
                target: "executor".into(),
                retry_budget: 3,
                on_exhausted: String::new(),
                reason: String::new(),
            },
            recovery_guidance: guidance,
        };
        // Avoid touching on_fail defaults beyond what the helper builds.
        rule.on_fail = PrecheckOnFail::default();
        rule.on_fail.target = "executor".into();

        let mut rules = BTreeMap::new();
        rules.insert(topic.to_string(), rule);
        let mut cfg = RalphConfig::default();
        cfg.event_loop.precheck = Some(PrecheckConfig {
            enabled: true,
            rules,
        });
        cfg
    }

    fn consistency_config(
        rule_id: &str,
        topic: &str,
        guidance: Option<RecoveryGuidance>,
    ) -> RalphConfig {
        let rule = PayloadConsistencyRule {
            id: rule_id.to_string(),
            topic: topic.to_string(),
            when: serde_json::json!({"field": "x", "eq": 1}),
            message: "test".into(),
            recovery_guidance: guidance,
        };
        let mut cfg = RalphConfig::default();
        cfg.event_loop.event_policy = Some(EventPolicyConfig {
            enabled: true,
            mode: Default::default(),
            payload_consistency: PayloadConsistencyConfig {
                enabled: true,
                rules: vec![rule],
            },
            ..EventPolicyConfig::default()
        });
        cfg
    }

    fn guidance_with(common: Vec<&str>, by_check: BTreeMap<&str, Vec<&str>>) -> RecoveryGuidance {
        RecoveryGuidance {
            common: common.into_iter().map(String::from).collect(),
            by_check: by_check
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.into_iter().map(String::from).collect()))
                .collect(),
        }
    }

    // 1. Legacy baseline: no recovery_guidance blocks ⇒ no findings.
    #[test]
    fn no_guidance_blocks_is_noop() {
        let cfg = precheck_config("review.complete", 3, None);
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(findings.is_empty(), "got {findings:?}");
    }

    // 2. Common-only guidance with safe items ⇒ no findings.
    #[test]
    fn clean_common_only_passes() {
        let cfg = precheck_config(
            "review.complete",
            3,
            Some(guidance_with(
                vec!["fix the missing field"],
                BTreeMap::new(),
            )),
        );
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(findings.is_empty(), "got {findings:?}");
    }

    // 3. Valid by_check index + safe items ⇒ no findings.
    #[test]
    fn valid_precheck_by_check_passes() {
        let mut by_check = BTreeMap::new();
        by_check.insert("2", vec!["fill required_fields"]);
        let cfg = precheck_config("review.complete", 3, Some(guidance_with(vec![], by_check)));
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(findings.is_empty(), "got {findings:?}");
    }

    // 4. Out-of-range precheck by_check key ⇒ finding.
    #[test]
    fn out_of_range_precheck_key_is_flagged() {
        let mut by_check = BTreeMap::new();
        by_check.insert("4", vec!["item"]);
        let cfg = precheck_config("review.complete", 3, Some(guidance_with(vec![], by_check)));
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_RECOVERY_GUIDANCE_UNKNOWN_CHECK),
            "got {findings:?}"
        );
    }

    // 5. Non-positive precheck key (`0`, `-1`, `+1`) ⇒ finding.
    #[test]
    fn non_positive_precheck_key_is_flagged() {
        let mut by_check = BTreeMap::new();
        by_check.insert("0", vec!["item"]);
        let cfg = precheck_config("review.complete", 3, Some(guidance_with(vec![], by_check)));
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_RECOVERY_GUIDANCE_UNKNOWN_CHECK),
            "got {findings:?}"
        );
    }

    // 6. Non-numeric precheck key ⇒ finding.
    #[test]
    fn non_numeric_precheck_key_is_flagged() {
        let mut by_check = BTreeMap::new();
        by_check.insert("two", vec!["item"]);
        let cfg = precheck_config("review.complete", 3, Some(guidance_with(vec![], by_check)));
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_RECOVERY_GUIDANCE_UNKNOWN_CHECK),
            "got {findings:?}"
        );
    }

    // 7. Consistency key NOT equal to rule id ⇒ finding.
    #[test]
    fn consistency_key_not_equal_rule_id_is_flagged() {
        let mut by_check = BTreeMap::new();
        by_check.insert("other-rule", vec!["item"]);
        let cfg = consistency_config("rule-a", "fix.done", Some(guidance_with(vec![], by_check)));
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_RECOVERY_GUIDANCE_UNKNOWN_CHECK),
            "got {findings:?}"
        );
    }

    // 8. Consistency key equals rule id ⇒ no finding.
    #[test]
    fn consistency_key_equal_rule_id_passes() {
        let mut by_check = BTreeMap::new();
        by_check.insert("rule-a", vec!["item"]);
        let cfg = consistency_config("rule-a", "fix.done", Some(guidance_with(vec![], by_check)));
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(findings.is_empty(), "got {findings:?}");
    }

    // 9. Empty item ⇒ finding.
    #[test]
    fn empty_item_is_flagged() {
        let cfg = precheck_config(
            "review.complete",
            3,
            Some(guidance_with(vec![""], BTreeMap::new())),
        );
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_RECOVERY_GUIDANCE_EMPTY_ITEM),
            "got {findings:?}"
        );
    }

    // 10. ANSI escape ⇒ finding.
    #[test]
    fn ansi_escape_item_is_flagged() {
        let cfg = precheck_config(
            "review.complete",
            3,
            Some(guidance_with(
                vec!["fix \x1b[31mred\x1b[0m"],
                BTreeMap::new(),
            )),
        );
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_RECOVERY_GUIDANCE_UNSAFE_ITEM),
            "got {findings:?}"
        );
    }

    // 11. C0 control character ⇒ finding.
    #[test]
    fn c0_control_item_is_flagged() {
        let cfg = precheck_config(
            "review.complete",
            3,
            Some(guidance_with(vec!["has\x00null"], BTreeMap::new())),
        );
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_RECOVERY_GUIDANCE_UNSAFE_ITEM),
            "got {findings:?}"
        );
    }

    // 12. Newline and tab in item are allowed.
    #[test]
    fn newline_and_tab_in_item_are_allowed() {
        let cfg = precheck_config(
            "review.complete",
            3,
            Some(guidance_with(
                vec!["line one\nline two\tindented"],
                BTreeMap::new(),
            )),
        );
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        let unsafe_items: Vec<_> = findings
            .iter()
            .filter(|f| f.id == FINDING_RECOVERY_GUIDANCE_UNSAFE_ITEM)
            .collect();
        assert!(unsafe_items.is_empty(), "got {findings:?}");
    }

    // 13. Zero-width character ⇒ finding.
    #[test]
    fn zero_width_item_is_flagged() {
        let cfg = precheck_config(
            "review.complete",
            3,
            Some(guidance_with(vec!["fix\u{200B}_status"], BTreeMap::new())),
        );
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_RECOVERY_GUIDANCE_UNSAFE_ITEM),
            "got {findings:?}"
        );
    }

    // 14. Oversized item (>1024 bytes) ⇒ finding.
    #[test]
    fn oversized_item_is_flagged() {
        let big = "x".repeat(1025);
        let cfg = precheck_config(
            "review.complete",
            3,
            Some(guidance_with(vec![&big], BTreeMap::new())),
        );
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_RECOVERY_GUIDANCE_UNSAFE_ITEM),
            "got {findings:?}"
        );
    }

    // 15. Item at exactly 1024 bytes is NOT flagged.
    #[test]
    fn item_at_byte_limit_is_not_flagged() {
        let exact = "x".repeat(1024);
        let cfg = precheck_config(
            "review.complete",
            3,
            Some(guidance_with(vec![&exact], BTreeMap::new())),
        );
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        let unsafe_items: Vec<_> = findings
            .iter()
            .filter(|f| f.id == FINDING_RECOVERY_GUIDANCE_UNSAFE_ITEM)
            .collect();
        assert!(unsafe_items.is_empty(), "got {findings:?}");
    }

    // 16. Severity follows strictness (Warn default, Error strict).
    #[test]
    fn severity_follows_strictness() {
        let cfg = precheck_config(
            "review.complete",
            3,
            Some(guidance_with(vec![""], BTreeMap::new())),
        );
        let default_findings = check_recovery_guidance(&cfg, LintStrictness::Default);
        let strict_findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(
            default_findings
                .iter()
                .all(|f| f.severity == LintSeverity::Warn),
            "got {default_findings:?}"
        );
        assert!(
            strict_findings
                .iter()
                .all(|f| f.severity == LintSeverity::Error),
            "got {strict_findings:?}"
        );
    }

    // 17. Oversized list (>32 items) ⇒ finding (covers MAX_ITEMS_PER_LIST).
    #[test]
    fn oversized_list_is_flagged() {
        let many: Vec<String> = (0..33).map(|i| format!("item {i}")).collect();
        let guidance = RecoveryGuidance {
            common: many,
            by_check: BTreeMap::new(),
        };
        let cfg = precheck_config("review.complete", 3, Some(guidance));
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_RECOVERY_GUIDANCE_UNSAFE_ITEM),
            "got {findings:?}"
        );
    }

    // 18. Common + by_check safety: a bad item under by_check["1"] is flagged.
    #[test]
    fn bad_item_in_by_check_is_flagged() {
        let mut by_check = BTreeMap::new();
        by_check.insert("1", vec!["\x1b[31mansi\x1b[0m"]);
        let cfg = precheck_config("review.complete", 3, Some(guidance_with(vec![], by_check)));
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_RECOVERY_GUIDANCE_UNSAFE_ITEM),
            "got {findings:?}"
        );
    }

    // ── U2 (plan 2026-08-17-1841) — leading-zero strict reject
    //
    // C1 / R5: `is_precheck_key_in_range` previously accepted
    // `"01"` / `"001"` / `"0128"` because `parse::<usize>()` dropped
    // the leading zeros. Runtime always emits no-leading-zero form,
    // so a preset author who wrote `by_check: { "01": [...] }` would
    // see the runtime silently fail to match the key. The predicate
    // now strict-rejects any leading-zero form.

    // 19. Leading-zero key `"01"` (single digit zero prefix) ⇒ finding.
    #[test]
    fn leading_zero_precheck_key_is_flagged() {
        let mut by_check = BTreeMap::new();
        by_check.insert("01", vec!["item"]);
        let cfg = precheck_config("review.complete", 3, Some(guidance_with(vec![], by_check)));
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(
            findings
                .iter()
                .any(|f| f.id == FINDING_RECOVERY_GUIDANCE_UNKNOWN_CHECK),
            "got {findings:?}"
        );
    }

    // 20. Leading-zero keys `"001"` (multi-digit prefix) and
    //     `"0128"` (looks in-range but rejected as shape) ⇒ finding.
    #[test]
    fn multi_digit_leading_zero_precheck_keys_are_flagged() {
        for key in ["001", "0128"] {
            let mut by_check = BTreeMap::new();
            by_check.insert(key, vec!["item"]);
            let cfg = precheck_config("review.complete", 3, Some(guidance_with(vec![], by_check)));
            let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
            assert!(
                findings
                    .iter()
                    .any(|f| f.id == FINDING_RECOVERY_GUIDANCE_UNKNOWN_CHECK),
                "key {key:?}: got {findings:?}"
            );
        }
    }

    // 21. Malformed shape variants — `+1` / `-1` / `""` / ` 1` /
    //     `1 ` (whitespace, leading +, ASCII sign) ⇒ finding. The
    //     prior implementation also rejected these but only after
    //     the parse step; the new predicate catches them at the
    //     shape check, locking the behaviour.
    #[test]
    fn malformed_shape_precheck_keys_are_flagged() {
        for key in ["+1", "-1", "", " 1", "1 "] {
            let mut by_check = BTreeMap::new();
            by_check.insert(key, vec!["item"]);
            let cfg = precheck_config("review.complete", 3, Some(guidance_with(vec![], by_check)));
            let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
            assert!(
                findings
                    .iter()
                    .any(|f| f.id == FINDING_RECOVERY_GUIDANCE_UNKNOWN_CHECK),
                "key {key:?}: got {findings:?}"
            );
        }
    }

    // 22. Happy path — `"1"` / `"7"` / `"128"` (positive decimal in
    //     range) ⇒ no leading-zero finding. Confirms the strict
    //     predicate did not over-reject well-formed keys.
    #[test]
    fn well_formed_positive_precheck_keys_pass() {
        for key in ["1", "7", "128"] {
            let mut by_check = BTreeMap::new();
            by_check.insert(key, vec!["item"]);
            let cfg = precheck_config(
                "review.complete",
                130,
                Some(guidance_with(vec![], by_check)),
            );
            let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
            assert!(findings.is_empty(), "key {key:?}: got {findings:?}");
        }
    }

    // ── U4 / A3 (plan 2026-08-17-1841) — duplicate rule id
    // detection. Two `payload_consistency.rules[]` entries that
    // share an `id` AND both declare `recovery_guidance` ⇒ emit
    // `FINDING_DUPLICATE_RULE_ID`. Runtime `validation.rs` `break`s
    // on the first matching rule, so the second rule's guidance
    // is silently dropped — the lint catches the drift at
    // preset-load time.

    /// U4 / A3 happy path: two rules with distinct ids (both
    /// with recovery_guidance) ⇒ no `DUPLICATE_RULE_ID` finding.
    #[test]
    fn duplicate_rule_id_happy_path_passes() {
        let cfg = consistency_config(
            "rule-a",
            "fix.done",
            Some(guidance_with(vec![], BTreeMap::new())),
        );
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(
            !findings.iter().any(|f| f.id == FINDING_DUPLICATE_RULE_ID),
            "got {findings:?}"
        );
    }

    /// U4 / A3 negative: two rules share id `"X"` and both
    /// declare `recovery_guidance` ⇒ emit `FINDING_DUPLICATE_RULE_ID`.
    #[test]
    fn duplicate_rule_id_with_recovery_guidance_is_flagged() {
        use crate::config::{EventPolicyConfig, PayloadConsistencyConfig, PayloadConsistencyRule};
        let rule_a = PayloadConsistencyRule {
            id: "shared-id".into(),
            topic: "fix.done".into(),
            when: serde_json::json!({"field": "x", "eq": 1}),
            message: "first".into(),
            recovery_guidance: Some(guidance_with(
                vec!["first common"],
                BTreeMap::from([("shared-id".into(), vec!["first".into()])]),
            )),
        };
        let rule_b = PayloadConsistencyRule {
            id: "shared-id".into(),
            topic: "fix.done".into(),
            when: serde_json::json!({"field": "y", "eq": 2}),
            message: "second".into(),
            recovery_guidance: Some(guidance_with(
                vec!["second common"],
                BTreeMap::from([("shared-id".into(), vec!["second".into()])]),
            )),
        };
        let mut cfg = RalphConfig::default();
        cfg.event_loop.event_policy = Some(EventPolicyConfig {
            enabled: true,
            mode: Default::default(),
            payload_consistency: PayloadConsistencyConfig {
                enabled: true,
                rules: vec![rule_a, rule_b],
            },
            ..EventPolicyConfig::default()
        });
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(
            findings.iter().any(|f| f.id == FINDING_DUPLICATE_RULE_ID),
            "expected DUPLICATE_RULE_ID finding; got {findings:?}"
        );
    }

    /// U4 / A3 control: duplicate ids where only ONE rule has
    /// recovery_guidance ⇒ no `DUPLICATE_RULE_ID` finding. The
    /// lint is narrow on purpose — the runtime drop only matters
    /// when both rules' guidance would have been used.
    #[test]
    fn duplicate_rule_id_without_recovery_guidance_pair_is_not_flagged() {
        use crate::config::{EventPolicyConfig, PayloadConsistencyConfig, PayloadConsistencyRule};
        let rule_a = PayloadConsistencyRule {
            id: "shared-id".into(),
            topic: "fix.done".into(),
            when: serde_json::json!({"field": "x", "eq": 1}),
            message: "first".into(),
            recovery_guidance: Some(guidance_with(vec![], BTreeMap::new())),
        };
        let rule_b = PayloadConsistencyRule {
            id: "shared-id".into(),
            topic: "fix.done".into(),
            when: serde_json::json!({"field": "y", "eq": 2}),
            message: "second".into(),
            recovery_guidance: None,
        };
        let mut cfg = RalphConfig::default();
        cfg.event_loop.event_policy = Some(EventPolicyConfig {
            enabled: true,
            mode: Default::default(),
            payload_consistency: PayloadConsistencyConfig {
                enabled: true,
                rules: vec![rule_a, rule_b],
            },
            ..EventPolicyConfig::default()
        });
        let findings = check_recovery_guidance(&cfg, LintStrictness::Strict);
        assert!(
            !findings.iter().any(|f| f.id == FINDING_DUPLICATE_RULE_ID),
            "only one rule carries recovery_guidance; got {findings:?}"
        );
    }
}
