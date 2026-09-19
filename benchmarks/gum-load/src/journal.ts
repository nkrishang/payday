/**
 * Durable run journal: one authoritative NDJSON event log per run, written
 * ahead of every irreversible side effect, plus private blobs (signed
 * transactions, seeds, webhook raw bodies) kept outside the report allowlist.
 *
 * Durability: sequence-critical events (intents, signed transactions, webhook
 * captures) are fsynced before the corresponding external effect is confirmed;
 * the rest are group-flushed on a short timer. Snapshots are atomic renames
 * and always disposable — the journal is the source of truth.
 */

import { createHash, randomUUID } from "node:crypto";
import {
  closeSync, existsSync, mkdirSync, openSync, readFileSync, renameSync, statSync, writeFileSync, writeSync, fsyncSync,
} from "node:fs";
import { join } from "node:path";
import type { JournalEvent } from "./model.js";

interface JournalRecord {
  seq: number;
  epoch: number;
  mono: number;
  wall: string;
  type: string;
  payload: unknown;
}

/** Event types that must reach disk before the harness admits the effect. */
const WRITE_AHEAD_TYPES = new Set([
  "op.intended", "op.payment.intent", "op.payment.signed", "webhook.received",
  "funding.record", "run.started",
]);

export class Journal {
  readonly dir: string;
  readonly epoch: number;
  private readonly path: string;
  private readonly privateDir: string;
  private fd: number;
  private seq = 0;
  private dirty = false;
  private flushed = 0;

  constructor(dir: string) {
    this.dir = dir;
    this.epoch = process.pid;
    this.path = join(dir, "events.ndjson");
    this.privateDir = join(dir, "private");
    mkdirSync(dir, { recursive: true });
    mkdirSync(this.privateDir, { recursive: true, mode: 0o700 });
    const exists = existsSync(this.path);
    this.fd = openSync(this.path, exists ? "a" : "w", 0o600);
    if (exists) {
      // Resume: continue the sequence after the last durable record.
      for (const record of readRecords(this.path, { lenientTail: true })) this.seq = Math.max(this.seq, record.seq);
    }
  }

  append(type: JournalEvent["type"], payload: unknown, mono: number, options: { durable?: boolean } = {}): void {
    const record: JournalRecord = {
      seq: ++this.seq, epoch: this.epoch, mono, wall: new Date().toISOString(), type, payload,
    };
    const line = `${JSON.stringify(record)}\n`;
    writeSync(this.fd, line);
    if (options.durable ?? WRITE_AHEAD_TYPES.has(type)) fsyncSync(this.fd);
    else this.dirty = true;
  }

  /** Persist a secret-bearing payload outside the report allowlist; returns its reference. */
  blob(name: string, contents: string): string {
    const file = join(this.privateDir, `${encodeURIComponent(name)}.json`);
    if (existsSync(file)) {
      // A same-named blob must never silently carry different bytes: that
      // would let one operation's signed transaction stand in for another's.
      const existing = readFileSync(file, "utf8");
      if (existing !== contents) throw new Error(`blob ${name} already exists with different contents`);
      return relativeBlob(this.dir, file);
    }
    writeFileSync(file, contents, { mode: 0o600 });
    return relativeBlob(this.dir, file);
  }

  blobContents(reference: string): string {
    return readFileSync(join(this.dir, reference), "utf8");
  }

  flush(): void {
    if (!this.dirty) return;
    fsyncSync(this.fd);
    this.dirty = false;
  }

  snapshot(state: unknown): void {
    const tmp = join(this.dir, `state.json.tmp-${process.pid}`);
    writeFileSync(tmp, JSON.stringify(state, null, 2));
    fsyncSync(openSync(tmp, "r+"));
    renameSync(tmp, join(this.dir, "state.json"));
  }

  close(): void {
    this.flush();
    closeSync(this.fd);
  }

  get lastSeq(): number {
    return this.seq;
  }
}

function relativeBlob(runDir: string, file: string): string {
  return file.startsWith(runDir) ? file.slice(runDir.length + 1) : file;
}

export function readRecords(path: string, options: { lenientTail?: boolean } = {}): JournalRecord[] {
  const raw = readFileSync(path, "utf8");
  const records: JournalRecord[] = [];
  const lines = raw.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i]!.trim();
    if (!line) continue;
    try {
      records.push(JSON.parse(line) as JournalRecord);
    } catch {
      const isLast = i === lines.length - 1 || (i === lines.length - 2 && lines[lines.length - 1] === "");
      if (isLast && options.lenientTail) break; // interrupted final write: recoverable
      throw new Error(`journal ${path} is corrupted at record ${records.length + 1}; refusing to silently skip`);
    }
  }
  return records;
}

export function replay(path: string): Array<{ record: JournalRecord }> {
  const records = readRecords(path, { lenientTail: true });
  return records.map((record) => ({ record }));
}

export function newRunId(): string {
  const stamp = new Date().toISOString().replace(/[-:]/g, "").replace(/\..+/, "");
  return `${stamp}-${randomUUID().slice(0, 8)}`;
}

export function runDir(root: string, runId: string): string {
  return join(root, runId);
}

/** Stable short hash used to correlate webhook payloads without leaking secrets. */
export function shortHash(text: string): string {
  return createHash("sha256").update(text).digest("hex").slice(0, 12);
}

export function journalSize(path: string): number {
  try {
    return statSync(path).size;
  } catch {
    return 0;
  }
}
