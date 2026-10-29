---
name: ralph-html
description: Ralph HTML deliverable entry point — binding rules, required reading per deliverable kind, the offline single-file delivery contract, and failure handling for loop hats and operator skills that ship a human-readable final artifact. Load this first whenever your instructions bind an HTML deliverable.
metadata:
  internal: true
---

# Ralph HTML Deliverables

This skill is the Ralph entry point for every HTML deliverable. It defines who may do what, the required reading per deliverable kind, the offline delivery contract, and what to do when guidance or verification is unavailable.

## Authority and order of precedence

HTML guidance controls expression and delivery only. Evidence, verdicts, scores, topology decisions, and emitted events stay owned by the business hat or operator skill that produced them — this skill never starts another workflow, reviews evidence, or emits business events. When rules conflict, apply them in this order:

1. The user's explicit requirements and the project's hard rules.
2. The business contract: hat instructions, operator-skill deliverable sections, payload schemas, and content templates.
3. This delivery contract: offline readability, single file, honest content.
4. Specialist guidance: the report / plan / diagram / wireframe / prototype workflows bound below.
5. Visual guidance: palette, type, composition, and motion advice.

The original content template becomes the content checklist; page layout follows from that content. Never let visual guidance delete findings, evidence, open questions, or blocked-state detail — a polished page that drops uncertainty is a contract violation.

## Required reading by deliverable kind

Load this entry skill first, then the rows your binding lists. Inside a Ralph loop use `ralph tools skill load <name>`; operator-side, read the installed files under the bound `ralph-html/vendor/` directory directly (see `VENDOR.md` for the full name-to-file map). Do not substitute same-named skills from other locations for the names below.

| Deliverable kind | Required loads | Conditional loads |
|---|---|---|
| Report — run report, diagnosis, review report, settlement, manager report | `ralph-html-artifact`, `ralph-html-design`, `ralph-html-ref-documents-and-presentations` | `ralph-html-ref-charts-and-data` when the artifact compares metrics or quantitative data; `ralph-html-ref-diagrams` when a topology/sequence/causality figure materially helps; `ralph-html-diagram` when the diagram is itself the deliverable; `ralph-html-big-pics` only when a visual explainer was explicitly requested |
| Plan — task-brief reading view, author notes, execution or rollout plan | `ralph-html-plan`, `ralph-html-design` | same conditional rows as reports |
| Interface structure or interaction flow | `ralph-html-wireframe` or `ralph-html-prototype`, plus `ralph-html-design` | only when the deliverable itself explores UI structure or a working flow — never load both speculatively |

## Delivery contract

- One self-contained `.html` file: essential CSS and JavaScript inline, opens directly with no build step and no network dependency (no CDN fonts, scripts, or images).
- Real content lives in semantic HTML in the document body — every conclusion, piece of evidence, finding, and unresolved item required by the business contract must appear. Do not park required content in JavaScript data blobs, comments, or external files.
- Escape all interpolated business text (log lines, payloads, code, stack traces) as text; never inject it as markup.
- Provide a working table of contents or anchor navigation when the document has multiple sections; interactive controls must be keyboard-usable, and essential content must not sit behind hover or collapsed state that print cannot reveal.
- Readable at narrow and wide viewports and in print: responsive layout, sensible print styles, no accidental horizontal overflow.
- Machine contracts stay untouched: YAML briefs, JSON payloads, schemas, and internal Markdown evidence keep their existing formats and paths. This skill adds a human-readable HTML surface; it does not convert machine handoff artifacts.

## Runtime order

1. Finish the business work and collect the evidence the business contract requires.
2. Load the bound guidance: this entry skill, the required rows, and any conditional rows that genuinely apply.
3. Write the HTML file to the exact path the business contract declares.
4. Run the delivery-contract checklist against the file, fix what fails, then deliver through the original path — the terminal payload's `report_path` / `artifact_path` field plus the `DELIVERABLE_PATH:` line inside a loop, or the operator skill's own handoff convention outside a loop.

## Verification and failure handling

Before claiming delivery, check: every required finding and section present; business text escaped; anchors and controls work; nothing essential hidden behind interaction; narrow-viewport and print readability acceptable; browser console clean when scripts exist.

- If a bound skill cannot be loaded — missing name, hat-visibility refusal, or truncated content — do not claim the deliverable is done and do not silently fall back to a same-named skill from another source. Report the loading failure through the business entry's own failure or blocked path.
- If no browser is available for visual verification, keep the HTML draft and state plainly that visual verification was not performed. Where the business contract requires a verified render, report the environment gap through its failure or stop path instead of claiming completion.

## Scope boundaries

This skill ships one human-readable HTML surface per bound entry. It does not convert internal Markdown evidence, execution plans, YAML/JSON machine contracts, or historical reports, and it introduces no design hat, rendering platform, or public publishing flow.
