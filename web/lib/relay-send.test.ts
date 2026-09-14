import { beforeEach, describe, expect, it, vi } from "vitest";
import { RelaySendRetrier, pendingRelayReport, recordRelayBroadcast } from "./relay-send";

const entry = { intentId: "rli_1", originChainId: "137", hash: "0xabc", at: 42 };

describe("the Relay send journal", () => {
  beforeEach(() => sessionStorage.clear());

  it("survives a page reload because the broadcast is stored in the tab", () => {
    recordRelayBroadcast("dr_1", entry);
    expect(pendingRelayReport("dr_1")).toEqual(entry);
  });

  it("posts a pending report and clears it once accepted", async () => {
    recordRelayBroadcast("dr_1", entry);
    const send = vi.fn().mockResolvedValue({});
    expect(await new RelaySendRetrier(send).attempt("dr_1")).toBe(true);
    expect(send).toHaveBeenCalledWith(entry);
    expect(pendingRelayReport("dr_1")).toBeNull();
  });

  it("keeps a failed report and backs off before trying again", async () => {
    recordRelayBroadcast("dr_1", entry);
    const retrier = new RelaySendRetrier(vi.fn().mockRejectedValue(new Error("offline")));
    expect(await retrier.attempt("dr_1")).toBe(false);
    expect(retrier.delay).toBe(1_000);
    expect(pendingRelayReport("dr_1")).toEqual(entry);
  });
});
