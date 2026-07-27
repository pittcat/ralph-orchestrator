//! 2026-07-27-002 plan Unit 3: capability inventory for preset authors/reviewers.
//!
//! Each `Capability` represents a runtime capability that preset authors
//! and reviewers MUST understand to assess whether their preset can
//! legitimately use it. The `covered_in_author_review` field is computed
//! at runtime by checking whether the corresponding reference document
//! contains a stable anchor.

use serde::{Deserialize, Serialize};

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
                "skills/ralph-preset-common/references/finding-rubric.md",
                "crates/ralph-core/data/ralph-tools-wave.md",
            ],
            recommended_evidence_level: "runtime",
            source: "binary_embedded",
        },
        Capability {
            id: "supervisor-emit",
            trigger_signal: "execution_model == supervisor | supervisor+wave",
            applies_when: "preset sets event_loop.supervisor.enabled",
            evidence_sources: &[
                "skills/ralph-preset-common/references/finding-rubric.md",
                "presets/schemas/ce-executor-supervisor.yml",
            ],
            recommended_evidence_level: "runtime",
            source: "binary_embedded",
        },
        Capability {
            id: "task-id-live",
            trigger_signal: "any preset that emits work.done",
            applies_when: "task_id is required for any work.done emit",
            evidence_sources: &[
                "crates/ralph-core/data/ralph-tools-tasks.md",
                "skills/ralph-preset-common/references/commands.md",
            ],
            recommended_evidence_level: "runtime",
            source: "binary_embedded",
        },
        Capability {
            id: "artifact-first",
            trigger_signal: "any preset that emits results",
            applies_when: "presets emit complete results / long content / cross-hat summaries",
            evidence_sources: &[
                "skills/ralph-preset-common/references/agent-native-model.md",
                "skills/ralph-preset-common/references/finding-rubric.md",
            ],
            recommended_evidence_level: "runtime",
            source: "binary_embedded",
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
        },
        Capability {
            id: "trigger-context",
            trigger_signal: "preset uses trigger_context declarations",
            applies_when: "preset declares summary_fields / routing_hints for a trigger topic",
            evidence_sources: &[
                "crates/ralph-core/src/trigger_context.rs",
                "skills/ralph-preset-common/references/commands.md",
            ],
            recommended_evidence_level: "runtime",
            source: "binary_embedded",
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
}
