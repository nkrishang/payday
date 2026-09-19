/**
 * Load scheduling: synchronized cohorts (capacity60) and open-loop arrivals
 * (latency, soak). Arrivals are independent of completions — completion must
 * never throttle admission — and the scheduler is the only admission source.
 */

import { randomBytes } from "node:crypto";

export interface ScheduleEntry {
  index: number;
  /** Delay in milliseconds from the phase's T0. */
  atMs: number;
  warmup: boolean;
}

/** One cohort: every lifecycle admitted at a common T0. */
export function cohort(count: number, warmupOffset = 0): ScheduleEntry[] {
  return Array.from({ length: count }, (_, index) => ({ index: warmupOffset + index, atMs: 0, warmup: false }));
}

/** Deterministic open-loop arrivals: fixed rate with optional seeded jitter. */
export function openLoop(count: number, ratePerSecond: number, options: { jitter?: boolean; seed?: string } = {}): ScheduleEntry[] {
  const entries: ScheduleEntry[] = [];
  let gapMs = ratePerSecond > 0 ? 1000 / ratePerSecond : 0;
  let t = 0;
  let seed = options.seed ? Number.parseInt(options.seed, 16) : Number.parseInt(randomBytes(4).toString("hex"), 16);
  for (let index = 0; index < count; index++) {
    entries.push({ index, atMs: Math.round(t), warmup: false });
    if (options.jitter) {
      // Multiplicative jitter from a simple xorshift; keeps the mean rate while
      // de-correlating arrivals.
      seed ^= seed << 13; seed >>>= 0;
      seed ^= seed >> 17;
      seed ^= seed << 5; seed >>>= 0;
      const uniform = (seed % 1000) / 1000;
      t += gapMs * (0.5 + uniform);
    } else {
      t += gapMs;
    }
  }
  return entries;
}

export function nextSeed(): string {
  return randomBytes(4).toString("hex");
}

/** Waiting helpers shared by the engine. */
export function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
