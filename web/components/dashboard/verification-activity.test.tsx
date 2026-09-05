import type { PaydayClient, VerificationAttempt, VerificationDetail } from "@payday/sdk";
import { render, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { MerchantProvider } from "./session";
import { VerificationActivity } from "./verification-activity";

const ABANDONED_ATTEMPT: VerificationAttempt = {
  id: "v-abandoned",
  kind: "email",
  status: "abandoned",
  verified_at: null,
  created_at: "2026-08-25T10:01:00Z",
};

const EMAIL_ATTEMPT: VerificationAttempt = {
  id: "v-email",
  kind: "email",
  status: "approved",
  verified_at: "2026-08-25T10:05:00Z",
  created_at: "2026-08-25T10:04:00Z",
};

const VERIFIED: VerificationDetail = {
  payer_policy_mode: "verified_email",
  verification_completed_at: "2026-08-25T10:05:00Z",
  likely_unsolicited_at: null,
  facts: { email: "approved", complete: true },
  attempts: [ABANDONED_ATTEMPT, EMAIL_ATTEMPT],
};

function renderActivity(detail: VerificationDetail) {
  const verification = vi.fn().mockResolvedValue(detail);
  const client = { payments: { verification } } as unknown as PaydayClient;
  render(
    <MerchantProvider value={{ client, accessToken: "eyJ.dash.token", signOut: vi.fn() }}>
      <VerificationActivity paymentId="pay_1" />
    </MerchantProvider>,
  );
  return { verification };
}

describe("VerificationActivity", () => {
  it("lists every attempt with its status and time, and nothing about the session", async () => {
    const { verification } = renderActivity(VERIFIED);

    expect(await screen.findByLabelText("Verification activity")).toBeInTheDocument();
    expect(verification).toHaveBeenCalledWith("pay_1");
    expect(screen.getAllByText("Email verification")).toHaveLength(2);
    expect(screen.getByText("Abandoned")).toBeInTheDocument();
    expect(screen.getByText("Approved")).toBeInTheDocument();
    expect(screen.getByText("Verified")).toBeInTheDocument();
    expect(screen.getByText("Started")).toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/pps_|payer_ref|provider/);
  });

  it("renders nothing while no attempt has been made", async () => {
    const { verification } = renderActivity({
      ...VERIFIED,
      verification_completed_at: null,
      facts: { email: "pending", complete: false },
      attempts: [],
    });

    await vi.waitFor(() => expect(verification).toHaveBeenCalled());
    await vi.waitFor(() => expect(screen.queryByRole("status")).not.toBeInTheDocument());
    expect(screen.queryByLabelText("Verification activity")).not.toBeInTheDocument();
  });
});
