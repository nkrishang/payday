import type { PaymentStatus } from "@payday/sdk";
import { isTerminalStatus } from "./checkout-state";

/**
 * How often the checkout re-reads the payment.
 *
 * The person who just paid is the case that has to feel instant, and that is
 * the one case we can detect precisely: their transaction receipt starts a
 * short window of fast polling. Everyone else — a payer sending from an
 * exchange, a merchant watching — is served fine by a few seconds. That is why
 * the payer route needs no long polling: it would hold open connections on an
 * unauthenticated endpoint to improve a case we already handle.
 *
 * `null` means stop.
 */

export const FAST_POLL_MS = 1_500;
export const DEFAULT_POLL_MS = 4_000;
export const IDLE_POLL_MS = 30_000;
/** How long after a send we keep polling fast, covering inclusion plus finality. */
export const SEND_WINDOW_MS = 90_000;
export const MAX_BACKOFF_MS = 30_000;

export interface PollInput {
  status: PaymentStatus;
  receivedBaseUnits: string;
  /** True when the tab is in the background; polling pauses and resumes on focus. */
  documentHidden: boolean;
  /** Milliseconds since this browser's own transfer was mined, or null. */
  msSinceSend: number | null;
}

export function pollDelayMs(input: PollInput): number | null {
  if (isTerminalStatus(input.status)) return null;

  // An expired payment that never received anything can no longer change.
  if (input.status === "expired" && BigInt(input.receivedBaseUnits) === 0n) return null;

  if (input.documentHidden) return null;

  if (input.msSinceSend !== null && input.msSinceSend < SEND_WINDOW_MS) return FAST_POLL_MS;

  if (input.status === "expired") return IDLE_POLL_MS;

  return DEFAULT_POLL_MS;
}

/** 1s, 2s, 4s, 8s, 16s, then a 30s ceiling. */
export function backoffMs(consecutiveFailures: number): number {
  if (consecutiveFailures <= 0) return 0;
  const exponential = 1_000 * 2 ** (consecutiveFailures - 1);
  return Math.min(exponential, MAX_BACKOFF_MS);
}

/** Spreads retries so every open checkout does not reconnect on the same tick. */
export function jitter(delayMs: number, random: number): number {
  const spread = delayMs * 0.2;
  return Math.round(delayMs - spread + random * spread * 2);
}
