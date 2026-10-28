import Database from 'better-sqlite3';
import fs from 'node:fs';
import path from 'node:path';

export type MonitorDb = Database.Database;

export function openDatabase(dbPath: string): MonitorDb {
  if (dbPath !== ':memory:') {
    fs.mkdirSync(path.dirname(dbPath), { recursive: true });
  }
  const db = new Database(dbPath);
  db.pragma('journal_mode = WAL');
  db.pragma('foreign_keys = ON');
  return db;
}
