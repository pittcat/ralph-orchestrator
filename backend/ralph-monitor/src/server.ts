import { buildApp } from './app.js';
import { loadConfig } from './config.js';

const config = loadConfig();
const { app, machineId } = buildApp({ dbPath: config.dbPath, logger: true });

const shutdown = async () => {
  try {
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
