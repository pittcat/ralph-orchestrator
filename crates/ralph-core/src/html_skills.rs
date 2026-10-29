//! Built-in Ralph HTML deliverable skills (plan 2026-10-29-1053).
//!
//! `skills/ralph-html/` in the repository root is the single maintenance
//! source: a thin Ralph entry skill plus a vendored copy of the upstream
//! Effective HTML collection (upstream commit, verification, and license
//! recorded in `skills/ralph-html/VENDOR.md`). This module embeds that
//! source into the binary and registers each file under a namespaced
//! on-demand name, so loop hats and binary-only installs can load the
//! complete guidance without a user `HOME` or a source checkout.
//!
//! Vendored sibling links (`../<name>/SKILL.md`) and reference links
//! (`references/<file>.md`) cannot resolve once the content is embedded
//! as strings; [`rewrite_links`] converts every known link target into an
//! explicit `` `ralph tools skill load <name>` `` mention. The mapping
//! must stay complete — an unmapped vendored link fails registration
//! rather than silently pointing an agent at a same-named external copy.
//!
//! The names registered here are *controlled*: a user skill scanned from
//! `skills.dirs` that resolves to a controlled name is refused (the
//! embedded entry always wins), and the default hat-visibility list
//! scopes the guidance to deliverable-producing hats. Presets that bind
//! the guidance to differently-named hats extend visibility through
//! `skills.overrides."<name>".hats`.

use crate::skill_registry::SkillRegistry;
use anyhow::{Result, bail};

/// Canonical deliverable-producing hat ids that see the ralph-html
/// family by default (union across embedded presets: reporter/finalizer
/// style final-doc hats, plus common generic role names for file
/// presets). Hats not in this list cannot load the skills in an agent
/// context; a file preset grants visibility to its own hat names via
/// `skills.overrides."ralph-html".hats`.
const DEFAULT_HTML_SKILL_HATS: &[&str] = &[
    // Final-deliverable writers across embedded presets:
    // reporter (ce pipelines, merge-batch, post-merge, parallel-forge,
    // red-team); finalizer + fix-planner + scope-preparer +
    // review-dispatcher + review-synthesizer (implementation-review
    // terminal/blocked artifacts); evaluator + strategist (autoresearch);
    // fixer (debug root-cause note); reviewer (extras/ralph.reviewer);
    // impact-boundary (red-team repair plan).
    "reporter",
    "finalizer",
    "fix-planner",
    "scope-preparer",
    "review-dispatcher",
    "review-synthesizer",
    "evaluator",
    "strategist",
    "fixer",
    "reviewer",
    "impact-boundary",
    // Common deliverable role names for file presets; a file preset with
    // differently-named hats grants visibility via `skills.overrides`.
    "cleaner",
    "failure_handler",
    "planner",
    "author",
    "writer",
    "summarizer",
    "synthesizer",
];

/// One embedded HTML-guidance file and the name it registers under.
struct HtmlSkillSpec {
    /// Namespaced registration name (`ralph tools skill load <name>`).
    name: &'static str,
    /// Embedded maintenance-source content (path under
    /// `skills/ralph-html/`). `include_str!` resolves relative to this
    /// source file, so the repo checkout layout is a compile-time
    /// dependency — the same SSOT pattern as `crates/ralph-core/data/`.
    raw: &'static str,
    /// Index description used when the file carries no frontmatter
    /// description (the vendored reference/*.md files have none).
    description: &'static str,
}

