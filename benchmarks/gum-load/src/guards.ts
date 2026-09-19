/**
 * Guard controller: atomic reservations against the approval limits, the
 * pause/drain switches, and the stop conditions of the plan's §8. A limit
 * breach stops admission; it never cancels or rolls back funded work.
 */

import type { EventSink, RunState } from "./model.js";
import type { Limits } from "./config.js";
import { decimalToUnits } from "./model.js";

export interface Reservation {
  op: string;
  amountUnits: string;
  release(): void;
}

export class Guards {
  private paused = false;
  private stoppedReason?: string;
  private reserved = 0;
  private grossUnits = 0n;
  private outstandingUnits = 0n;
  private admittedCount = 0;

  constructor(
    private readonly limits: Limits,
    private readonly sink: EventSink,
    private readonly mode: "local" | "real",
  ) {}

  get stopReason(): string | undefined {
    return this.stoppedReason;
  }

  get isPaused(): boolean {
    return this.paused;
  }

  pause(reason: string): void {
    if (!this.paused) {
      this.paused = true;
      this.sink({ type: "guard.event", payload: { kind: "paused", detail: reason } });
    }
  }

  resume(): void {
    if (this.paused && !this.stoppedReason) {
      this.paused = false;
      this.sink({ type: "guard.event", payload: { kind: "resumed" } });
    }
  }

  stop(reason: string): void {
    if (!this.stoppedReason) {
      this.stoppedReason = reason;
      this.paused = true;
      this.sink({ type: "run.stopped", payload: { reason } });
      this.sink({ type: "guard.event", payload: { kind: "stopped", detail: reason } });
    }
  }

  canAdmit(): string | undefined {
    if (this.stoppedReason) return this.stoppedReason;
    if (this.paused) return "paused";
    if (this.admittedCount >= this.limits.maxRequests) return `request limit ${this.limits.maxRequests} reached`;
    return undefined;
  }

  /** Reserves money exposure before any external effect. Throws on breach. */
  reserve(op: string, amount: string): Reservation {
    const amountUnits = BigInt(decimalToUnits(amount));
    const maxAmount = BigInt(decimalToUnits(this.limits.maxAmount));
    const maxGross = BigInt(decimalToUnits(this.limits.maxGrossVolume));
    const maxOutstanding = BigInt(decimalToUnits(this.limits.maxOutstandingPrincipal));
    if (amountUnits > maxAmount) throw new Error(`guard: amount ${amount} exceeds approved max ${this.limits.maxAmount}`);
    if (this.grossUnits + amountUnits > maxGross) {
      const reason = `guard: gross volume limit ${this.limits.maxGrossVolume} reached`;
      this.stop(reason);
      throw new Error(reason);
    }
    if (this.outstandingUnits + amountUnits > maxOutstanding) {
      const reason = `guard: outstanding principal limit ${this.limits.maxOutstandingPrincipal} reached`;
      this.stop(reason);
      throw new Error(reason);
    }
    if (this.reserved >= this.limits.maxConcurrentFunded) {
      const reason = `guard: concurrency limit ${this.limits.maxConcurrentFunded} reached`;
      this.stop(reason);
      throw new Error(reason);
    }
    this.reserved += 1;
    this.admittedCount += 1;
    this.grossUnits += amountUnits;
    this.outstandingUnits += amountUnits;
    return {
      op,
      amountUnits: amountUnits.toString(),
      release: () => {
        this.reserved = Math.max(0, this.reserved - 1);
        this.outstandingUnits = BigInt(0) > this.outstandingUnits - amountUnits ? 0n : this.outstandingUnits - amountUnits;
      },
    };
  }

  /** Called by the engine when a lifecycle ages past the approved bound. */
  checkOldestUnsettled(ageSeconds: number | undefined): void {
    if (ageSeconds === undefined) return;
    if (ageSeconds > this.limits.maxOldestUnsettledSeconds) {
      this.stop(`oldest unsettled lifecycle is ${Math.round(ageSeconds)}s, past the approved ${this.limits.maxOldestUnsettledSeconds}s`);
    } else if (this.mode === "real" && ageSeconds > this.limits.maxOldestUnsettledSeconds * 0.8) {
      this.pause(`oldest unsettled lifecycle at ${Math.round(ageSeconds)}s, nearing the approved bound`);
    }
  }

  applyTo(state: RunState): void {
    state.guard.paused = this.paused;
    if (this.stoppedReason) state.guard.stopReason = this.stoppedReason;
    state.guard.reserved = this.reserved;
    state.guard.grossUnits = this.grossUnits.toString();
    state.guard.outstandingPrincipalUnits = this.outstandingUnits.toString();
  }
}
