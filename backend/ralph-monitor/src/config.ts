import os from 'node:os';
import path from 'node:path';
import { z } from 'zod';

export interface MonitorConfig {
  host: string;
  port: number;
  dbPath: string;
}

const envSchema = z.object({
  host: z.string().min(1).default('127.0.0.1'),
  port: z.coerce.number().int().min(1).max(65535).default(3200),
  dbPath: z
    .string()
    .min(1)
    .default(path.join(os.homedir(), '.ralph', 'monitor.db')),
});

export function defaultDbPath(): string {
  return path.join(os.homedir(), '.ralph', 'monitor.db');
}

/// Expand a leading `~/` via os.homedir(); `~user` forms are rejected
/// (ambiguous across platforms — see plan §20).
export function expandHome(input: string): string {
  if (input === '~') return os.homedir();
  if (input.startsWith('~/') || input.startsWith('~\\')) {
    return path.join(os.homedir(), input.slice(2));
  }
  if (input.startsWith('~')) {
    throw new Error(`~user home expansion is not supported: ${input}`);
  }
  return input;
}

export function loadConfig(
  env: NodeJS.ProcessEnv = process.env,
): MonitorConfig {
  const parsed = envSchema.parse({
    host: env.RALPH_MONITOR_HOST,
    port: env.RALPH_MONITOR_PORT,
    dbPath: env.RALPH_MONITOR_DB_PATH,
  });
  return {
    host: parsed.host,
    port: parsed.port,
    dbPath:
      parsed.dbPath === ':memory:'
        ? parsed.dbPath
        : path.resolve(expandHome(parsed.dbPath)),
  };
}
