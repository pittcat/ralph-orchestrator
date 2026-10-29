import { buildApp } from './app.js';
import { loadConfig } from './config.js';
import { startCollector } from './collector/collector.js';
import { startDeliveryWorker } from './notify/worker.js';

const config = loadConfig();
const { app, db, machineId } = buildApp({
  dbPath: config.dbPath,
  logger: true,
});
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

app.listen({ host: config.host, port: config.port }).catch((err) => {
  app.log.error(err);
  process.exit(1);
});
app.log.info({ machineId, dbPath: config.dbPath }, 'ralph-monitor up');
