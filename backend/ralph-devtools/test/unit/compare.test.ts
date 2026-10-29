import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { compareReports } from '../../src/report/compare.js';
import type { DiagnoseReportLike } from '../../src/report/classify.js';

const a: DiagnoseReportLike = {
  schema_version: '1',
  session_path: '/A',
  top_findings: [
    {
      retry_key: 'k1', severity: 'warn', source: 'policy',
      reason_code: 'payload_contract_violation', topic: 'work.done',
      target_hat: 'ex', occurrences: 2, last_iteration: 4, message: 'm1',
    },
    {
      retry_key: 'k2', severity: 'error', source: 'origin',
      reason_code: 'isolated_scope_violation', topic: 'review.done',
      target_hat: 'rev', occurrences: 1, last_iteration: 5, message: 'm2',
    },
  ],
  dup_storm_topics: ['work.ready'],
};

const b: DiagnoseReportLike = {
  schema_version: '1',
  session_path: '/B',
  top_findings: [
    {
      retry_key: 'k1', severity: 'error', source: 'policy',
      reason_code: 'payload_contract_violation', topic: 'work.done',
      target_hat: 'ex', occurrences: 7, last_iteration: 9, escalated: true, message: 'm1',
    },
    {
      retry_key: 'k3', severity: 'warn', source: 'wave',
      reason_code: 'wave_partial_threshold', topic: 'review.file',
      target_hat: 'agg', occurrences: 4, last_iteration: 9, message: 'm3',
    },
  ],
  dup_storm_topics: ['work.ready', 'review.file'],
};

describe('compareReports', () => {
  it('classifies escalated/new/resolved', () => {
    const cmp = compareReports(a, b);
    const byKey = new Map(cmp.deltas.map((d) => [d.retry_key, d]));
    assert.equal(byKey.get('k1')!.status, 'escalated');
    assert.equal(byKey.get('k3')!.status, 'new');
    assert.equal(byKey.get('k2')!.status, 'resolved');
    assert.equal(cmp.escalatedCount, 1);
    assert.equal(cmp.newCount, 1);
    assert.equal(cmp.resolvedCount, 1);
  });

  it('reports occurrence deltas', () => {
    const cmp = compareReports(a, b);
    const k1 = cmp.deltas.find((d) => d.retry_key === 'k1')!;
    assert.equal(k1.a?.occurrences, 2);
    assert.equal(k1.b?.occurrences, 7);
  });

  it('diffs dup storm topics', () => {
    const cmp = compareReports(a, b);
    assert.deepEqual(cmp.dupStormNew, ['review.file']);
    assert.deepEqual(cmp.dupStormResolved, []);
  });

  it('compares ledger-view reports by stage:reason_code', () => {
    const la: DiagnoseReportLike = {
      schema_version: 'u8-1', used_ledger: true, workspace: '/w',
      root_causes: [
        { stage: 'origin', reason_code: 'isolated_scope_violation', frequency: 3 },
      ],
    };
    const lb: DiagnoseReportLike = {
      schema_version: 'u8-1', used_ledger: true, workspace: '/w',
      root_causes: [
        { stage: 'origin', reason_code: 'isolated_scope_violation', frequency: 3 },
        { stage: 'policy', reason_code: 'missing_field', frequency: 1 },
      ],
    };
    const cmp = compareReports(la, lb);
    assert.equal(cmp.newCount, 1);
    assert.equal(cmp.persistentCount, 1);
    assert.equal(cmp.aView, 'ledger');
  });

  it('orders escalated first', () => {
    const cmp = compareReports(a, b);
    assert.equal(cmp.deltas[0].status, 'escalated');
  });
});
