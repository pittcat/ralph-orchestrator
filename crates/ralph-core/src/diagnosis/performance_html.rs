//! Standalone offline HTML renderer for [`PerformanceReport`]
//! (plan 2026-10-29-0104 U8 / R9-R12).
//!
//! Contract:
//! - fully self-contained: inline CSS + a small amount of inline
//!   vanilla JS for interaction (filter/toggle). No external
//!   resources, no network, no `eval`, no artifact-provided code
//!   executed (R12);
//! - every dynamic string is HTML-escaped before interpolation;
//! - secrets are never rendered: `job_token`, raw prompts and raw
//!   tool output are not part of the report model and must not be
//!   added to `attributes` upstream;
//! - unknown/censored cells render with a distinct `data-unknown`
//!   style rather than fabricated numbers (R10).

use std::fmt::Write as _;

use super::performance::{PerformanceReport, SpanView};
use crate::diagnostics::{IdentityValue, SpanKind, TimingCaptureStatus};

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn ident_str(v: &IdentityValue) -> String {
    match v {
        IdentityValue::Missing => String::new(),
        IdentityValue::NotApplicable => "n/a".to_string(),
        IdentityValue::Value(s) => esc(s),
    }
}

fn fmt_ms(ms: u64) -> String {
    if ms >= 60_000 {
        format!("{}m {:02}s", ms / 60_000, (ms % 60_000) / 1000)
    } else if ms >= 1000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{ms}ms")
    }
}

fn opt_ms(ms: Option<u64>) -> String {
    ms.map(fmt_ms).unwrap_or_else(|| "—".to_string())
}

