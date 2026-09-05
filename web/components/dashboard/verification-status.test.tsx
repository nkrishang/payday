import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { VerificationBadge, VerificationStatus } from "./verification-status";

describe("VerificationBadge", () => {
  it("says a permissionless invoice needs nothing", () => {
    render(<VerificationBadge mode="permissionless" completedAt={null} unsolicitedAt={null} />);
    expect(screen.getByText("Not required")).toBeInTheDocument();
  });

  it("keeps a gated invoice pending until the gateway records completion", () => {
    const { rerender } = render(
      <VerificationBadge mode="verified_email" completedAt={null} unsolicitedAt={null} />,
    );
    expect(screen.getByText("Pending")).toBeInTheDocument();

    rerender(
      <VerificationBadge
        mode="verified_email"
        completedAt="2026-08-19T08:30:00Z"
        unsolicitedAt={null}
      />,
    );
    expect(screen.getByText("Verified")).toBeInTheDocument();
  });

  it("flags funds that arrived before verification without changing the verification verdict", () => {
    render(
      <VerificationBadge
        mode="verified_email"
        completedAt={null}
        unsolicitedAt="2026-08-26T11:00:00Z"
      />,
    );
    expect(screen.getByText("Pending")).toBeInTheDocument();
    expect(screen.getByText("Likely unsolicited")).toBeInTheDocument();
  });
});

describe("VerificationStatus", () => {
  it("shows the app's own payer reference for a merchant-session policy, and no email", () => {
    render(
      <VerificationStatus
        policy={{ mode: "merchant_session", payer_reference: "user_123" }}
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

  it("keeps a merchant-session request pending until its app opens it", () => {
    render(<VerificationBadge mode="merchant_session" completedAt={null} unsolicitedAt={null} />);
    expect(screen.getByText("Pending")).toBeInTheDocument();
  });

  it("shows the merchant's own assertion for a verified email policy", () => {
    render(
      <VerificationStatus
        policy={{ mode: "verified_email", expected_email: "alice@example.com" }}
        completedAt="2026-08-19T08:30:00Z"
        unsolicitedAt={null}
      />,
    );
    expect(screen.getByText("Verified email")).toBeInTheDocument();
    expect(screen.getByText("alice@example.com")).toBeInTheDocument();
    expect(screen.getByText("Verified")).toBeInTheDocument();
    expect(screen.queryByRole("note")).not.toBeInTheDocument();
  });

  it("explains a likely unsolicited payment", () => {
    render(
      <VerificationStatus
        policy={{ mode: "verified_email", expected_email: "bob@example.com" }}
        completedAt={null}
        unsolicitedAt="2026-08-26T11:00:00Z"
      />,
    );
    expect(screen.getByRole("note")).toHaveTextContent(/Likely unsolicited/);
    expect(screen.getByRole("note")).toHaveTextContent(/a wallet other than the one the payer signed with/);
    expect(screen.queryByText("Expected identity")).not.toBeInTheDocument();
  });
});
