import type { PaydayClient, VerificationAttempt, VerificationDetail } from "@payday/sdk";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { MerchantProvider } from "./session";
import { VerificationActivity } from "./verification-activity";

const EMAIL_ATTEMPT: VerificationAttempt = {
  id: "v-email",
  kind: "email",
  status: "approved",
  provider: "auth0",
  provider_reference: null,
  attempt_number: 1,
  document: "not_required",
  liveness: "not_required",
  identity_match: "not_required",
  risk_codes: [],
  country_code: null,
  verified_at: "2026-08-25T10:05:00Z",
  expires_at: null,
  created_at: "2026-08-25T10:04:00Z",
  review: null,
};

const IDENTITY_ATTEMPT: VerificationAttempt = {
  id: "v-identity",
  kind: "identity",
  status: "declined",
  provider: "didit",
  provider_reference: "9f1c0f6e-1111-4c1a-9c1e-000000000003",
  attempt_number: 1,
  document: "declined",
  liveness: "approved",
  identity_match: "pending",
  risk_codes: ["EXPECTED_DETAILS_MISMATCH", "DOCUMENT_EXPIRED"],
  country_code: "ESP",
  verified_at: null,
  expires_at: null,
  created_at: "2026-08-25T10:10:00Z",
  review: null,
};

const DECLINED: VerificationDetail = {
  payer_policy_mode: "verified_identity",
  verification_completed_at: null,
  likely_unsolicited_at: "2026-08-26T11:00:00Z",
  facts: {
    email: "approved",
    document: "declined",
    liveness: "approved",
    identity_match: "pending",
    complete: false,
  },
  attempts: [EMAIL_ATTEMPT, IDENTITY_ATTEMPT],
  review_available: true,
  retry_available: true,
};

function renderActivity(detail: VerificationDetail, after: VerificationDetail = detail) {
  const verification = vi.fn().mockResolvedValueOnce(detail).mockResolvedValue(after);
  const requestVerificationReview = vi.fn().mockResolvedValue(after);
  const client = {
    payments: { verification, requestVerificationReview },
  } as unknown as PaydayClient;
  render(
    <MerchantProvider
      value={{ client, accessToken: "eyJ.dash.token", email: "merchant@example.com", signOut: vi.fn() }}
    >
      <VerificationActivity paymentId="pay_1" />
    </MerchantProvider>,
  );
  return { verification, requestVerificationReview };
}

describe("VerificationActivity", () => {
  it("shows each fact separately, the provider reference and risk categories, and never extracted identity", async () => {
    renderActivity(DECLINED);

    expect(await screen.findByLabelText("Verification facts")).toBeInTheDocument();
    expect(screen.getByText("Email ownership")).toBeInTheDocument();
    expect(screen.getByText("Identity document")).toBeInTheDocument();
    expect(screen.getByText("Liveness and face match")).toBeInTheDocument();
    expect(screen.getByText("Name matches the invoice")).toBeInTheDocument();
    expect(screen.getAllByText("Declined").length).toBeGreaterThan(0);

    expect(screen.getByText("9f1c0f6e-1111-4c1a-9c1e-000000000003")).toBeInTheDocument();
    expect(screen.getByText("EXPECTED_DETAILS_MISMATCH")).toBeInTheDocument();
    expect(screen.getByText("DOCUMENT_EXPIRED")).toBeInTheDocument();
    expect(screen.getByText("ESP")).toBeInTheDocument();
    expect(screen.getByText(/may try the identity check once more/)).toBeInTheDocument();
    expect(document.body.textContent).not.toMatch(/SENTINEL|1900-01-01/);
  });

  it("requests a review and shows the reviewer's outcome once decided", async () => {
    const reviewed: VerificationDetail = {
      ...DECLINED,
      retry_available: false,
      attempts: [
        EMAIL_ATTEMPT,
        {
          ...IDENTITY_ATTEMPT,
          status: "review_required",
          review: {
            requested_at: "2026-08-26T12:00:00Z",
            decision: null,
            reviewer: null,
            note: null,
            decided_at: null,
          },
        },
      ],
    };
    const { requestVerificationReview } = renderActivity(DECLINED, reviewed);

    fireEvent.click(await screen.findByRole("button", { name: /request review/i }));
    await waitFor(() => expect(requestVerificationReview).toHaveBeenCalledWith("pay_1"));
    expect(await screen.findByText(/awaiting a reviewer/)).toBeInTheDocument();
    expect(screen.queryByText(/may try the identity check once more/)).not.toBeInTheDocument();
  });

  it("shows who decided a review, when, and with what note", async () => {
    const decided: VerificationDetail = {
      ...DECLINED,
      verification_completed_at: "2026-08-27T09:00:00Z",
      facts: {
        ...DECLINED.facts,
        document: "approved",
        identity_match: "approved",
        complete: true,
      },
      review_available: false,
      retry_available: false,
      attempts: [
        EMAIL_ATTEMPT,
        {
          ...IDENTITY_ATTEMPT,
          status: "approved",
          provider: "manual",
          document: "approved",
          identity_match: "approved",
          verified_at: "2026-08-27T09:00:00Z",
          review: {
            requested_at: "2026-08-26T12:00:00Z",
            decision: "approved",
            reviewer: "reviewer@payday.test",
            note: "Appeal upheld",
            decided_at: "2026-08-27T09:00:00Z",
          },
        },
      ],
    };
    renderActivity(decided);

    expect(await screen.findByText(/Approved by reviewer@payday\.test/)).toBeInTheDocument();
    expect(screen.getByText("Appeal upheld")).toBeInTheDocument();
    expect(screen.getByText("Manual review")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /request review/i })).not.toBeInTheDocument();
  });
});
