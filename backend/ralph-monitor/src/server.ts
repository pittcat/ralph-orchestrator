import fs from 'node:fs';
import { buildApp } from './app.js';
import { loadConfig } from './config.js';
import { startCollector } from './collector/collector.js';
import { startDeliveryWorker } from './notify/worker.js';
import { registerStatic, registerStaticMissing } from './static/spa.js';

const LOOPBACK = new Set(['127.0.0.1', 'localhost', '::1']);

const config = loadConfig();

// Fail-closed: non-loopback bind requires explicit acknowledgement —
// the service has no auth layer (Plan5-U4 §20).
if (!LOOPBACK.has(config.host)) {
  console.error(
    `ralph-monitor: refusing to bind non-loopback host ${config.host} ` +
      `(loopback-only service; set RALPH_MONITOR_HOST=127.0.0.1)`,
  );
  process.exit(1);
}

const { app, db, machineId } = buildApp({
  dbPath: config.dbPath,
  logger: true,
});

// Plan5-U4: static SPA hosting — a missing dist dir is a warning,
// not a startup failure (the API stays usable; / serves a hint page).
if (fs.existsSync(config.staticDir)) {
  registerStatic(app, config.staticDir);
  const indexMtime = fs
    .statSync(`${config.staticDir}/index.html`)
    .mtime.toISOString();
  app.log.info(
    { staticDir: config.staticDir, indexMtime },
    'serving dashboard bundle',
  );
} else {
  registerStaticMissing(app, config.staticDir);
  app.log.warn(
    { staticDir: config.staticDir },
    'dashboard bundle missing — API-only mode',
  );
}

const collector = startCollector(db, {
  pollMs: config.pollMs,
  log: (msg, err) => app.log.error(err, msg),
});
const worker = startDeliveryWorker(db, {
  log: (msg, err) => app.log.error(err, msg),
});

const shutdown = async () => {
  try {
    collector.stop();
    worker.stop();
    await app.close();
    process.exit(0);
  } catch (err) {
    app.log.error(err, 'error during shutdown');
    process.exit(1);
  }
};
process.on('SIGINT', shutdown);
process.on('SIGTERM', shutdown);

app
  .listen({ host: config.host, port: config.port })
  .then(() => {
    // `ralph web --monitor` waits on this literal line (ready pattern).
    app.log.info(
      { machineId, dbPath: config.dbPath },
      `ralph-monitor listening on http://${config.host}:${config.port}`,
    );
  })
  .catch((err) => {
    app.log.error(err);
    process.exit(1);
  });
