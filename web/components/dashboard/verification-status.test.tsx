import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { VerificationBadge, VerificationStatus } from "./verification-status";

const EMAIL_ONLY = { email: { expected_email: "alice@example.com" }, wallet_attestation: false };
const MERCHANT_ONLY = { merchant_auth: { payer_reference: "user_123" }, wallet_attestation: false };

describe("VerificationBadge", () => {
  it("says a request with no identity add-on needs nothing", () => {
    render(
      <VerificationBadge
        verification={{ wallet_attestation: true }}
        completedAt={null}
        unsolicitedAt={null}
      />,
    );
    expect(screen.getByText("Not required")).toBeInTheDocument();
  });

  it("keeps an identity-gated request pending until the gateway records completion", () => {
    const { rerender } = render(
      <VerificationBadge verification={EMAIL_ONLY} completedAt={null} unsolicitedAt={null} />,
    );
    expect(screen.getByText("Pending")).toBeInTheDocument();

    rerender(
      <VerificationBadge
        verification={EMAIL_ONLY}
        completedAt="2026-08-19T08:30:00Z"
        unsolicitedAt={null}
      />,
    );
    expect(screen.getByText("Verified")).toBeInTheDocument();
  });

  it("flags funds that arrived before verification without changing the verification verdict", () => {
    render(
      <VerificationBadge
        verification={EMAIL_ONLY}
        completedAt={null}
        unsolicitedAt="2026-08-26T11:00:00Z"
      />,
    );
    expect(screen.getByText("Pending")).toBeInTheDocument();
    expect(screen.getByText("Likely unsolicited")).toBeInTheDocument();
  });
});

describe("VerificationStatus", () => {
  it("shows the app's own payer reference for merchant auth, and no email", () => {
    render(
      <VerificationStatus
        verification={MERCHANT_ONLY}
        completedAt="2026-08-19T08:30:00Z"
        unsolicitedAt={null}
      />,
    );
    expect(screen.getByText("Your app's sign-in")).toBeInTheDocument();
    expect(screen.getByText("Payer reference")).toBeInTheDocument();
    expect(screen.getByText("user_123")).toBeInTheDocument();
    expect(screen.queryByText("Expected email")).not.toBeInTheDocument();
    expect(screen.getByText("Verified")).toBeInTheDocument();
  });

  it("keeps a merchant-auth request pending until its app opens it", () => {
    render(
      <VerificationBadge verification={MERCHANT_ONLY} completedAt={null} unsolicitedAt={null} />,
    );
    expect(screen.getByText("Pending")).toBeInTheDocument();
  });

  it("shows the expected mailbox for an email-gated request", () => {
    render(
      <VerificationStatus
        verification={EMAIL_ONLY}
        completedAt="2026-08-19T08:30:00Z"
        unsolicitedAt={null}
      />,
    );
    expect(screen.getByText("Verified email")).toBeInTheDocument();
    expect(screen.getByText("alice@example.com")).toBeInTheDocument();
    expect(screen.getByText("Verified")).toBeInTheDocument();
    expect(screen.queryByRole("note")).not.toBeInTheDocument();
  });

  it("shows wallet attestation as its own row, separate from the identity verdict", () => {
    render(
      <VerificationStatus
        verification={{ wallet_attestation: true }}
        completedAt={null}
        unsolicitedAt={null}
      />,
    );
    expect(screen.getAllByText("Wallet attestation").length).toBeGreaterThan(0);
    expect(screen.getByText("Not required")).toBeInTheDocument();
  });

  it("explains a likely unsolicited deposit", () => {
    render(
      <VerificationStatus
        verification={{ email: { expected_email: "bob@example.com" }, wallet_attestation: true }}
        completedAt={null}
        unsolicitedAt="2026-08-26T11:00:00Z"
      />,
    );
    expect(screen.getByRole("note")).toHaveTextContent(/Likely unsolicited/);
    expect(screen.getByRole("note")).toHaveTextContent(/a wallet other than the one the payer signed with/);
    expect(screen.queryByText("Expected identity")).not.toBeInTheDocument();
  });
});