const HTML_SKILLS: &[HtmlSkillSpec] = &[
    HtmlSkillSpec {
        name: "ralph-html",
        raw: include_str!("../../../skills/ralph-html/SKILL.md"),
        description: "",
    },
    HtmlSkillSpec {
        name: "ralph-html-artifact",
        raw: include_str!("../../../skills/ralph-html/vendor/html/SKILL.md"),
        description: "",
    },
    HtmlSkillSpec {
        name: "ralph-html-design",
        raw: include_str!("../../../skills/ralph-html/vendor/design-artifact/SKILL.md"),
        description: "",
    },
    HtmlSkillSpec {
        name: "ralph-html-plan",
        raw: include_str!("../../../skills/ralph-html/vendor/html-plan/SKILL.md"),
        description: "",
    },
    HtmlSkillSpec {
        name: "ralph-html-diagram",
        raw: include_str!("../../../skills/ralph-html/vendor/html-diagram/SKILL.md"),
        description: "",
    },
    HtmlSkillSpec {
        name: "ralph-html-wireframe",
        raw: include_str!("../../../skills/ralph-html/vendor/html-wireframe/SKILL.md"),
        description: "",
    },
    HtmlSkillSpec {
        name: "ralph-html-prototype",
        raw: include_str!("../../../skills/ralph-html/vendor/html-prototype/SKILL.md"),
        description: "",
    },
    HtmlSkillSpec {
        name: "ralph-html-big-pics",
        raw: include_str!("../../../skills/ralph-html/vendor/big-pics-little-words/SKILL.md"),
        description: "",
    },
    HtmlSkillSpec {
        name: "ralph-html-ref-documents-and-presentations",
        raw: include_str!(
            "../../../skills/ralph-html/vendor/html/references/documents-and-presentations.md"
        ),
        description: "Vendored Effective HTML reference: reports, briefs, plans, explainers, comparisons, and slide-like artifacts",
    },
    HtmlSkillSpec {
        name: "ralph-html-ref-charts-and-data",
        raw: include_str!("../../../skills/ralph-html/vendor/html/references/charts-and-data.md"),
        description: "Vendored Effective HTML reference: quantitative charts, tables, metrics, and data stories",
    },
    HtmlSkillSpec {
        name: "ralph-html-ref-diagrams",
        raw: include_str!("../../../skills/ralph-html/vendor/html/references/diagrams.md"),
        description: "Vendored Effective HTML reference: architecture, sequence, state, hierarchy, and concept diagrams",
    },
    HtmlSkillSpec {
        name: "ralph-html-ref-interfaces",
        raw: include_str!("../../../skills/ralph-html/vendor/html/references/interfaces.md"),
        description: "Vendored Effective HTML reference: interfaces, calculators, and tool-like artifacts",
    },
    HtmlSkillSpec {
        name: "ralph-html-ref-creative-direction",
        raw: include_str!(
            "../../../skills/ralph-html/vendor/html/references/creative-direction.md"
        ),
        description: "Vendored Effective HTML reference: fallback creative direction when the design skill is unavailable",
    },
];

/// Names that always resolve to the embedded controlled source. A
/// filesystem skill scanned from `skills.dirs` whose resolved name is
/// controlled is refused, so an unvetted same-named external copy can
/// never replace the vendored guidance.
pub const CONTROLLED_SKILL_NAMES: &[&str] = &[
    "ralph-html",
    "ralph-html-artifact",
    "ralph-html-design",
    "ralph-html-plan",
    "ralph-html-diagram",
    "ralph-html-wireframe",
    "ralph-html-prototype",
    "ralph-html-big-pics",
    "ralph-html-ref-documents-and-presentations",
    "ralph-html-ref-charts-and-data",
    "ralph-html-ref-diagrams",
    "ralph-html-ref-interfaces",
    "ralph-html-ref-creative-direction",
];

/// Whether `name` is a controlled ralph-html registration name.
pub fn is_controlled_name(name: &str) -> bool {
    CONTROLLED_SKILL_NAMES.contains(&name)
}

/// Map a vendored relative link target to its namespaced load name.
///
/// Returns `None` for targets outside the vendored collection so the
/// caller can fail loudly instead of guessing a same-named substitute.
fn load_name_for_target(target: &str) -> Option<&'static str> {
    match target {
        "../html/SKILL.md" => Some("ralph-html-artifact"),
        "../design-artifact/SKILL.md" => Some("ralph-html-design"),
        "../html-plan/SKILL.md" => Some("ralph-html-plan"),
        "../html-diagram/SKILL.md" => Some("ralph-html-diagram"),
        "../html-wireframe/SKILL.md" => Some("ralph-html-wireframe"),
        "../html-prototype/SKILL.md" => Some("ralph-html-prototype"),
        "../big-pics-little-words/SKILL.md" => Some("ralph-html-big-pics"),
        "references/documents-and-presentations.md" => {
            Some("ralph-html-ref-documents-and-presentations")
        }
        "references/charts-and-data.md" => Some("ralph-html-ref-charts-and-data"),
        "references/diagrams.md" => Some("ralph-html-ref-diagrams"),
        "references/interfaces.md" => Some("ralph-html-ref-interfaces"),
        "references/creative-direction.md" => Some("ralph-html-ref-creative-direction"),
        _ => None,
    }
}