/// Render the standalone HTML report for `report`.
pub fn render_html(report: &PerformanceReport) -> String {
    let mut html = String::with_capacity(32 * 1024);
    html.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    let _ = writeln!(
        html,
        "<title>Ralph Run Analysis — {}</title>",
        esc(&report.session_dir)
    );
    html.push_str(STYLE);
    html.push_str("</head>\n<body>\n<main class=\"page\">\n");

    // ── Header ───────────────────────────────────────────────────
    html.push_str("<header class=\"hero\">\n<h1>Run Analysis</h1>\n");
    let _ = writeln!(
        html,
        "<p class=\"subtitle\">session <code>{}</code> · generated {} · schema {}</p>",
        esc(&report.session_dir),
        esc(&report.generated_at),
        esc(&report.schema_version)
    );
    html.push_str("<div class=\"stat-grid\">\n");
    stat(&mut html, "Wall clock", &opt_ms(report.wall_clock_ms), "");
    stat(
        &mut html,
        "Cumulative work",
        &fmt_ms(report.cumulative_work_ms),
        "",
    );
    stat(
        &mut html,
        "Span rows",
        &report.capture.span_rows.to_string(),
        "",
    );
    let cap_label = match report.capture.status {
        TimingCaptureStatus::Collected => "collected",
        TimingCaptureStatus::Degraded => "degraded",
        TimingCaptureStatus::NotCollected => "not collected",
    };
    let cap_class = match report.capture.status {
        TimingCaptureStatus::Collected => "ok",
        TimingCaptureStatus::Degraded => "warn",
        TimingCaptureStatus::NotCollected => "unknown",
    };
    stat(&mut html, "Capture", cap_label, cap_class);
    html.push_str("</div>\n</header>\n");

    // ── Evidence gaps ────────────────────────────────────────────
    if !report.capture.gaps.is_empty() {
        html.push_str("<section class=\"card gaps\">\n<h2>Evidence gaps</h2>\n<ul>\n");
        for gap in &report.capture.gaps {
            let _ = writeln!(html, "<li class=\"gap\">{}</li>", esc(gap));
        }
        html.push_str("</ul>\n</section>\n");
    }

    // ── Controls ─────────────────────────────────────────────────
    html.push_str(
        "<section class=\"toolbar\">\n\
         <input id=\"q\" type=\"search\" placeholder=\"Filter by hat / unit / span…\" \
         oninput=\"filterRows()\" aria-label=\"filter\">\n\
         <label class=\"pct\"><input id=\"pct\" type=\"checkbox\" onchange=\"renderPercents()\"> \
         show % of work</label>\n\
         </section>\n",
    );

    // ── By-kind table (heatmap) ──────────────────────────────────
    html.push_str("<section class=\"card\">\n<h2>Time by stage kind</h2>\n");
    html.push_str(
        "<table class=\"grid\" id=\"kind-table\">\n<thead><tr>\
        <th>Kind</th><th>Spans</th><th>Censored</th><th>Total</th><th class=\"pctcol\">%</th>\
        </tr></thead>\n<tbody>\n",
    );
    let max_total = report
        .by_kind
        .iter()
        .map(|k| k.total_ms)
        .max()
        .unwrap_or(1)
        .max(1);
    for k in &report.by_kind {
        let kind = k.kind.map(SpanKind::as_str).unwrap_or("other");
        let heat = (k.total_ms as f64 / max_total as f64 * 100.0).round() as u32;
        let pct = if report.cumulative_work_ms > 0 && k.kind.map_or(false, SpanKind::is_work) {
            format!(
                "{:.0}%",
                k.total_ms as f64 / report.cumulative_work_ms as f64 * 100.0
            )
        } else {
            "—".to_string()
        };
        let _ = writeln!(
            html,
            "<tr data-search=\"{}\">\
             <td><span class=\"pill kind-{}\">{}</span></td>\
             <td>{}</td>\
             <td class=\"{}\">{}</td>\
             <td data-ms=\"{}\"><span class=\"bar\" style=\"width:{}%\"></span>{}</td>\
             <td class=\"pctcol\">{}</td></tr>",
            esc(kind),
            esc(kind),
            esc(kind),
            k.count,
            if k.censored > 0 { "unknown" } else { "num" },
            k.censored,
            k.total_ms,
            heat,
            fmt_ms(k.total_ms),
            pct,
        );
    }
    html.push_str("</tbody>\n</table>\n</section>\n");

    // ── Timeline ─────────────────────────────────────────────────
    html.push_str("<section class=\"card\">\n<h2>Timeline</h2>\n");
    render_timeline(&mut html, report);
    html.push_str("</section>\n");

    // ── Slowest spans ────────────────────────────────────────────
    html.push_str("<section class=\"card\">\n<h2>Slowest observed spans</h2>\n");
    html.push_str(
        "<p class=\"note\">Observed facts only — a slow span is a candidate for \
         investigation, not a verified root cause (plan R7/R8).</p>\n",
    );
    html.push_str(
        "<table class=\"grid\" id=\"spans-table\">\n<thead><tr>\
        <th>Span</th><th>Kind</th><th>Status</th><th>Hat</th><th>Unit</th><th>Attempt</th>\
        <th>Duration</th><th>Start (UTC)</th></tr></thead>\n<tbody>\n",
    );
    for s in &report.spans {
        row(&mut html, s);
    }
    html.push_str("</tbody>\n</table>\n</section>\n");

    // ── Bottleneck candidates (U5) ────────────────────────────────
    if !report.bottlenecks.is_empty() {
        html.push_str("<section class=\"card\">\n<h2>Bottleneck candidates</h2>\n");
        html.push_str(
            "<p class=\"note\">Directions backed by evidence or explicit gaps — \
             not verified root causes.</p>\n<ol class=\"bottleneck-list\">\n",
        );
        for b in &report.bottlenecks {
            let kind = format!("{:?}", b.kind).to_lowercase();
            let _ = writeln!(
                html,
                "<li class=\"bottleneck\"><span class=\"pill kind-{}\">{}</span> {}",
                esc(&kind),
                esc(&kind.replace('_', "-")),
                esc(&b.finding)
            );
            if let Some(pct) = b.wall_clock_share_pct {
                let _ = writeln!(html, "<span class=\"share\">{}% of wall clock</span>", pct);
            }
            if !b.evidence.is_empty() {
                html.push_str("<ul class=\"evidence\">\n");
                for e in &b.evidence {
                    let where_ = e
                        .span_id
                        .as_deref()
                        .map(|id| format!("{:?}#{}", e.artifact, id))
                        .unwrap_or_else(|| format!("{:?}", e.artifact));
                    let _ = writeln!(
                        html,
                        "<li>{} <code>{}</code></li>",
                        esc(&e.note),
                        esc(&where_.to_lowercase())
                    );
                }
                html.push_str("</ul>\n");
            }
            if let Some(rec) = &b.recommendation {
                let _ = writeln!(html, "<p class=\"direction\">Direction: {}</p>", esc(rec));
            }
            if let Some(gap) = &b.gap {
                let _ = writeln!(html, "<p class=\"gap-note\">Gap: {}</p>", esc(gap));
            }
            html.push_str("</li>\n");
        }
        html.push_str("</ol>\n</section>\n");
    }

    html.push_str(
        "<footer class=\"foot\">Offline report — all data embedded; \
        JS is interaction-only (no eval, no network, no artifact code execution).</footer>\n",
    );
    html.push_str("</main>\n");
    html.push_str(SCRIPT);
    html.push_str("</body>\n</html>\n");
    html
}

