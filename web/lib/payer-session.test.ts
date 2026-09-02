import { act, renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import {
  clearPayerSession,
  loadPayerSession,
  payerSessionKey,
  storePayerSession,
  usePayerSession,
} from "./payer-session";

describe("payer session storage", () => {
  beforeEach(() => {
    window.sessionStorage.clear();
    window.localStorage.clear();
  });

  it("keeps one token per payment in sessionStorage only", () => {
    storePayerSession("pay_1", "pps_one");
    storePayerSession("pay_2", "pps_two");

    expect(loadPayerSession("pay_1")).toBe("pps_one");
    expect(loadPayerSession("pay_2")).toBe("pps_two");
    expect(loadPayerSession("pay_3")).toBeNull();
    expect(window.sessionStorage.getItem(payerSessionKey("pay_1"))).toBe("pps_one");
    expect(window.localStorage.length).toBe(0);

    clearPayerSession("pay_1");
    expect(loadPayerSession("pay_1")).toBeNull();
    expect(loadPayerSession("pay_2")).toBe("pps_two");
  });

  it("starts empty on first render and picks the stored token up afterwards", async () => {
    storePayerSession("pay_1", "pps_stored");
    const { result } = renderHook(() => usePayerSession("pay_1"));

    // The effect has run by the time renderHook returns; the initial state
    // was null so server and client markup agree.
    expect(result.current[0]).toBe("pps_stored");

    await act(async () => result.current[1]("pps_new"));
    expect(result.current[0]).toBe("pps_new");
    expect(loadPayerSession("pay_1")).toBe("pps_new");

    await act(async () => result.current[1](null));
    expect(result.current[0]).toBeNull();
    expect(loadPayerSession("pay_1")).toBeNull();
  });
});
