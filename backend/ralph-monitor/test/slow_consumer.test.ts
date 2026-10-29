import assert from 'node:assert/strict';
import { describe, it, beforeEach, afterEach } from 'node:test';
import fs from 'node:fs';
import net from 'node:net';
import {
  tmpDbPath,
  makeProjectDir,
  insertProject,
} from './helpers.js';
import { buildApp, type MonitorApp } from '../src/app.js';
import { readEvents } from '../src/stream/monitor_log.js';

const NOW = new Date().toISOString();

describe('slow consumer', () => {
  let app: MonitorApp;
  let ws: string;
  let projectId: string;
  let base: string;

  beforeEach(async () => {
    // Tiny pending-byte budget + fast heartbeat so the test stays quick.
    app = buildApp({
      dbPath: tmpDbPath(),
      sse: { maxBufferedBytes: 64, heartbeatMs: 50, pollMs: 25 },
    });
    ws = makeProjectDir();
    projectId = insertProject(app.db, ws);
    base = await app.app.listen({ port: 0, host: '127.0.0.1' });
  });
  afterEach(async () => {
    await app.app.close();
    fs.rmSync(ws, { recursive: true, force: true });
  });

  it('socket that never reads is disconnected once pending bytes overflow', async () => {
    // Seed a multi-MB replay: far larger than the client's kernel
    // receive buffer, so the server's pending budget trips mid-replay.
    const fat = 'x'.repeat(2048);
    const insert = app.db.prepare(
      `INSERT INTO monitor_events (kind, project_id, run_id, payload, at)
       VALUES ('run.updated', ?, 'r', ?, ?)`,
    );
    app.db.transaction(() => {
      for (let i = 0; i < 3000; i++) {
        insert.run(projectId, fat, NOW);
      }
    })();
    assert.ok(readEvents(app.db, 0).length >= 1000);

    const { port } = new URL(base);
    const closed = await new Promise<boolean>((resolve) => {
      const sock = net.connect(Number(port), '127.0.0.1', () => {
        sock.write(
          `GET /api/monitor/v1/stream?cursor=0 HTTP/1.1\r\nHost: 127.0.0.1:${port}\r\n\r\n`,
        );
        // never read — kernel rx buffer fills, server overflows its
        // logical pending budget on the next write and destroys us.
      });
      const t = setTimeout(() => {
        sock.destroy();
        resolve(false);
      }, 5_000);
      sock.on('close', () => {
        clearTimeout(t);
        resolve(true);
      });
      sock.on('error', () => {});
    });
    assert.equal(closed, true, 'server did not drop the slow consumer');
  });
});
