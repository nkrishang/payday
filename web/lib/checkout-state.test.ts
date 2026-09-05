import { describe, expect, it } from "vitest";
import { lockedPayment, merchantSessionPayment, payment } from "@/test/fixtures";
import { checkoutView, isTerminalStatus, unlockedDepositRequest } from "./checkout-state";

const open = { secondsRemaining: 3_600, pendingTxHash: null };

describe("checkoutView", () => {
  it("asks for the full amount while nothing has arrived", () => {
    const view = checkoutView(payment(), open);
    expect(view.phase).toBe("awaiting");
    expect(view.showInstructions).toBe(true);
    expect(view.isTerminal).toBe(false);
  });

  it("asks for the remainder once a partial transfer lands", () => {
    const view = checkoutView(
      payment({
        status: "partially_deposited",
        received: "10.00",
        received_base_units: "10000000",
        remaining: "15.00",
        remaining_base_units: "15000000",
      }),
      open,
    );
    expect(view.phase).toBe("partial");
    expect(view.title).toContain("15.00");
    expect(view.showInstructions).toBe(true);
  });

  it("hides the address once the countdown reaches zero, and does not call it expired", () => {
    const view = checkoutView(payment(), { ...open, secondsRemaining: 0 });
    expect(view.phase).toBe("closing");
    expect(view.showInstructions).toBe(false);
    expect(view.isTerminal).toBe(false);
    expect(view.title).not.toMatch(/expired/i);
  });

  it("hides the address as soon as the gateway stops considering it payable", () => {
    const view = checkoutView(payment({ payable: false }), open);
    expect(view.phase).toBe("closing");
    expect(view.showInstructions).toBe(false);
  });

  it("waits for finality after this browser sends a transfer", () => {
    const view = checkoutView(payment(), { ...open, pendingTxHash: "0xabc" });
    expect(view.phase).toBe("confirming");
    expect(view.showInstructions).toBe(false);
  });

  it("puts the deadline ahead of a pending transfer", () => {
    const view = checkoutView(payment(), { secondsRemaining: 0, pendingTxHash: "0xabc" });
    expect(view.phase).toBe("closing");
  });

  it("reports settlement as the terminal success", () => {
    const view = checkoutView(payment({ status: "settled" }), open);
    expect(view.phase).toBe("settled");
    expect(view.tone).toBe("success");
    expect(view.isTerminal).toBe(true);
    expect(view.showInstructions).toBe(false);
  });

  it("tells an exactly paid payer only that the requested amount reached the merchant", () => {
    const view = checkoutView(
      payment({
        status: "settled",
        payable: false,
        received: "25.00",
        received_base_units: "25000000",
        remaining: "0",
        remaining_base_units: "0",
      }),
      open,
    );
    expect(view.phase).toBe("settled");
    expect(view.detail).toMatch(/Exactly the requested amount reached the merchant/);
    expect(view.detail).not.toMatch(/recovery wallet/);
  });

  it("tells an overpaid payer where the remainder went once settled", () => {
    const view = checkoutView(
      payment({
        status: "settled",
        payable: false,
        received: "30.00",
        received_base_units: "30000000",
        remaining: "0",
        remaining_base_units: "0",
      }),
      open,
    );
    expect(view.phase).toBe("settled");
    expect(view.detail).toMatch(/Exactly the requested amount reached the merchant/);
    expect(view.detail).toMatch(/above the requested amount went to the Payday recovery wallet/);
    expect(view.detail).toMatch(/Payday support/);
    expect(view.detail).not.toMatch(/refund/i);
  });

  it("treats deposited as in-progress, because settlement has not happened yet", () => {
    const view = checkoutView(payment({ status: "deposited" }), open);
    expect(view.phase).toBe("deposited");
    expect(view.isTerminal).toBe(false);
    expect(view.detail).toMatch(/settling the requested amount to the merchant/);
  });

  it("separates an expired deposit request that received nothing from one holding funds", () => {
    expect(checkoutView(payment({ status: "expired", payable: false }), open).phase).toBe(
      "expired_empty",
    );

    const funded = checkoutView(
      payment({
        status: "expired",
        payable: false,
        received: "10.00",
        received_base_units: "10000000",
      }),
      open,
    );
    expect(funded.phase).toBe("expired_funded");
    expect(funded.detail).toMatch(/Payday recovery wallet/);
    expect(funded.detail).toMatch(/not automatically the payer/i);
    expect(funded.detail).toMatch(/Payday support/);
  });

  it("says plainly where returned funds went", () => {
    const view = checkoutView(payment({ status: "returned", payable: false }), open);
    expect(view.phase).toBe("returned");
    expect(view.detail).toMatch(/Payday recovery wallet/);
  });

  it("never calls the recovery wallet a refund address", () => {
    const statuses = ["partially_deposited", "expired", "returned"] as const;
    for (const status of statuses) {
      const view = checkoutView(
        payment({ status, payable: status === "partially_deposited", received_base_units: "1" }),
        open,
      );
      expect(view.detail, status).not.toMatch(/refund/i);
    }
  });

  it("shows the gateway's own words when settlement needs attention", () => {
    const view = checkoutView(
      payment({
        status: "needs_attention",
        payable: false,
        payer_message: "Payout is paused, but your funds remain safe.",
      }),
      open,
    );
    expect(view.phase).toBe("attention");
    expect(view.detail).toBe("Payout is paused, but your funds remain safe.");
    expect(view.showInstructions).toBe(false);
  });

  it("never shows deposit instructions in a state that is not payable", () => {
    const closed = ["settled", "returned", "expired", "needs_attention", "deposited"] as const;
    for (const status of closed) {
      const view = checkoutView(payment({ status, payable: false }), open);
      expect(view.showInstructions, status).toBe(false);
    }
  });
});

