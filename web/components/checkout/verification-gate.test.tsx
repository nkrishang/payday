import type { PayerDepositRequest } from "@payday/sdk";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { checkoutView, type CheckoutLocalState } from "@/lib/checkout-state";
import { lockedPayment, merchantSessionPayment } from "@/test/fixtures";
import type { ClientSecretStatus } from "./merchant-session";
import { VerificationGate } from "./verification-gate";

const open: CheckoutLocalState = { secondsRemaining: 3_600, pendingTxHash: null };

function gate(
  payment: PayerDepositRequest,
  local = open,
  payerSession: string | null = null,
  clientSecretStatus: ClientSecretStatus = "none",
) {
  return (
    <VerificationGate
      payment={payment}
      view={checkoutView(payment, local)}
      payerSession={payerSession}
      clientSecretStatus={clientSecretStatus}
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
      screen.getByRole("heading", { name: /verify to view this deposit request/i }),
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

  it("tells a merchant-session payer to open it from the app, with nothing to type or click", () => {
    const { container } = render(gate(merchantSessionPayment({ heading: "Deposit 500 USDC" })));

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Acme Corp");
    expect(screen.getByText("Deposit request from")).toBeInTheDocument();
    expect(
      screen.getByRole("heading", { name: /open this deposit request from acme corp/i }),
    ).toBeInTheDocument();
    expect(screen.getByText(/sign in there and open it again/i)).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(container.textContent).not.toMatch(/email|code/i);
    expect(container.textContent).not.toMatch(/25|0x9a3f|Globex/);
  });

  it("shows progress while the client secret is being exchanged", () => {
    render(
      gate(merchantSessionPayment(), { ...open, exchangingClientSecret: true }, null, "exchanging"),
    );
    expect(screen.getByRole("status")).toHaveTextContent(/opening your deposit request/i);
    expect(screen.getByRole("heading", { name: /opening your deposit request/i })).toBeInTheDocument();
  });

  it("says when the link was already opened, and where to go", () => {
    render(gate(merchantSessionPayment(), open, null, "used"));
    expect(screen.getByText(/this link was already opened/i)).toBeInTheDocument();
    expect(screen.getByText(/go back to acme corp/i)).toBeInTheDocument();
  });

  it("says when the link has expired", () => {
    render(gate(merchantSessionPayment(), open, null, "invalid"));
    expect(screen.getByText(/expired or is not valid/i)).toBeInTheDocument();
  });
});
