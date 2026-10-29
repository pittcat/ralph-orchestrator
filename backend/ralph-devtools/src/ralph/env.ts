/**
 * Child-process env construction.
 *
 * Same contract as `backend/ralph-monitor/src/ralph/client.ts`: an
 * allowlist of ambient keys plus `RALPH_DEVTOOLS_*`, so running this
 * tool inside a hat activation never leaks agent context
 * (`RALPH_CURRENT_HAT`, `RALPH_EVENTS_FILE`, `RALPH_HATS_SOURCE`,
 * `RALPH_CONFIG`, …) into a human-CLI invocation — the TS-side
 * equivalent of HARD RULE 5 (`scrub_agent_runtime_env`).
 */

const ENV_ALLOWLIST = new Set([
  'PATH',
  'HOME',
  'TMPDIR',
  'TMP',
  'TEMP',
  'LANG',
  'LC_ALL',
  'LC_CTYPE',
  'TZ',
  'USER',
  'SHELL',
  'TERM',
]);

export function buildChildEnv(
  source: NodeJS.ProcessEnv = process.env,
): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {};
  for (const key of Object.keys(source)) {
    if (ENV_ALLOWLIST.has(key) || key.startsWith('RALPH_DEVTOOLS_')) {
      const value = source[key];
      if (value !== undefined) env[key] = value;
    }
  }
  return env;
}