describe("checkoutView for a gated deposit request", () => {
  it("requires verification while the gateway withholds the content", () => {
    const view = checkoutView(lockedPayment(), open);
    expect(view.phase).toBe("verification_required");
    expect(view.showInstructions).toBe(false);
    expect(view.isTerminal).toBe(false);
  });

  it("puts the lock ahead of every lifecycle state, so a locked page never reports an outcome", () => {
    const statuses = [
      "partially_deposited",
      "deposited",
      "settled",
      "expired",
      "returned",
      "needs_attention",
    ] as const;
    for (const status of statuses) {
      const view = checkoutView(lockedPayment({ status, payable: false }), open);
      expect(view.phase, status).toBe("verification_required");
      expect(view.showInstructions, status).toBe(false);
    }
  });

  it("keeps the lock when the countdown ends or a transfer is pending", () => {
    expect(checkoutView(lockedPayment(), { secondsRemaining: 0, pendingTxHash: null }).phase).toBe(
      "verification_required",
    );
    expect(checkoutView(lockedPayment(), { ...open, pendingTxHash: "0xabc" }).phase).toBe(
      "verification_required",
    );
  });

  it("never mentions an amount, address, or attachment in the locked copy", () => {
    const view = checkoutView(lockedPayment(), open);
    expect(`${view.label} ${view.title} ${view.detail}`).not.toMatch(/25|0x9a3f|USDC|Globex/);
  });

  it("waits for the code once this tab asked for one", () => {
    const view = checkoutView(lockedPayment(), { ...open, emailCodeSent: true });
    expect(view.phase).toBe("email_pending");
    expect(view.detail).toContain("a****@e***.com");
    expect(view.showInstructions).toBe(false);
    expect(`${view.label} ${view.title} ${view.detail}`).not.toMatch(/25|0x9a3f|USDC|Globex/);
  });

  it("sends a merchant-session payer back to the app, and shows progress while the secret is exchanged", () => {
    const bare = checkoutView(merchantSessionPayment(), open);
    expect(bare.phase).toBe("app_required");
    expect(bare.title).toBe("Open this deposit request from Acme Corp");
    expect(bare.showInstructions).toBe(false);
    expect(`${bare.label} ${bare.title} ${bare.detail}`).not.toMatch(
      /email|code|25|0x9a3f|USDC|Globex/,
    );

    const opening = checkoutView(merchantSessionPayment(), {
      ...open,
      exchangingClientSecret: true,
    });
    expect(opening.phase).toBe("app_opening");
    expect(opening.tone).toBe("progress");
    expect(opening.showInstructions).toBe(false);

    // A code sent in this tab means nothing for this mode; the API's own
    // completion without a session does not open it either.
    expect(checkoutView(merchantSessionPayment(), { ...open, emailCodeSent: true }).phase).toBe(
      "app_required",
    );
    expect(
      checkoutView(
        merchantSessionPayment({
          requirements: { email: "not_required", merchant_session: "approved", complete: true },
        }),
        open,
      ).phase,
    ).toBe("app_required");
  });

  it("still asks the bare link to verify after another session completed the deposit request", () => {
    // The API reports the deposit request's own completion without a session; this
    // tab has no session, so it must verify itself.
    const view = checkoutView(
      lockedPayment({
        requirements: { email: "approved", merchant_session: "not_required", complete: true },
      }),
      open,
    );
    expect(view.phase).toBe("verification_required");
  });
});

describe("unlockedDepositRequest", () => {
  it("returns the mechanics when the gateway unlocked them", () => {
    const unlocked = unlockedDepositRequest(payment());
    expect(unlocked?.address).toBe("0x9a3f0000000000000000000000000000000000c2");
    expect(unlocked?.token.symbol).toBe("USDC");
  });

  it("treats a locked response as locked regardless of status", () => {
    expect(unlockedDepositRequest(lockedPayment())).toBeNull();
  });

  it("treats a response that claims to be unlocked but lacks a mechanic as locked", () => {
    // The API nulls every gated field together; a response that disagrees with
    // its own flag must not be rendered with holes.
    expect(unlockedDepositRequest(payment({ address: null } as never))).toBeNull();
    expect(unlockedDepositRequest(payment({ token: null } as never))).toBeNull();
  });
});

describe("isTerminalStatus", () => {
  it("covers exactly the statuses that stop polling", () => {
    expect(isTerminalStatus("settled")).toBe(true);
    expect(isTerminalStatus("returned")).toBe(true);
    expect(isTerminalStatus("needs_attention")).toBe(true);
    expect(isTerminalStatus("awaiting_deposit")).toBe(false);
    expect(isTerminalStatus("expired")).toBe(false);
  });
});
