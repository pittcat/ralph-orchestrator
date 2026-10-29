import cors from '@fastify/cors';
import Fastify, { type FastifyInstance } from 'fastify';
import { openDatabase, type MonitorDb } from './db/index.js';
import { runMigrations } from './db/migrations.js';
import { ensureMachineId } from './lib/machine_id.js';
import { registerSecurity } from './plugins/security.js';
import { registerProjectRoutes } from './routes/projects.js';
import { registerRunRoutes } from './routes/runs.js';
import { registerHealthRoutes } from './routes/health.js';
import { registerEventRoutes } from './routes/events.js';
import { registerArtifactRoutes } from './routes/artifacts.js';
import { registerStreamRoutes } from './routes/stream.js';
import { registerSnapshotRoutes } from './routes/snapshot.js';
import { registerNotificationRoutes } from './routes/notifications.js';
import { registerOperationRoutes } from './routes/operations.js';

export interface BuildAppOptions {
  /** SQLite path; `:memory:` is honored for unit tests. */
  dbPath: string;
  /** Emit Fastify logs (off in tests). */
  logger?: boolean;
  /** SSE tuning overrides (tests shrink buffers/heartbeats). */
  sse?: Partial<import('./stream/sse.js').SseOptions>;
}

export interface MonitorApp {
  app: FastifyInstance;
  db: MonitorDb;
  machineId: string;
}

/**
 * Build the monitor Fastify instance. `app.inject()`-testable —
 * callers own the lifecycle (`await app.app.close()` closes the db
 * via the onClose hook).
 */
export function buildApp(options: BuildAppOptions): MonitorApp {
  const app = Fastify({
    logger: options.logger ?? false,
    // Global run ids are `<machineId>:<projectId>:<runId>` (~130 chars);
    // find-my-way's default maxParamLength (100) would drop `/runs/:globalId`
    // matches to the SPA wildcard, so raise it with headroom for longer
    // project/run ids.
    routerOptions: { maxParamLength: 512 },
  });
  const db = openDatabase(options.dbPath);
  runMigrations(db);
  const machineId = ensureMachineId(db);

  app.addHook('onClose', async () => {
    db.close();
  });

  // CORS: loopback origins only — same rule the Origin check enforces
  // on unsafe methods, applied here to preflights too.
  void app.register(cors, {
    origin: (origin, cb) => {
      if (origin === undefined) {
        cb(null, true);
        return;
      }
      try {
        const hostname = new URL(origin).hostname.toLowerCase();
        cb(
          null,
          hostname === 'localhost' ||
            hostname === '127.0.0.1' ||
            hostname === '::1' ||
            hostname === '[::1]',
        );
      } catch {
        cb(null, false);
      }
    },
  });

  registerSecurity(app);
  registerProjectRoutes(app, db);
  registerRunRoutes(app, db, machineId);
  registerHealthRoutes(app, db, machineId);
  registerEventRoutes(app, db, machineId);
  registerArtifactRoutes(app, db, machineId);
  registerStreamRoutes(app, db, options.sse);
  registerSnapshotRoutes(app, db, machineId);
  registerNotificationRoutes(app, db);
  registerOperationRoutes(app, db);

  app.get('/api/monitor/v1/health', async () => ({
    ok: true,
    machine_id: machineId,
  }));

  return { app, db, machineId };
}