fn stat(html: &mut String, label: &str, value: &str, class: &str) {
    let _ = writeln!(
        html,
        "<div class=\"stat {}\"><span class=\"stat-label\">{}</span>\
         <span class=\"stat-value\">{}</span></div>",
        class, label, value
    );
}

fn row(html: &mut String, s: &SpanView) {
    let status = match s.status {
        crate::diagnostics::SpanStatus::Completed => "completed",
        crate::diagnostics::SpanStatus::Interrupted => "interrupted",
        crate::diagnostics::SpanStatus::Invalid => "invalid",
        crate::diagnostics::SpanStatus::Open => "open",
    };
    let (dur, dur_class) = match s.duration_ms {
        Some(d) => (fmt_ms(d), "num"),
        None => ("censored".to_string(), "unknown"),
    };
    let search = format!(
        "{} {} {} {} {}",
        s.span_id,
        s.kind.as_str(),
        s.identity.hat.as_deref().unwrap_or(""),
        s.identity.unit_key.as_str().unwrap_or(""),
        s.identity.job_id.as_deref().unwrap_or("")
    );
    let _ = writeln!(
        html,
        "<tr data-search=\"{}\">\
         <td><code>{}</code></td>\
         <td><span class=\"pill kind-{}\">{}</span></td>\
         <td class=\"st-{}\">{}</td>\
         <td>{}</td>\
         <td>{}</td>\
         <td class=\"{}\">{}</td>\
         <td class=\"{}\" data-ms=\"{}\">{}</td>\
         <td class=\"ts\">{}</td></tr>",
        esc(&search),
        esc(&s.span_id),
        s.kind.as_str(),
        s.kind.as_str(),
        status,
        status,
        esc(s.identity.hat.as_deref().unwrap_or("")),
        ident_str(&s.identity.unit_key),
        if s.identity.attempt.is_some() {
            "num"
        } else {
            "unknown"
        },
        s.identity
            .attempt
            .map(|a| a.to_string())
            .unwrap_or_else(|| "—".to_string()),
        dur_class,
        s.duration_ms.unwrap_or(0),
        dur,
        esc(&s.ts_start_utc),
    );
}

fn render_timeline(html: &mut String, report: &PerformanceReport) {
    let (Some(min), Some(wall)) = (
        report
            .spans
            .iter()
            .filter_map(|s| chrono::DateTime::parse_from_rfc3339(&s.ts_start_utc).ok())
            .map(|d| d.timestamp_millis())
            .min(),
        report.wall_clock_ms,
    ) else {
        html.push_str("<p class=\"unknown\">No time-bounded spans to chart.</p>\n");
        return;
    };
    let wall = (wall as i64).max(1);
    html.push_str("<div class=\"tl\">\n");
    for s in &report.spans {
        let Ok(start) = chrono::DateTime::parse_from_rfc3339(&s.ts_start_utc) else {
            continue;
        };
        let start_ms = start.timestamp_millis() - min;
        let dur = s
            .duration_ms
            .map(|d| d as i64)
            .unwrap_or_else(|| (wall / 40).max(1)); // censored: thin stub
        let left = (start_ms * 100 / wall).clamp(0, 100);
        let width = (dur * 100 / wall).clamp(0, 100 - left).max(1);
        let censored = s.duration_ms.is_none();
        let _ = writeln!(
            html,
            "<div class=\"tl-row\" data-search=\"{}\">\
             <span class=\"tl-label\"><code>{}</code></span>\
             <span class=\"tl-track\"><span class=\"tl-bar kind-{}{}\" \
             style=\"left:{}%;width:{}%\" title=\"{}\"></span></span></div>",
            esc(&s.span_id),
            esc(&s.span_id),
            s.kind.as_str(),
            if censored { " tl-censored" } else { "" },
            left,
            width,
            esc(&format!("{} · {}", s.span_id, opt_ms(s.duration_ms))),
        );
    }
    html.push_str("</div>\n");
}

