import type { PayerPayment } from "@payday/sdk";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { payerClient } from "@/lib/payday";
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

const IDENTITY_APPROVED_EMAIL = {
  email: "approved",
  document: "pending",
  liveness: "pending",
  identity_match: "pending",
  complete: false,
} as const;

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
    expect(screen.getByText(/does not prove ownership of the wallet/i)).toBeInTheDocument();
  });

  it("lists only the facts the policy needs, distinguishing matched from unattributed identity", () => {
    const { rerender } = render(gate(lockedPayment()));
    expect(screen.getByText("Email ownership")).toBeInTheDocument();
    expect(screen.queryByText("Identity document")).not.toBeInTheDocument();

    rerender(
      gate(
        lockedPayment({
          payer_policy: { mode: "verified_identity", expected_email_hint: "a****@e***.com" },
          requirements: {
            email: "pending",
            document: "pending",
            liveness: "pending",
            identity_match: "pending",
            complete: false,
          },
        }),
      ),
    );
    expect(screen.getByText("Identity document")).toBeInTheDocument();
    expect(screen.getByText("Name matches the invoice")).toBeInTheDocument();
    expect(screen.getByText(/matching the person it names/)).toBeInTheDocument();

    rerender(
      gate(
        lockedPayment({
          payer_policy: {
            mode: "verified_identity_unattributed",
            expected_email_hint: "a****@e***.com",
          },
          requirements: {
            email: "pending",
            document: "pending",
            liveness: "pending",
            identity_match: "not_required",
            complete: false,
          },
        }),
      ),
    );
    expect(screen.getByText("Identity document")).toBeInTheDocument();
    expect(screen.queryByText("Name matches the invoice")).not.toBeInTheDocument();
    expect(screen.queryByText(/matching the person it names/)).not.toBeInTheDocument();
  });

  it("moves on to the identity step once the email is approved, without offering the code again", async () => {
    vi.spyOn(payerClient.verification, "status").mockResolvedValue({
      requirements: IDENTITY_APPROVED_EMAIL,
      identity_start_available: true,
      identity: null,
    });
    render(
      gate(
        lockedPayment({
          payer_policy: { mode: "verified_identity", expected_email_hint: "a****@e***.com" },
          requirements: IDENTITY_APPROVED_EMAIL,
        }),
        open,
        "pps_1",
      ),
    );

    expect(
      screen.getByRole("heading", { name: /verify your identity to view this invoice/i }),
    ).toBeInTheDocument();
    expect(screen.getByText("Approved")).toBeInTheDocument();
    expect(
      await screen.findByRole("button", { name: /continue to identity verification/i }),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /send a code/i })).not.toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("asks for the email in this tab before the identity step when no session is held", () => {
    render(
      gate(
        lockedPayment({
          payer_policy: { mode: "verified_identity", expected_email_hint: "a****@e***.com" },
          requirements: IDENTITY_APPROVED_EMAIL,
        }),
      ),
    );
    expect(screen.getByText(/verify the expected email in this tab/i)).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });
});
