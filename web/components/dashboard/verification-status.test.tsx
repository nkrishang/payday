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
        mode="verified_identity"
        completedAt={null}
        unsolicitedAt="2026-08-26T11:00:00Z"
      />,
    );
    expect(screen.getByText("Pending")).toBeInTheDocument();
    expect(screen.getByText("Likely unsolicited")).toBeInTheDocument();
  });
});

describe("VerificationStatus", () => {
  it("shows the merchant's own assertions for a matched identity policy", () => {
    render(
      <VerificationStatus
        policy={{
          mode: "verified_identity",
          expected_email: "alice@example.com",
          expected_identity: { first_name: "Alice", last_name: "Smith" },
        }}
        completedAt="2026-08-19T08:30:00Z"
        unsolicitedAt={null}
      />,
    );
    expect(screen.getByText("Verified identity")).toBeInTheDocument();
    expect(screen.getByText("alice@example.com")).toBeInTheDocument();
    expect(screen.getByText("Alice Smith")).toBeInTheDocument();
    expect(screen.getByText("Verified")).toBeInTheDocument();
    expect(screen.queryByRole("note")).not.toBeInTheDocument();
  });

  it("explains an unattributed policy and a likely unsolicited payment", () => {
    render(
      <VerificationStatus
        policy={{ mode: "verified_identity_unattributed", expected_email: "bob@example.com" }}
        completedAt={null}
        unsolicitedAt="2026-08-26T11:00:00Z"
      />,
    );
    expect(screen.getByText(/confirms a real person, not who they are/)).toBeInTheDocument();
    expect(screen.getByRole("note")).toHaveTextContent(/Likely unsolicited/);
    expect(screen.getByRole("note")).toHaveTextContent(/not quarantined/);
    expect(screen.queryByText("Expected identity")).not.toBeInTheDocument();
  });
});