const STYLE: &str = r##"<style>
:root{--bg:#0f1420;--panel:#171e2e;--ink:#e8ecf4;--muted:#8b95ab;--line:#2a3350;
--ok:#34d399;--warn:#fbbf24;--unk:#64748b;--acc:#60a5fa}
*{box-sizing:border-box}body{margin:0;background:linear-gradient(180deg,#0c1018,#0f1420);
color:var(--ink);font:15px/1.55 -apple-system,"SF Pro","Segoe UI",system-ui,sans-serif}
.page{max-width:1080px;margin:0 auto;padding:32px 20px 64px}
.hero h1{margin:0;font-size:28px;letter-spacing:.4px}
.subtitle{color:var(--muted);margin:6px 0 18px}
code{background:#0b0f18;padding:1px 6px;border-radius:5px;font-size:.9em;color:#a5c4ff}
.stat-grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(160px,1fr));gap:12px}
.stat{background:var(--panel);border:1px solid var(--line);border-radius:12px;padding:14px 16px}
.stat-label{display:block;color:var(--muted);font-size:12px;text-transform:uppercase;letter-spacing:.8px}
.stat-value{font-size:22px;font-weight:650;margin-top:4px;display:block}
.stat.ok .stat-value{color:var(--ok)}.stat.warn .stat-value{color:var(--warn)}
.stat.unknown .stat-value{color:var(--unk)}
.card{background:var(--panel);border:1px solid var(--line);border-radius:14px;
padding:18px 20px;margin:18px 0}
.card h2{margin:0 0 12px;font-size:17px}
.gaps{border-color:#5a4a1e}.gap{color:var(--warn)}
.toolbar{display:flex;gap:14px;align-items:center;margin:18px 0}
.toolbar input[type=search]{flex:1;background:#0b0f18;border:1px solid var(--line);
border-radius:10px;color:var(--ink);padding:9px 12px;font-size:14px}
.toolbar .pct{color:var(--muted);font-size:13px;white-space:nowrap}
table.grid{width:100%;border-collapse:collapse;font-size:13.5px}
.grid th{text-align:left;color:var(--muted);font-weight:600;font-size:12px;
text-transform:uppercase;letter-spacing:.6px;padding:6px 8px;border-bottom:1px solid var(--line)}
.grid td{padding:7px 8px;border-bottom:1px solid #202943}
.grid tr:hover td{background:#1b2338}
.num{font-variant-numeric:tabular-nums}
.unknown{color:var(--unk);font-style:italic}
.ts{color:var(--muted);font-size:12px}
.pill{display:inline-block;padding:2px 9px;border-radius:999px;font-size:11.5px;
font-weight:600;letter-spacing:.3px;background:#26304d}
.kind-agent_exec{background:#3b2f63;color:#c4b5fd}
.kind-tool_exec{background:#1e3a4d;color:#7dd3fc}
.kind-dependency_wait,.kind-capacity_wait{background:#4d3a1e;color:#fbbf24}
.kind-review_verify{background:#1e4d3a;color:#6ee7b7}
.kind-startup{background:#26304d;color:#93c5fd}
.kind-integration{background:#4d1e3a;color:#f9a8d4}
.kind-memory_selection{background:#3a4d1e;color:#bef264}
.st-completed{color:var(--ok)}.st-interrupted{color:var(--warn)}
.st-invalid,.st-open{color:var(--unk)}
.bar{display:inline-block;height:8px;border-radius:4px;background:var(--acc);
margin-right:8px;vertical-align:middle;opacity:.85;min-width:2px}
.note{color:var(--muted);font-size:13px;margin:0 0 10px}
.bottleneck-list{padding-left:22px}
.bottleneck{margin:10px 0;padding:10px 12px;background:#141b2c;border-radius:10px;
border:1px solid #202943;list-style-position:inside}
.bottleneck .share{color:var(--muted);font-size:12.5px;margin-left:8px}
.bottleneck .evidence{margin:8px 0 2px;padding-left:16px;color:var(--muted);font-size:12.5px}
.bottleneck .evidence code{color:#93c5fd}
.bottleneck .direction{color:#6ee7b7;font-size:13px;margin:6px 0 0}
.bottleneck .gap-note{color:var(--warn);font-size:13px;margin:6px 0 0}
.kind-slow_stage{background:#4d1e2b;color:#fda4af}
.kind-wait_release{background:#4d3a1e;color:#fbbf24}
.kind-coverage_gap{background:#3d2a4d;color:#d8b4fe}
.tl-row{display:flex;align-items:center;gap:10px;padding:3px 0}
.tl-label{width:170px;flex:none;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:12px}
.tl-track{position:relative;flex:1;height:14px;background:#0b0f18;border-radius:7px}
.tl-bar{position:absolute;top:2px;height:10px;border-radius:5px;background:var(--acc)}
.tl-censored{background:repeating-linear-gradient(45deg,var(--unk),var(--unk) 4px,transparent 4px,transparent 8px)}
.foot{color:var(--muted);font-size:12px;text-align:center;margin-top:26px}
@media(max-width:640px){.tl-label{width:110px}}
</style>
"##;

const SCRIPT: &str = r##"<script>
// Interaction-only: no eval, no network, no artifact-provided code.
function filterRows(){
  var q=document.getElementById('q').value.toLowerCase();
  document.querySelectorAll('[data-search]').forEach(function(r){
    r.style.display=r.getAttribute('data-search').toLowerCase().indexOf(q)>=0?'':'none';
  });
}
function renderPercents(){
  // Percent values are precomputed server-side; the toggle switches the
  // "%" column emphasis only — numbers never recomputed by the page.
  var on=document.getElementById('pct').checked;
  document.querySelectorAll('.pctcol').forEach(function(c){
    c.style.opacity=on?'1':'0.35';
  });
}
</script>
"##;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::{ClockEpoch, ExecutionIdentity, SpanStatus, TimingSpan};

    fn fixture() -> PerformanceReport {
        let epoch = ClockEpoch::new();
        let mut span = TimingSpan::open(
            "sp-a",
            ExecutionIdentity::for_hat("executor"),
            SpanKind::AgentExec,
            &epoch,
        );
        span.complete(&epoch);
        PerformanceReport {
            schema_version: "run-analysis-performance/v1".into(),
            generated_at: "2026-10-29T01:04:00Z".into(),
            session_dir: "/tmp/sess-1".into(),
            capture: super::super::performance::CaptureCoverage {
                status: TimingCaptureStatus::Collected,
                span_rows: 2,
                malformed_rows: 0,
                censored_spans: 0,
                gaps: vec![],
            },
            by_kind: vec![super::super::performance::KindAggregate {
                kind: Some(SpanKind::AgentExec),
                count: 1,
                total_ms: span.duration_ms().unwrap_or(0),
                censored: 0,
            }],
            slowest_spans: vec![],
            spans: vec![SpanView {
                span_id: "sp-a".into(),
                parent_span_id: None,
                kind: SpanKind::AgentExec,
                status: SpanStatus::Completed,
                identity: ExecutionIdentity::for_hat("executor"),
                ts_start_utc: span.ts_start_utc.clone(),
                ts_end_utc: span.ts_end_utc.clone(),
                duration_ms: span.duration_ms(),
                clock_epoch: epoch.id().to_string(),
                attributes: None,
                rows: 2,
            }],
            wall_clock_ms: span.duration_ms(),
            cumulative_work_ms: span.duration_ms().unwrap_or(0),
            bottlenecks: vec![],
        }
    }

    #[test]
    fn html_is_standalone_and_escapes() {
        let mut report = fixture();
        report.session_dir = "<script>alert(1)</script>".into();
        let html = render_html(&report);
        assert!(html.contains("<style>"));
        assert!(html.contains("<script>"));
        // The injected script text must be escaped, not embedded.
        assert!(!html.contains("<script>alert(1)</script>"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(!html.contains("eval("));
        assert!(html.contains("executor"));
    }
}
