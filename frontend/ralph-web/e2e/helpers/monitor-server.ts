/**
 * Plan5-U5 e2e helper — real monitor supervisor + fixture generator.
 *
 * Run by playwright `webServer`:
 *   npx tsx e2e/helpers/monitor-server.ts
 *
 * It (1) materialises two git-initialised fixture projects with a
 * historical run each, (2) writes `.tmp-monitor/fixtures.json` so the
 * spec can discover paths, then (3) spawns the real ralph-monitor with
 * the fake-ralph fixture on PATH. On child exit it respawns after a
 * short delay — the journey spec kills the child mid-test to exercise
 * the offline → reconnect recovery path against a fresh process.
 */
import { execFileSync, spawn, type ChildProcess } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const e2eDir = path.dirname(fileURLToPath(import.meta.url));
const repoRoot = path.resolve(e2eDir, "../../../..");
const monitorSrc = path.join(repoRoot, "backend/ralph-monitor/src/server.ts");
const fakeRalphDir = path.join(
  repoRoot,
  "backend/ralph-monitor/test/fixtures/fake-ralph",
);
const tmpRoot = path.join(e2eDir, ".tmp-monitor");
const PORT = Number(process.env.MONITOR_E2E_PORT ?? 3310);
const RESPAWN_DELAY_MS = 800;

export interface JourneyFixtures {
  port: number;
  dbPath: string;
  projects: { key: string; dir: string; runId: string }[];
}

function historicalRun(projectDir: string, runId: string): unknown {
  return {
    run_id: runId,
    kind: "primary",
    state: "exited",
    cwd: projectDir,
    workspace: projectDir,
    repo_root: projectDir,
    prompt_preview: `e2e history ${runId}`,
    preset: "builtin:debug",
    started_at: "2026-10-28T11:59:00Z",
    events_file: path.join(projectDir, ".ralph/events.jsonl"),
    terminal: { reason: "completed", at: "2026-10-28T12:00:00Z" },
    evidence: { manifest_present: true, registry_present: true },
    actions: [],
  };
}

export function materialiseFixtures(): JourneyFixtures {
  fs.rmSync(tmpRoot, { recursive: true, force: true });
  const projects = ["alpha", "beta"].map((key) => {
    const dir = path.join(tmpRoot, `proj-${key}`);
    fs.mkdirSync(path.join(dir, ".git"), { recursive: true });
    const runId = `e2e-${key}-run`;
    const fixture = {
      schema_version: "run-observation/v1",
      workspace: dir,
      generated_at: "2026-10-28T12:00:00Z",
      runs: [historicalRun(dir, runId)],
    };
    fs.mkdirSync(path.join(dir, ".monitor-fixture"), { recursive: true });
    fs.writeFileSync(
      path.join(dir, ".monitor-fixture/snapshot.json"),
      JSON.stringify(fixture),
    );
    return { key, dir, runId };
  });
  const dbPath = path.join(tmpRoot, "monitor.db");
  const manifest: JourneyFixtures = { port: PORT, dbPath, projects };
  fs.writeFileSync(
    path.join(tmpRoot, "fixtures.json"),
    JSON.stringify(manifest, null, 2),
  );
  return manifest;
}

function spawnMonitor(): ChildProcess {
  return spawn("npx", ["tsx", monitorSrc], {
    cwd: repoRoot,
    env: {
      ...process.env,
      RALPH_MONITOR_PORT: String(PORT),
      RALPH_MONITOR_DB_PATH: path.join(tmpRoot, "monitor.db"),
      RALPH_MONITOR_POLL_MS: "400",
      PATH: `${fakeRalphDir}:${process.env.PATH ?? ""}`,
    },
    stdio: ["ignore", "inherit", "inherit"],
  });
}

// ── script entry ──
if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  const manifest = materialiseFixtures();
  console.log(
    `[monitor-e2e] fixtures ready on port ${manifest.port}: ` +
      manifest.projects.map((p) => p.dir).join(", "),
  );
  // The journey spec kills the monitor mid-test to exercise the
  // offline → reconnect path; every exit respawns after a short delay.
  let child: ChildProcess;
  const start = () => {
    child = spawnMonitor();
    child.on("exit", (code) => {
      console.log(`[monitor-e2e] child exited (${code}); respawning…`);
      setTimeout(start, RESPAWN_DELAY_MS);
    });
  };
  start();
  const shutdown = () => {
    child.kill("SIGTERM");
    process.exit(0);
  };
  process.on("SIGTERM", shutdown);
  process.on("SIGINT", shutdown);
}

/** Spec-side helper: fixtures manifest may lag webServer boot. */
export function readFixtures(retries = 50): JourneyFixtures {
  const file = path.join(tmpRoot, "fixtures.json");
  for (let i = 0; i < retries; i++) {
    if (fs.existsSync(file)) {
      return JSON.parse(fs.readFileSync(file, "utf8"));
    }
    execFileSync("sleep", ["0.1"]);
  }
  throw new Error("monitor e2e fixtures.json never materialised");
}

/** Kill the monitor child (matched on its src/server.ts argv). */
export function killMonitorChild(): void {
  try {
    execFileSync("pkill", ["-f", "ralph-monitor/src/server.ts"]);
  } catch {
    // pkill exits 1 when nothing matched — tolerate.
  }
}
