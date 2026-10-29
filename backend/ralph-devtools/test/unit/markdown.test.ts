import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { renderRunReport, renderCompareMarkdown } from '../../src/report/markdown.js';
import { deriveTimeline } from '../../src/report/timeline.js';
import { compareReports } from '../../src/report/compare.js';
import type { DiagnoseReportLike } from '../../src/report/classify.js';

const diagnose: DiagnoseReportLike = {
  schema_version: '1',
  session_path: '/ws/.ralph/diagnostics/S1',
  top_findings: [
    {
      retry_key: 'k1', severity: 'error', source: 'policy',
      reason_code: 'payload_contract_violation', topic: 'work.done',
      target_hat: 'ex', occurrences: 3, escalated: true, message: 'missing field',
    },
  ],
  recovery_timeline: [
    { iteration: 3, hat: 'ex', reason_code: 'payload_contract_violation', topic: 'work.done', message: 'a' },
    { iteration: 4, hat: 'ex', reason_code: 'payload_contract_violation', topic: 'work.done', message: 'b' },
  ],
  dup_storm_topics: ['work.ready'],
  drift_findings: [],
  repair_suggestions: [{ title: 'fix contract', layer: 'preset' }],
  warnings: [],
};

describe('renderRunReport', () => {
  it('renders all major sections', () => {
    const tl = deriveTimeline([
      { ts: '2026-01-01T00:00:00Z', iteration: 0, hat: 'a', topic: 'start', payload: '' },
      { ts: '2026-01-01T00:01:00Z', iteration: 1, hat: 'b', topic: 'work.done', payload: '' },
    ]);
    const md = renderRunReport({
      generatedAt: '2026-01-02T00:00:00Z',
      workspace: '/ws',
      diagnose,
      timeline: tl,
    });
    assert.match(md, /# Ralph Run Report/);
    assert.match(md, /Failure classification/);
    assert.match(md, /payload_contract/);
    assert.match(md, /Repeated failures/);
    assert.match(md, /Timeline/);
    assert.match(md, /dup_storm_topics.*work\.ready/);
    assert.match(md, /Repair suggestions/);
  });

  it('degrades gracefully without inputs', () => {
    const md = renderRunReport({
      generatedAt: 'x',
      diagnoseError: 'no session found',
      eventsError: 'no events file',
    });
    assert.match(md, /diagnose unavailable/);
    assert.match(md, /events unavailable/);
  });
});

describe('renderCompareMarkdown', () => {
  it('renders status table + dup storm diff', () => {
    const cmp = compareReports(diagnose, {
      ...diagnose,
      session_path: '/ws/.ralph/diagnostics/S2',
      top_findings: [
        {
          retry_key: 'k1', severity: 'error', source: 'policy',
          reason_code: 'payload_contract_violation', topic: 'work.done',
          target_hat: 'ex', occurrences: 3, message: 'missing field',
        },
      ],
      dup_storm_topics: [],
    });
    const md = renderCompareMarkdown(cmp);
    assert.match(md, /# Ralph Run Comparison/);
    assert.match(md, /resolved dup storms.*work\.ready/);
  });
});
