import { describe, expect, it } from "vitest";
import { payment } from "@/test/fixtures";
import { checkoutView, isTerminalStatus } from "./checkout-state";

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
        status: "partially_paid",
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

  it("tells an exactly paid payer only that the invoice amount reached the merchant", () => {
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
    expect(view.detail).toMatch(/Exactly the invoice amount reached the merchant/);
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
    expect(view.detail).toMatch(/Exactly the invoice amount reached the merchant/);
    expect(view.detail).toMatch(/above the invoice amount went to the Payday recovery wallet/);
    expect(view.detail).toMatch(/Payday support/);
    expect(view.detail).not.toMatch(/refund/i);
  });

  it("treats paid as in-progress, because settlement has not happened yet", () => {
    const view = checkoutView(payment({ status: "paid" }), open);
    expect(view.phase).toBe("paid");
    expect(view.isTerminal).toBe(false);
    expect(view.detail).toMatch(/settling the invoice amount to the merchant/);
  });

  it("separates an expired payment that received nothing from one holding funds", () => {
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
    const statuses = ["partially_paid", "expired", "returned"] as const;
    for (const status of statuses) {
      const view = checkoutView(
        payment({ status, payable: status === "partially_paid", received_base_units: "1" }),
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

  it("never shows payment instructions in a state that is not payable", () => {
    const closed = ["settled", "returned", "expired", "needs_attention", "paid"] as const;
    for (const status of closed) {
      const view = checkoutView(payment({ status, payable: false }), open);
      expect(view.showInstructions, status).toBe(false);
    }
  });
});

describe("isTerminalStatus", () => {
  it("covers exactly the statuses that stop polling", () => {
    expect(isTerminalStatus("settled")).toBe(true);
    expect(isTerminalStatus("returned")).toBe(true);
    expect(isTerminalStatus("needs_attention")).toBe(true);
    expect(isTerminalStatus("awaiting_payment")).toBe(false);
    expect(isTerminalStatus("expired")).toBe(false);
  });
});
