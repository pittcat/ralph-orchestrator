import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import {
  classifyReason,
  classifyFindings,
  diagnoseView,
  repeatGroups,
  type DiagnoseReportLike,
} from '../../src/report/classify.js';

describe('classifyReason', () => {
  it('maps known reason families', () => {
    assert.equal(classifyReason('duplicate_work_done_same_step'), 'duplicate_resend');
    assert.equal(classifyReason('isolated_scope_violation'), 'isolation_violation');
    assert.equal(classifyReason('wave_aggregate_deadline_exceeded'), 'wave_handoff');
    assert.equal(classifyReason('missing_terminal_emit_exhausted'), 'missing_emit');
    assert.equal(classifyReason('payload_contract_violation'), 'payload_contract');
    assert.equal(classifyReason('missing_field'), 'payload_contract');
    assert.equal(classifyReason('phase_violation'), 'topology_flow');
    assert.equal(classifyReason('drift_field_completeness'), 'drift');
    assert.equal(classifyReason('task_resume_dead_letter'), 'task_resume');
    assert.equal(classifyReason('totally_unheard_of'), 'other');
  });

  it('source string participates in matching', () => {
    assert.equal(classifyReason('some_reason', 'wave_dispatcher'), 'wave_handoff');
    assert.equal(classifyReason('some_reason', 'drift_monitor'), 'drift');
  });
});

const sessionReport: DiagnoseReportLike = {
  schema_version: '1',
  session_path: '/ws/.ralph/diagnostics/S',
  top_findings: [
    {
      retry_key: 'k1',
      severity: 'error',
      source: 'policy',
      reason_code: 'payload_contract_violation',
      topic: 'work.done',
      occurrences: 3,
      escalated: true,
      message: 'm',
    },
    {
      retry_key: 'k2',
      severity: 'warn',
      source: 'wave',
      reason_code: 'wave_partial_threshold',
      topic: 'review.file',
      occurrences: 2,
      message: 'n',
    },
  ],
  recovery_timeline: [
    { iteration: 3, hat: 'ex', reason_code: 'payload_contract_violation', topic: 'work.done', message: 'a' },
    { iteration: 4, hat: 'ex', reason_code: 'payload_contract_violation', topic: 'work.done', message: 'b' },
    { iteration: 9, hat: 'agg', reason_code: 'wave_partial_threshold', topic: 'review.file', message: 'c' },
  ],
};

const ledgerReport: DiagnoseReportLike = {
  schema_version: 'u8-1',
  used_ledger: true,
  workspace: '/ws',
  root_causes: [
    {
      stage: 'origin',
      reason_code: 'isolated_scope_violation',
      frequency: 5,
      escalated: true,
      affected_hats: ['reviewer'],
      affected_topics: ['review.done'],
      suggested_remediation: 'declare topic in publishes',
    },
  ],
  rejection_summary: { record_count: 5, distinct_reason_codes: 1, terminal_count: 1 },
};

describe('diagnoseView', () => {
  it('distinguishes session vs ledger shapes', () => {
    assert.equal(diagnoseView(sessionReport), 'session');
    assert.equal(diagnoseView(ledgerReport), 'ledger');
    assert.equal(diagnoseView({}), 'unknown');
  });
});

describe('classifyFindings', () => {
  it('buckets session top_findings with severity + occurrences', () => {
    const buckets = classifyFindings(sessionReport);
    assert.equal(buckets.length, 2);
    const first = buckets[0];
    assert.equal(first.category, 'payload_contract');
    assert.equal(first.occurrences, 3);
    assert.equal(first.escalated, 1);
    assert.equal(first.worstSeverity, 'error');
  });

  it('buckets ledger root_causes (frequency → occurrences)', () => {
    const buckets = classifyFindings(ledgerReport);
    assert.equal(buckets.length, 1);
    assert.equal(buckets[0].category, 'isolation_violation');
    assert.equal(buckets[0].occurrences, 5);
    assert.equal(buckets[0].worstSeverity, 'error');
  });
});

describe('repeatGroups', () => {
  it('re-aggregates timeline rows by (reason, topic, hat)', () => {
    const groups = repeatGroups(sessionReport);
    const g = groups.find((x) => x.reason_code === 'payload_contract_violation')!;
    assert.equal(g.count, 2);
    assert.equal(g.firstIteration, 3);
    assert.equal(g.lastIteration, 4);
  });

  it('lifts ledger root_causes into groups', () => {
    const groups = repeatGroups(ledgerReport);
    assert.equal(groups.length, 1);
    assert.equal(groups[0].count, 5);
    assert.equal(groups[0].category, 'isolation_violation');
  });
});
