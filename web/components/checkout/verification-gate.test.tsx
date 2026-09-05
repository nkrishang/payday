import type { PayerPayment } from "@payday/sdk";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { checkoutView } from "@/lib/checkout-state";
import { lockedPayment } from "@/test/fixtures";
import { VerificationGate } from "./verification-gate";

const open = { secondsRemaining: 3_600, pendingTxHash: null };

function gate(payment: PayerPayment, local = open, payerSession: string | null = null) {
  return (
    <VerificationGate
      payment={payment}
      view={checkoutView(payment, local)}
      payerSession={payerSession}
      onSession={vi.fn()}
      onCodeSent={vi.fn()}
      onVerified={vi.fn()}
    />
  );
}

describe("VerificationGate", () => {
  it("shows the issuer, the heading, and the masked mailbox and nothing else", () => {
    const { container } = render(gate(lockedPayment({ heading: "Consulting — August" })));

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Acme Corp");
    expect(screen.getByText("Consulting — August")).toBeInTheDocument();
    expect(screen.getByText("a****@e***.com")).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: /verify to view this invoice/i }),
    ).toBeInTheDocument();
    // The masked hint is the only mailbox-shaped string on the page.
    expect(container.textContent).not.toMatch(/alice|example\.com/);
    expect(container.textContent).not.toMatch(/25|USDC|0x9a3f|Globex/);
  });

  it("offers to send the code, and no field to type an email into", () => {
    render(gate(lockedPayment()));

    expect(
      screen.getByRole("button", { name: /send a code to a\*\*\*\*@e\*\*\*\.com/i }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("explains that the content opens once the expected mailbox is verified", () => {
    render(gate(lockedPayment()));
    expect(screen.getByText(/once the payer verifies the email address/i)).toBeInTheDocument();
    expect(screen.queryByText(/identity/i)).not.toBeInTheDocument();
  });
});
