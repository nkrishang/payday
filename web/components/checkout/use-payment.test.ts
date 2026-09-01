import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { payment } from "@/test/fixtures";
import { useSecondsRemaining } from "./use-payment";

/** The gateway said it was this time when it answered. */
const SERVER_NOW = 1_788_000_000;
const ONE_HOUR = 3_600;

const open = payment({
  server_timestamp: String(SERVER_NOW),
  expires_at: new Date((SERVER_NOW + ONE_HOUR) * 1000).toISOString(),
});

describe("useSecondsRemaining", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  afterEach(() => {
    vi.useRealTimers();
  });

  it("counts down from the gateway's clock, not the device's", () => {
    // This device is five hours ahead of the gateway. The deadline is still an
    // hour away, and the countdown must say so.
    const deviceNow = (SERVER_NOW + 5 * ONE_HOUR) * 1000;
    vi.setSystemTime(deviceNow);

    const { result } = renderHook(() => useSecondsRemaining(open, deviceNow));

    expect(result.current).toBe(ONE_HOUR);
  });

  it("is unaffected by a device clock running behind", () => {
    const deviceNow = (SERVER_NOW - 3 * ONE_HOUR) * 1000;
    vi.setSystemTime(deviceNow);

    const { result } = renderHook(() => useSecondsRemaining(open, deviceNow));

    expect(result.current).toBe(ONE_HOUR);
  });

  it("ticks down once a second", () => {
    const deviceNow = SERVER_NOW * 1000;
    vi.setSystemTime(deviceNow);

    const { result } = renderHook(() => useSecondsRemaining(open, deviceNow));
    expect(result.current).toBe(ONE_HOUR);

    act(() => {
      vi.advanceTimersByTime(90_000);
    });

    expect(result.current).toBe(ONE_HOUR - 90);
  });

  it("re-anchors on a fresh read, so drift cannot accumulate", () => {
    const deviceNow = SERVER_NOW * 1000;
    vi.setSystemTime(deviceNow);

    const { result, rerender } = renderHook(
      ({ p, at }) => useSecondsRemaining(p, at),
      { initialProps: { p: open, at: deviceNow } },
    );

    // Half an hour of local time passes, then a poll returns a payment whose
    // server clock says only ten minutes went by. The gateway wins.
    act(() => {
      vi.advanceTimersByTime(1_800_000);
    });
    const later = Date.now();
    rerender({
      p: payment({
        server_timestamp: String(SERVER_NOW + 600),
        expires_at: open.expires_at,
      }),
      at: later,
    });

    expect(result.current).toBe(ONE_HOUR - 600);
  });

  it("floors at zero rather than counting past the deadline", () => {
    const deviceNow = SERVER_NOW * 1000;
    vi.setSystemTime(deviceNow);

    const { result } = renderHook(() => useSecondsRemaining(open, deviceNow));

    act(() => {
      vi.advanceTimersByTime((ONE_HOUR + 120) * 1000);
    });

    expect(result.current).toBe(0);
  });
});
