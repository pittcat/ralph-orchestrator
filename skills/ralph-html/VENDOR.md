# ralph-html vendor provenance

`vendor/` contains a verbatim vendored copy of the **Effective HTML** skill
collection, plus this provenance record. These files carry thin delivery rules
for Ralph's HTML deliverables; they do not encode preset-specific behaviour.

- **Upstream**: <https://github.com/plannotator/effective-html>
- **Upstream commit**: `975b047f6da6874e176db54f048c0c31c30e9f65`
  (2026-10-15, "Add big-pics-little-words skill and website guide")
- **License**: MIT — see `vendor/LICENSE` (required attribution copy).
- **Verification (2026-10-29)**: the installed copies at
  `~/.agents/skills/<name>/` and `~/.claude/skills/<name>/` are byte-identical
  (`diff -r`, no output) to upstream at the commit above; both install trees
  carry the same content, so either counts as the same upstream version.
  This directory is the single maintenance source — do not re-copy from a
  second installed location and create divergent sources.

Do not edit vendored files in place. To update, re-vendor a newer upstream
commit (verify `diff -r` against a trusted clone first) and update the commit
hash and verification note above.

## File inventory

| Vendored path | Upstream path | Purpose |
|---|---|---|
| `vendor/html/SKILL.md` | `skills/html/SKILL.md` | General artifact workflow / implicit router — reports, explainers, mixed artifacts |
| `vendor/html/references/documents-and-presentations.md` | `skills/html/references/documents-and-presentations.md` | Documents/decks layout guidance (report bindings) |
| `vendor/html/references/charts-and-data.md` | `skills/html/references/charts-and-data.md` | Quantitative charts, tables, metrics |
| `vendor/html/references/diagrams.md` | `skills/html/references/diagrams.md` | Architecture/sequence/state/hierarchy figures |
| `vendor/html/references/interfaces.md` | `skills/html/references/interfaces.md` | Tool/interface guidance |
| `vendor/html/references/creative-direction.md` | `skills/html/references/creative-direction.md` | Fallback creative direction when `design-artifact` is absent |
| `vendor/design-artifact/SKILL.md` | `skills/design-artifact/SKILL.md` | Visual register: palette, type, composition, accessibility |
| `vendor/html-plan/SKILL.md` | `skills/html-plan/SKILL.md` | Plan/roadmap artifacts preserving source commitments |
| `vendor/html-diagram/SKILL.md` | `skills/html-diagram/SKILL.md` | Diagram-as-deliverable specialist |
| `vendor/html-wireframe/SKILL.md` | `skills/html-wireframe/SKILL.md` | Low-fidelity layout directions |
| `vendor/html-prototype/SKILL.md` | `skills/html-prototype/SKILL.md` | Mockups and working interactive flows |
| `vendor/big-pics-little-words/SKILL.md` | `skills/big-pics-little-words/SKILL.md` | Visual explainer with minimal prose |
| `vendor/*/agents/openai.yaml` | `skills/*/agents/openai.yaml` | Upstream OpenAI-agent metadata (inert for Ralph; kept verbatim) |
| `vendor/LICENSE` | `LICENSE` | MIT license text (attribution requirement) |

## Loop-side load names

`crates/ralph-core/src/html_skills.rs` embeds this maintenance source into the
binary and registers each file under a namespaced on-demand name. At embed time
the sibling/reference links inside vendored content are rewritten to these
explicit load names — the registry never silently adopts a same-named external
copy, and controlled names refuse replacement by user-scanned skills.

| `ralph tools skill load` name | Source file |
|---|---|
| `ralph-html` | `skills/ralph-html/SKILL.md` |
| `ralph-html-artifact` | `skills/ralph-html/vendor/html/SKILL.md` |
| `ralph-html-design` | `skills/ralph-html/vendor/design-artifact/SKILL.md` |
| `ralph-html-plan` | `skills/ralph-html/vendor/html-plan/SKILL.md` |
| `ralph-html-diagram` | `skills/ralph-html/vendor/html-diagram/SKILL.md` |
| `ralph-html-wireframe` | `skills/ralph-html/vendor/html-wireframe/SKILL.md` |
| `ralph-html-prototype` | `skills/ralph-html/vendor/html-prototype/SKILL.md` |
| `ralph-html-big-pics` | `skills/ralph-html/vendor/big-pics-little-words/SKILL.md` |
| `ralph-html-ref-documents-and-presentations` | `skills/ralph-html/vendor/html/references/documents-and-presentations.md` |
| `ralph-html-ref-charts-and-data` | `skills/ralph-html/vendor/html/references/charts-and-data.md` |
| `ralph-html-ref-diagrams` | `skills/ralph-html/vendor/html/references/diagrams.md` |
| `ralph-html-ref-interfaces` | `skills/ralph-html/vendor/html/references/interfaces.md` |
| `ralph-html-ref-creative-direction` | `skills/ralph-html/vendor/html/references/creative-direction.md` |

## Operator install

`skills/install.py` installs the whole `ralph-html/` directory (entry SKILL.md,
VENDOR.md, and the complete `vendor/` tree) as one public skill unit. The
install target is whatever the installer's target configuration selects —
`.claude/skills/`, `.agents/skills/`, or a custom `--dir`; never assume a user
`HOME` path exists. Operator-side reads resolve vendored sibling links
(`../<name>/SKILL.md`, `references/<file>.md`) on disk exactly as upstream
wrote them.