/// Rewrite vendored relative links to explicit `ralph tools skill load`
/// mentions.
///
/// Matches `[text](../<sibling>/SKILL.md)` and `[text](references/<file>.md)`
/// and emits `` `{text}` — load via `ralph tools skill load {name}` ``.
/// Any such link whose target is not in [`load_name_for_target`] aborts
/// registration with an error: the mapping table is the contract that
/// keeps embedded guidance from silently resolving against same-named
/// external copies.
fn rewrite_links(skill_name: &str, content: &str) -> Result<String> {
    let mut out = String::with_capacity(content.len() + 512);
    let mut rest = content;
    while let Some(open) = rest.find("](") {
        let (before, after_open) = rest.split_at(open);
        // Find the matching `[` that opens this link text.
        let Some(bracket) = before.rfind('[') else {
            bail!("{skill_name}: link `](` without `[` near byte {}", open);
        };
        let link_text = &before[bracket + 1..];
        let after_paren = &after_open[2..];
        let Some(close) = after_paren.find(')') else {
            bail!("{skill_name}: unterminated link target near byte {}", open);
        };
        let target = &after_paren[..close];
        if !(target.starts_with("../") || target.starts_with("references/")) {
            // External URLs, anchors, and unrelated relative links pass
            // through untouched.
            out.push_str(before);
            out.push_str(&after_open[..2 + close + 1]);
            rest = &after_paren[close + 1..];
            continue;
        }
        let Some(load_name) = load_name_for_target(target) else {
            bail!(
                "{skill_name}: unmapped vendored link target `{target}` — \
                 extend load_name_for_target; never silently substitute an \
                 external copy"
            );
        };
        // Drop the `[` … `](` wrapper and emit an explicit load mention.
        out.push_str(&before[..bracket]);
        out.push_str(link_text);
        out.push_str(" — load via `ralph tools skill load ");
        out.push_str(load_name);
        out.push('`');
        rest = &after_paren[close + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Register every embedded ralph-html skill into `registry`.
///
/// Called from `SkillRegistry::register_builtins`; each entry keeps its
/// frontmatter description when present, carries the canonical
/// deliverable-hat visibility list, and registers under its controlled
/// namespaced name regardless of the vendored frontmatter `name:`.
pub fn register_html_skills(registry: &mut SkillRegistry) -> Result<()> {
    let hats: Vec<String> = DEFAULT_HTML_SKILL_HATS
        .iter()
        .map(|hat| (*hat).to_string())
        .collect();
    for spec in HTML_SKILLS {
        let content = rewrite_links(spec.name, spec.raw)?;
        registry.register_builtin_scoped(
            spec.name,
            &content,
            spec.description,
            hats.clone(),
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::SkillSource;

    fn registry() -> SkillRegistry {
        let mut registry = SkillRegistry::new(None);
        register_html_skills(&mut registry).unwrap();
        registry
    }

    #[test]
    fn registers_every_controlled_name() {
        let registry = registry();
        for name in CONTROLLED_SKILL_NAMES {
            let skill = registry
                .get(name)
                .unwrap_or_else(|| panic!("{name} must be registered"));
            assert!(matches!(skill.source, SkillSource::BuiltIn));
            assert!(!skill.content.trim().is_empty(), "{name} must have content");
        }
    }

    #[test]
    fn registered_names_ignore_vendored_frontmatter_name() {
        // The vendored file declares `name: html`; the builtin must
        // register under the controlled namespaced name only.
        let registry = registry();
        assert!(registry.get("html").is_none());
        assert!(registry.get("design-artifact").is_none());
        assert!(registry.get("ralph-html-artifact").is_some());
    }

    #[test]
    fn rewrite_converts_every_vendored_link() {
        let registry = registry();
        for name in CONTROLLED_SKILL_NAMES {
            let content = &registry.get(name).unwrap().content;
            assert!(
                !content.contains("](../"),
                "{name} still contains a sibling link: {}",
                content
                    .lines()
                    .find(|line| line.contains("](../"))
                    .unwrap_or("")
            );
            assert!(
                !content.contains("](references/"),
                "{name} still contains a references link"
            );
        }
        let artifact = registry.get("ralph-html-artifact").unwrap();
        assert!(artifact.content.contains("ralph tools skill load ralph-html-design"));
        assert!(
            artifact
                .content
                .contains("ralph tools skill load ralph-html-ref-documents-and-presentations")
        );
    }

    #[test]
    fn rewrite_rejects_unmapped_vendored_link() {
        let err = rewrite_links(
            "test-skill",
            "see [`x`](../phantom/SKILL.md) now",
        )
        .unwrap_err();
        assert!(err.to_string().contains("unmapped vendored link target"));
    }

    #[test]
    fn rewrite_passes_through_external_and_anchor_links() {
        let out = rewrite_links(
            "test-skill",
            "[site](https://example.com) and [anchor](#section)",
        )
        .unwrap();
        assert!(out.contains("[site](https://example.com)"));
        assert!(out.contains("[anchor](#section)"));
    }

    #[test]
    fn default_hat_scope_hides_skills_from_unrelated_hats() {
        let registry = registry();
        assert!(registry.is_hat_eligible("ralph-html", "reporter"));
        assert!(!registry.is_hat_eligible("ralph-html", "implementer"));
        assert!(!registry.is_hat_eligible("ralph-html-diagram", "worker"));
        // Human / solo context (no hat) still sees the index.
        assert!(
            registry
                .skills_for_hat(None)
                .iter()
                .any(|s| s.name == "ralph-html")
        );
    }
}
