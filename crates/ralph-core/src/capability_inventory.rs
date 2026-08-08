//! 2026-07-27-002 plan Unit 3: capability inventory for preset authors/reviewers.
//!
//! Each `Capability` represents a runtime capability that preset authors
//! and reviewers MUST understand to assess whether their preset can
//! legitimately use it. The `covered_in_author_review` field is computed
//! at compile time by checking whether the corresponding reference documents
//! contain stable anchors (`<!-- anchor: <id> -->` comments).

use serde::{Deserialize, Serialize};

// Relative to crates/ralph-core/src/ (source file location).
//
// Authors and reviewers each read their own local references; the
// author-side and review-side mirrors are kept byte-identical so
// compile-time anchor coverage stays meaningful for both hats. Even
// after the shared ``skills/ralph-preset-common`` directory is
// removed, every capability in this inventory still has stable
// ``<!-- anchor: <id> -->`` markers in both skill-local paths below.
const AGENT_NATIVE_MODEL: &str =
    include_str!("../../../skills/ralph-preset-author/references/agent-native-model.md");
const COMMANDS_DOC: &str =
    include_str!("../../../skills/ralph-preset-author/references/commands.md");
const REVIEW_AGENT_NATIVE_MODEL: &str =
    include_str!("../../../skills/ralph-preset-review/references/agent-native-model.md");
const REVIEW_COMMANDS_DOC: &str =
    include_str!("../../../skills/ralph-preset-review/references/commands.md");
const REVIEW_FINDING_RUBRIC: &str =
    include_str!("../../../skills/ralph-preset-review/references/finding-rubric.md");

/// Returns "yes" if both anchors are present, "partial" if at least one
/// is present, "no" if none are present.
const fn compute_coverage(author_anchor: bool, review_anchor: bool) -> &'static str {
    if author_anchor && review_anchor {
        "yes"
    } else if author_anchor || review_anchor {
        "partial"
    } else {
        "no"
    }
}

/// Compile-time anchor presence check using simple byte scan.
/// All args are Copy types, so usable in const fn.
const fn doc_has_anchor(haystack: &str, needle: &str) -> bool {
    let h = haystack.as_bytes();
    let n = needle.as_bytes();
    let hl = h.len();
    let nl = n.len();
    if nl > hl {
        return false;
    }
    let mut i = 0;
    while i <= hl - nl {
        let mut j = 0;
        while j < nl {
            if h[i + j] != n[j] {
                break;
            }
            j += 1;
        }
        if j == nl {
            return true;
        }
        i += 1;
    }
    false
}

// Pre-computed anchor strings per capability id.
const ANCHOR_WAVE_EMIT: &str = "<!-- anchor: wave-emit -->";
const ANCHOR_SUPERVISOR_EMIT: &str = "<!-- anchor: supervisor-emit -->";
const ANCHOR_TASK_ID_LIVE: &str = "<!-- anchor: task-id-live -->";
const ANCHOR_ARTIFACT_FIRST: &str = "<!-- anchor: artifact-first -->";
const ANCHOR_PAYLOAD_CONSISTENCY: &str = "<!-- anchor: payload-consistency -->";
const ANCHOR_TRIGGER_CONTEXT: &str = "<!-- anchor: trigger-context -->";

/// One entry in the capability inventory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Capability {
    /// Stable kebab-case identifier (e.g. `wave-emit`, `task-id-live`).
    pub id: &'static str,
    /// What triggers this capability (e.g. `execution_model == "wave"`).
    pub trigger_signal: &'static str,
    /// When the capability applies (e.g. "when the preset uses `ralph wave emit`").
    pub applies_when: &'static str,
    /// Stable evidence sources (paths to references / skill docs).
    pub evidence_sources: &'static [&'static str],
    /// Recommended evidence level: "static" | "runtime" | "unverified".
    pub recommended_evidence_level: &'static str,
    /// Source of this inventory entry: "binary_embedded" | "repo_local".
    pub source: &'static str,
    /// Whether this capability is covered in the author/review reference docs.
    /// Computed at compile time from stable `<!-- anchor: <id> -->` comments.
    pub covered_in_author_review: &'static str,
}

/// Manual Deserialize impl: the capability inventory is a static compile-time
/// list, not loaded from JSON at runtime. This impl always returns an error
/// to catch any accidental deserialization attempts.
impl<'de> Deserialize<'de> for Capability {
    fn deserialize<D>(_deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Err(serde::de::Error::custom(
            "Capability cannot be deserialized; it is a compile-time static inventory",
        ))
    }
}

