import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { lockedPayment } from "@/test/fixtures";
import { VerificationGate } from "./verification-gate";

describe("VerificationGate", () => {
  it("shows the issuer, the heading, and the masked mailbox and nothing else", () => {
    const { container } = render(
      <VerificationGate payment={lockedPayment({ heading: "Consulting — August" })} />,
    );

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("Acme Corp");
    expect(screen.getByText("Consulting — August")).toBeInTheDocument();
    expect(screen.getByText("a****@e***.com")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: /verification required/i })).toBeInTheDocument();
    // The masked hint is the only mailbox-shaped string on the page.
    expect(container.textContent).not.toMatch(/alice|example\.com/);
    expect(container.textContent).not.toMatch(/25|USDC|0x9a3f|Globex/);
  });

  it("says the flow is being enabled rather than offering a form", () => {
    render(<VerificationGate payment={lockedPayment()} />);

    expect(screen.getByText(/verification is being enabled/i)).toBeInTheDocument();
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("lists only the facts the policy needs, distinguishing matched from unattributed identity", () => {
    const { rerender } = render(<VerificationGate payment={lockedPayment()} />);
    expect(screen.getByText("Email ownership")).toBeInTheDocument();
    expect(screen.queryByText("Identity document")).not.toBeInTheDocument();

    rerender(
      <VerificationGate
        payment={lockedPayment({
          payer_policy: { mode: "verified_identity", expected_email_hint: "a****@e***.com" },
          requirements: {
            email: "pending",
            document: "pending",
            liveness: "pending",
            identity_match: "pending",
            complete: false,
          },
        })}
      />,
    );
    expect(screen.getByText("Identity document")).toBeInTheDocument();
    expect(screen.getByText("Name matches the invoice")).toBeInTheDocument();
    expect(screen.getByText(/matching the person it names/)).toBeInTheDocument();

    rerender(
      <VerificationGate
        payment={lockedPayment({
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
        })}
      />,
    );
    expect(screen.getByText("Identity document")).toBeInTheDocument();
    expect(screen.queryByText("Name matches the invoice")).not.toBeInTheDocument();
    expect(screen.queryByText(/matching the person it names/)).not.toBeInTheDocument();
  });
});
