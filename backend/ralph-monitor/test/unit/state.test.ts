import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  isMonitorState,
  mapObservationState,
  type ObservationStateInput,
} from '../../src/lib/state.js';

function obs(overrides: Partial<ObservationStateInput> = {}): ObservationStateInput {
  return {
    state: 'unknown',
    pid: null,
    terminal: null,
    evidence: {
      manifestPresent: false,
      manifestCorrupt: false,
      registryPresent: false,
      lockPresent: false,
      mergeState: null,
    },
    ...overrides,
  };
}

test('live pid state → running', () => {
  const { state, evidence } = mapObservationState(
    obs({ state: 'running', pid: 1234 }),
  );
  assert.equal(state, 'running');
  assert.equal(evidence.snapshotState, 'running');
  assert.equal(evidence.pid, 1234);
});

test('exited + completed → exited_success', () => {
  const { state } = mapObservationState(
    obs({
      state: 'exited',
      terminal: {
        reason: 'completed',
        at: '2026-10-28T12:00:00Z',
        mergeCommit: null,
      },
    }),
  );
  assert.equal(state, 'exited_success');
});

test('exited + merge queue merged → exited_merged (S5-Y)', () => {
  const { state, evidence } = mapObservationState(
    obs({
      state: 'exited',
      terminal: {
        reason: 'completed',
        at: '2026-10-28T12:00:00Z',
        mergeCommit: null,
      },
      evidence: {
        manifestPresent: true,
        manifestCorrupt: false,
        registryPresent: true,
        lockPresent: false,
        mergeState: 'merged',
      },
    }),
  );
  assert.equal(state, 'exited_merged');
  assert.equal(evidence.mergeState, 'merged');
});

test('exited + terminal merge_commit → exited_merged even without queue row', () => {
  const { state } = mapObservationState(
    obs({
      state: 'exited',
      terminal: {
        reason: 'completed',
        at: '2026-10-28T12:00:00Z',
        mergeCommit: 'deadbeef',
      },
    }),
  );
  assert.equal(state, 'exited_merged');
});

test('unknown + merged queue evidence still converges to exited_merged', () => {
  const { state } = mapObservationState(
    obs({
      state: 'unknown',
      evidence: {
        manifestPresent: false,
        manifestCorrupt: false,
        registryPresent: true,
        lockPresent: false,
        mergeState: 'merged',
      },
    }),
  );
  assert.equal(state, 'exited_merged');
});

test('exited + non-success reasons → exited_failed (E10 vocabulary)', () => {
  for (const reason of [
    'consecutive_failures',
    'loop_thrashing',
    'loop_stale',
    'validation_failure',
    'stopped',
    'interrupted',
    'workspace_gone',
    'cancelled',
    'payload_contract_violation',
    'recovery_exhausted',
    'review_failed',
    'completion_stuck',
    'fan_in_failed',
    'max_iterations',
    'max_runtime',
    'max_cost',
    'restart_requested',
  ]) {
    const { state, evidence } = mapObservationState(
      obs({
        state: 'exited',
        terminal: { reason, at: '2026-10-28T12:00:00Z', mergeCommit: null },
      }),
    );
    assert.equal(state, 'exited_failed', `reason=${reason}`);
    assert.equal(evidence.terminalReason, reason);
  }
});

test('dead pid + no terminal → exited_unknown, never fabricate success (S6)', () => {
  const { state, evidence } = mapObservationState(
    obs({
      state: 'unknown',
      pid: 99999999,
      evidence: {
        manifestPresent: false,
        manifestCorrupt: false,
        registryPresent: false,
        lockPresent: true,
        mergeState: null,
      },
    }),
  );
  assert.equal(state, 'exited_unknown');
  assert.equal(evidence.snapshotState, 'unknown');
  assert.equal(evidence.lockPresent, true);
  assert.equal(evidence.manifestPresent, false);
});

test('unrecognised snapshot state degrades to exited_unknown', () => {
  const { state, evidence } = mapObservationState(obs({ state: 'whatever' }));
  assert.equal(state, 'exited_unknown');
  assert.equal(evidence.snapshotState, 'unknown');
});

test('silent is in the monitor vocabulary (emitted by Plan 3, not here)', () => {
  assert.equal(isMonitorState('silent'), true);
  assert.equal(isMonitorState('running'), true);
  assert.equal(isMonitorState('bogus'), false);
});