/// The static capability inventory. New capabilities should be added here
/// in order, with stable `id` values that map to references anchors.
pub fn capability_inventory() -> Vec<Capability> {
    vec![
        Capability {
            id: "wave-emit",
            trigger_signal: "execution_model == wave | supervisor+wave",
            applies_when: "preset uses ralph wave emit / ralph wave verify",
            evidence_sources: &[
                "skills/ralph-preset-author/references/finding-rubric.md",
                "crates/ralph-core/data/ralph-tools-wave.md",
            ],
            recommended_evidence_level: "runtime",
            source: "binary_embedded",
            covered_in_author_review: compute_coverage(
                doc_has_anchor(AGENT_NATIVE_MODEL, ANCHOR_WAVE_EMIT)
                    || doc_has_anchor(COMMANDS_DOC, ANCHOR_WAVE_EMIT),
                doc_has_anchor(REVIEW_AGENT_NATIVE_MODEL, ANCHOR_WAVE_EMIT)
                    || doc_has_anchor(REVIEW_COMMANDS_DOC, ANCHOR_WAVE_EMIT)
                    || doc_has_anchor(REVIEW_FINDING_RUBRIC, ANCHOR_WAVE_EMIT),
            ),
        },
        Capability {
            id: "supervisor-emit",
            trigger_signal: "execution_model == supervisor | supervisor+wave",
            applies_when: "preset sets event_loop.supervisor.enabled",
            evidence_sources: &[
                "skills/ralph-preset-author/references/finding-rubric.md",
                // Plan 2026-08-09-001: removed `ce-executor-supervisor`
                // builtin. The surviving supervisor-enabled builtin is
                // `parallel-forge`, which still ships with
                // `event_loop.supervisor.enabled: true` and isolated mode.
                "presets/en/parallel-forge.yml",
            ],
            recommended_evidence_level: "runtime",
            source: "binary_embedded",
            covered_in_author_review: compute_coverage(
                doc_has_anchor(AGENT_NATIVE_MODEL, ANCHOR_SUPERVISOR_EMIT)
                    || doc_has_anchor(COMMANDS_DOC, ANCHOR_SUPERVISOR_EMIT),
                doc_has_anchor(REVIEW_AGENT_NATIVE_MODEL, ANCHOR_SUPERVISOR_EMIT)
                    || doc_has_anchor(REVIEW_COMMANDS_DOC, ANCHOR_SUPERVISOR_EMIT)
                    || doc_has_anchor(REVIEW_FINDING_RUBRIC, ANCHOR_SUPERVISOR_EMIT),
            ),
        },
        Capability {
            id: "task-id-live",
            trigger_signal: "any preset that emits work.done",
            applies_when: "task_id is required for any work.done emit",
            evidence_sources: &[
                "crates/ralph-core/data/ralph-tools-tasks.md",
                "skills/ralph-preset-author/references/commands.md",
            ],
            recommended_evidence_level: "runtime",
            source: "binary_embedded",
            covered_in_author_review: compute_coverage(
                doc_has_anchor(AGENT_NATIVE_MODEL, ANCHOR_TASK_ID_LIVE)
                    || doc_has_anchor(COMMANDS_DOC, ANCHOR_TASK_ID_LIVE),
                doc_has_anchor(REVIEW_AGENT_NATIVE_MODEL, ANCHOR_TASK_ID_LIVE)
                    || doc_has_anchor(REVIEW_COMMANDS_DOC, ANCHOR_TASK_ID_LIVE)
                    || doc_has_anchor(REVIEW_FINDING_RUBRIC, ANCHOR_TASK_ID_LIVE),
            ),
        },
        Capability {
            id: "artifact-first",
            trigger_signal: "any preset that emits results",
            applies_when: "presets emit complete results / long content / cross-hat summaries",
            evidence_sources: &[
                "skills/ralph-preset-author/references/agent-native-model.md",
                "skills/ralph-preset-author/references/finding-rubric.md",
            ],
            recommended_evidence_level: "runtime",
            source: "binary_embedded",
            covered_in_author_review: compute_coverage(
                doc_has_anchor(AGENT_NATIVE_MODEL, ANCHOR_ARTIFACT_FIRST)
                    || doc_has_anchor(COMMANDS_DOC, ANCHOR_ARTIFACT_FIRST),
                doc_has_anchor(REVIEW_AGENT_NATIVE_MODEL, ANCHOR_ARTIFACT_FIRST)
                    || doc_has_anchor(REVIEW_COMMANDS_DOC, ANCHOR_ARTIFACT_FIRST)
                    || doc_has_anchor(REVIEW_FINDING_RUBRIC, ANCHOR_ARTIFACT_FIRST),
            ),
        },
        Capability {
            id: "payload-consistency",
            trigger_signal: "preset has payload_consistency rules",
            applies_when: "preset declares inter-field invariants on a single emit payload",
            evidence_sources: &[
                "skills/ralph-preset-author/SKILL.md",
                "crates/ralph-core/data/ralph-tools-emit.md",
            ],
            recommended_evidence_level: "runtime",
            source: "binary_embedded",
            covered_in_author_review: compute_coverage(
                doc_has_anchor(AGENT_NATIVE_MODEL, ANCHOR_PAYLOAD_CONSISTENCY)
                    || doc_has_anchor(COMMANDS_DOC, ANCHOR_PAYLOAD_CONSISTENCY),
                doc_has_anchor(REVIEW_AGENT_NATIVE_MODEL, ANCHOR_PAYLOAD_CONSISTENCY)
                    || doc_has_anchor(REVIEW_COMMANDS_DOC, ANCHOR_PAYLOAD_CONSISTENCY)
                    || doc_has_anchor(REVIEW_FINDING_RUBRIC, ANCHOR_PAYLOAD_CONSISTENCY),
            ),
        },
        Capability {
            id: "trigger-context",
            trigger_signal: "preset uses trigger_context declarations",
            applies_when: "preset declares summary_fields / routing_hints for a trigger topic",
            evidence_sources: &[
                "crates/ralph-core/src/trigger_context.rs",
                "skills/ralph-preset-author/references/commands.md",
            ],
            recommended_evidence_level: "runtime",
            source: "binary_embedded",
            covered_in_author_review: compute_coverage(
                doc_has_anchor(AGENT_NATIVE_MODEL, ANCHOR_TRIGGER_CONTEXT)
                    || doc_has_anchor(COMMANDS_DOC, ANCHOR_TRIGGER_CONTEXT),
                doc_has_anchor(REVIEW_AGENT_NATIVE_MODEL, ANCHOR_TRIGGER_CONTEXT)
                    || doc_has_anchor(REVIEW_COMMANDS_DOC, ANCHOR_TRIGGER_CONTEXT)
                    || doc_has_anchor(REVIEW_FINDING_RUBRIC, ANCHOR_TRIGGER_CONTEXT),
            ),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capability_inventory_is_non_empty() {
        assert!(
            capability_inventory().len() >= 6,
            "expected at least 6 capabilities"
        );
    }

    #[test]
    fn capability_inventory_ids_are_kebab_case() {
        for c in capability_inventory() {
            assert!(!c.id.is_empty(), "id must be non-empty");
            assert!(
                c.id.chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
                "id must be kebab-case: {}",
                c.id
            );
        }
    }

    #[test]
    fn capability_inventory_evidence_level_valid() {
        for c in capability_inventory() {
            assert!(
                matches!(
                    c.recommended_evidence_level,
                    "static" | "runtime" | "unverified"
                ),
                "invalid evidence level for {}: {}",
                c.id,
                c.recommended_evidence_level
            );
        }
    }

    #[test]
    fn capability_inventory_covered_in_author_review_valid() {
        for c in capability_inventory() {
            assert!(
                matches!(c.covered_in_author_review, "yes" | "partial" | "no"),
                "invalid covered_in_author_review for {}: {}",
                c.id,
                c.covered_in_author_review
            );
        }
    }

    #[test]
    fn compute_coverage_all_three_states() {
        // All three states are reachable via the compute_coverage logic
        assert_eq!(compute_coverage(true, true), "yes");
        assert_eq!(compute_coverage(true, false), "partial");
        assert_eq!(compute_coverage(false, true), "partial");
        assert_eq!(compute_coverage(false, false), "no");
    }

    #[test]
    fn capability_inventory_covered_in_author_review_yes() {
        // All current capabilities have anchors in all three docs -> "yes"
        for c in capability_inventory() {
            assert_eq!(
                c.covered_in_author_review, "yes",
                "capability {} should have 'yes' coverage (anchors exist in all three docs)",
                c.id
            );
        }
    }

    #[test]
    fn capability_inventory_checks_review_skill_references() {
        let anchors = [
            ANCHOR_WAVE_EMIT,
            ANCHOR_SUPERVISOR_EMIT,
            ANCHOR_TASK_ID_LIVE,
            ANCHOR_ARTIFACT_FIRST,
            ANCHOR_PAYLOAD_CONSISTENCY,
            ANCHOR_TRIGGER_CONTEXT,
        ];

        for anchor in anchors {
            assert!(
                doc_has_anchor(REVIEW_AGENT_NATIVE_MODEL, anchor)
                    || doc_has_anchor(REVIEW_COMMANDS_DOC, anchor)
                    || doc_has_anchor(REVIEW_FINDING_RUBRIC, anchor),
                "review skill references are missing capability anchor: {anchor}"
            );
        }
    }
}
