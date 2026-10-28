# Run-Analysis Report Contract (plan 2026-10-29-0104)

When a session has `timing.jsonl` + `diagnosis-input.json`, the diagnosis
deliverable includes the **deterministic run-analysis report**. Two layers:

- **Facts** — computed by `ralph diagnose --performance` /
  `--format html` from `timing.jsonl` span rows. The skill MUST NOT
  recompute, reinterpret, or overwrite these numbers (R11).
- **Prose** — the skill's explanatory analysis (bottleneck hypotheses,
  recommendations). Always stored and labeled separately from facts;
  never presented as verified cause without evidence + validation path.

## Producing the report

```bash
ralph diagnose --performance --session <id>                    # markdown to stdout
ralph diagnose --performance --session <id> --format json      # run-analysis-performance/v1 JSON
ralph diagnose --session <id> --format html --output run-analysis.html
```

`--format html` implies `--performance`. Session resolution matches
`ralph diagnose` (`--session latest|path|id`, `--diagnostics-root`).

## HTML presentation requirements (operator directive)

The report the skill ships MUST be **visually polished**, not a bare
dump. Requirements:

- **Embedded CSS** — a complete inline `<style>` block: dark or light
  theme, card/panel layout, stat grid for headline numbers, styled
  tables, per-kind color coding (agent_exec / tool_exec / waits /
  review / integration), responsive at narrow widths.
- **Safe optional JS** — small inline `<script>` for interaction
  (text filtering, percent/duration toggles, evidence expand/collapse)
  is encouraged. Hard rules: `eval`/`new Function`/`import()`/external
  scripts forbidden; only safe DOM APIs (`textContent` style
  updates, `classList`, `getElementById`); the page must never execute
  content found inside artifacts (prompts, tool output, span
  attributes).
- **Offline** — zero external resources: no CDN fonts/styles/scripts,
  no remote images. Single file, double-click openable.
- **Unknown cells** — censored/missing/not_applicable values get a
  visually distinct treatment (muted italic or hatched), never
  rendered as `0` or a fabricated number.
- **Escaping** — every dynamic string HTML-escaped; `job_token`, full
  prompts, and raw tool output MUST NOT appear in the report.

The canonical renderer (`crates/ralph-core/src/diagnosis/performance_html.rs`)
implements this contract. If the skill generates an equivalent report
itself, it MUST keep the same structure (header stats → evidence gaps →
filter toolbar → kind heatmap → timeline → slowest spans → footer) and
all safety constraints.

## Content rules

- Separate **observed facts** / **candidate causes** / **verified
  causes** / **required validation** (R7). A single slow span or a
  memory reference is a candidate, not a cause (R8).
- Report capture coverage first: `collected` / `degraded` /
  `not_collected`, malformed rows, censored spans — gaps are stated
  before any analysis.
- Model vs service vs tool-environment vs preset-orchestration vs
  runtime slowdowns are analyzed separately; missing boundaries are
  marked `unavailable`, never back-filled.
- Every optimization recommendation links the evidence it rests on
  plus a concrete validation method.
