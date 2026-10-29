/**
 * Human renderers for preset models and topology: `show` summary,
 * ASCII edge listing, and Mermaid flowchart. Pure functions — CLI
 * dispatch decides where the text lands.
 */

import type { PresetModel } from './model.js';
import type { Topology } from './topology.js';

function fmtList(values: string[]): string {
  return values.length > 0 ? values.join(', ') : '—';
}

export function renderShow(model: PresetModel, topo: Topology): string {
  const lines: string[] = [];
  lines.push(`preset: ${model.ref} (${model.source})`);
  if (model.yamlErrors.length > 0) {
    for (const e of model.yamlErrors) lines.push(`  YAML parse error: ${e}`);
  }
  const el = model.eventLoop ?? {};
  const elBits: string[] = [];
  if (typeof el.execution_mode === 'string') {
    elBits.push(`execution_mode=${el.execution_mode}`);
  }
  if (typeof el.completion_promise === 'string') {
    elBits.push(`completion_promise=${el.completion_promise}`);
  }
  if (typeof el.starting_event === 'string') {
    elBits.push(`starting_event=${el.starting_event}`);
  }
  if (typeof el.max_iterations === 'number') {
    elBits.push(`max_iterations=${el.max_iterations}`);
  }
  if (elBits.length > 0) lines.push(`event_loop: ${elBits.join('  ')}`);
  lines.push(`sections: ${model.sections.join(', ') || '—'}`);
  lines.push('');
  lines.push(`hats (${model.hats.length}):`);
  for (const hat of model.hats) {
    const label = hat.name ? `${hat.id} (“${hat.name}”)` : hat.id;
    lines.push(`  ${label}`);
    if (hat.description) lines.push(`    desc:     ${hat.description}`);
    lines.push(`    triggers: ${fmtList(hat.triggers)}`);
    lines.push(`    publishes:${' '}${fmtList(hat.publishes)}`);
    if (hat.terminalEvents.length > 0) {
      lines.push(`    terminal: ${fmtList(hat.terminalEvents)}`);
    }
    if (hat.defaultPublishes) {
      lines.push(`    default:  ${hat.defaultPublishes}`);
    }
    if (hat.concurrency !== undefined || hat.hasAggregate) {
      const bits = [
        hat.concurrency !== undefined ? `concurrency=${hat.concurrency}` : null,
        hat.hasAggregate ? 'aggregate' : null,
      ].filter(Boolean);
      lines.push(`    flags:    ${bits.join(' ')}`);
    }
    if (hat.instructionsChars > 0) {
      lines.push(`    instructions: ${hat.instructionsChars} chars`);
    }
    if (hat.extraKeys.length > 0) {
      lines.push(`    extra keys: ${hat.extraKeys.join(', ')}`);
    }
  }
  lines.push('');
  lines.push(`sources (no hat publishes): ${fmtList(topo.sourceTopics)}`);
  lines.push(`sinks (no subscriber):      ${fmtList(topo.sinkTopics)}`);
  if (topo.unreachableHats.length > 0) {
    lines.push(`advisory: unreachable hats: ${topo.unreachableHats.join(', ')}`);
  }
  if (topo.deadEndHats.length > 0) {
    lines.push(
      `advisory: hats whose publishes reach no subscriber (terminal excluded): ${topo.deadEndHats.join(', ')}`,
    );
  }
  lines.push('');
  lines.push('authority: validity is decided by `ralph preset check` / `ralph preset verify`.');
  return lines.join('\n');
}

export function renderAsciiGraph(topo: Topology): string {
  const lines: string[] = [];
  const sorted = [...topo.edges].sort(
    (a, b) => a.from.localeCompare(b.from) || a.topic.localeCompare(b.topic) || a.to.localeCompare(b.to),
  );
  for (const e of sorted) {
    lines.push(`${e.from} --${e.topic}--> ${e.to}`);
  }
  if (lines.length === 0) lines.push('(no edges — no hat publishes a topic another hat triggers on)');
  return lines.join('\n');
}

function mermaidId(hatId: string): string {
  const cleaned = hatId.replace(/[^a-zA-Z0-9_]/g, '_');
  return cleaned.length > 0 ? `h_${cleaned}` : 'h_hat';
}

export function renderMermaid(model: PresetModel, topo: Topology): string {
  const lines: string[] = ['flowchart LR'];
  for (const hat of model.hats) {
    const label = (hat.name ?? hat.id).replace(/"/g, "'");
    lines.push(`  ${mermaidId(hat.id)}["${label}"]`);
  }
  for (const topic of topo.sourceTopics) {
    lines.push(`  ext_${topic.replace(/[^a-zA-Z0-9_]/g, '_')}(("${topic}"))`);
  }
  for (const e of topo.edges) {
    const from = e.from === '<external>' ? `ext_${e.topic.replace(/[^a-zA-Z0-9_]/g, '_')}` : mermaidId(e.from);
    lines.push(`  ${from} -- "${e.topic}" --> ${mermaidId(e.to)}`);
  }
  return lines.join('\n');
}
