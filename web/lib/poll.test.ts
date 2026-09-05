import { describe, expect, it } from "vitest";
import {
  DEFAULT_POLL_MS,
  FAST_POLL_MS,
  IDLE_POLL_MS,
  MAX_BACKOFF_MS,
  backoffMs,
  jitter,
  pollDelayMs,
} from "./poll";

const base = {
  status: "awaiting_deposit",
  receivedBaseUnits: "0",
  documentHidden: false,
  msSinceSend: null,
} as const;

describe("pollDelayMs", () => {
  it("stops on states that can no longer change", () => {
    expect(pollDelayMs({ ...base, status: "settled" })).toBeNull();
    expect(pollDelayMs({ ...base, status: "returned" })).toBeNull();
    expect(pollDelayMs({ ...base, status: "needs_attention" })).toBeNull();
  });

  it("stops for an expired deposit request that never received anything", () => {
    expect(pollDelayMs({ ...base, status: "expired", receivedBaseUnits: "0" })).toBeNull();
  });

  it("keeps watching an expired deposit request that holds funds, since recovery follows", () => {
    expect(pollDelayMs({ ...base, status: "expired", receivedBaseUnits: "1" })).toBe(IDLE_POLL_MS);
  });

  it("pauses in a background tab", () => {
    expect(pollDelayMs({ ...base, documentHidden: true })).toBeNull();
    expect(pollDelayMs({ ...base, documentHidden: true, msSinceSend: 1_000 })).toBeNull();
  });

  it("polls fast for a short window after this browser sent a transfer", () => {
    expect(pollDelayMs({ ...base, msSinceSend: 0 })).toBe(FAST_POLL_MS);
    expect(pollDelayMs({ ...base, msSinceSend: 89_999 })).toBe(FAST_POLL_MS);
    expect(pollDelayMs({ ...base, msSinceSend: 90_000 })).toBe(DEFAULT_POLL_MS);
  });

  it("otherwise uses the default cadence", () => {
    expect(pollDelayMs(base)).toBe(DEFAULT_POLL_MS);
    expect(pollDelayMs({ ...base, status: "partially_deposited" })).toBe(DEFAULT_POLL_MS);
    expect(pollDelayMs({ ...base, status: "deposited" })).toBe(DEFAULT_POLL_MS);
  });
});

describe("backoffMs", () => {
  it("doubles up to a ceiling", () => {
    expect(backoffMs(0)).toBe(0);
    expect(backoffMs(1)).toBe(1_000);
    expect(backoffMs(2)).toBe(2_000);
    expect(backoffMs(5)).toBe(16_000);
    expect(backoffMs(6)).toBe(MAX_BACKOFF_MS);
    expect(backoffMs(50)).toBe(MAX_BACKOFF_MS);
  });
});

describe("jitter", () => {
  it("spreads within twenty percent either way", () => {
    expect(jitter(1_000, 0)).toBe(800);
    expect(jitter(1_000, 0.5)).toBe(1_000);
    expect(jitter(1_000, 1)).toBe(1_200);
  });
});
