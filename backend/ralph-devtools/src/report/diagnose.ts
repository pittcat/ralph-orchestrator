/**
 * `ralph diagnose --format json` wrapper — the Rust-side report is the
 * sole authority on findings; TS layers classification, timeline and
 * cross-run comparison on top of its stable JSON contract
 * (`schema_version="1"`).
 */

import { runRalphJson, type RalphResult, type RalphRunOptions } from '../ralph/runner.js';
import type { DiagnoseReportLike } from './classify.js';

export interface DiagnoseArgs {
  /** Workspace dir containing `.ralph/` (default: cwd). */
  cwd?: string;
  /** "latest" (default), absolute/relative path, or session id under --diagnostics-root. */
  session?: string;
  diagnosticsRoot?: string;
  source?: string;
  fromLedger?: boolean;
  legacy?: boolean;
  causal?: boolean;
  supervisor?: 'off' | 'json' | 'human';
}

export function runDiagnose(
  args: DiagnoseArgs = {},
  opts: RalphRunOptions = {},
): Promise<RalphResult<DiagnoseReportLike>> {
  const argv = ['diagnose', '--format', 'json'];
  if (args.session !== undefined) argv.push('--session', args.session);
  if (args.diagnosticsRoot !== undefined) {
    argv.push('--diagnostics-root', args.diagnosticsRoot);
  }
  if (args.source !== undefined) argv.push('--source', args.source);
  if (args.fromLedger) argv.push('--from-ledger');
  if (args.legacy) argv.push('--legacy');
  if (args.causal) argv.push('--causal');
  if (args.supervisor !== undefined && args.supervisor !== 'off') {
    argv.push('--supervisor', args.supervisor);
  }
  return runRalphJson<DiagnoseReportLike>(argv, { ...opts, cwd: args.cwd ?? opts.cwd });
}
