import { PaydayError, type VerificationStatus } from "@payday/sdk";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { payerClient } from "@/lib/payday";
import { IdentityVerification } from "./identity-verification";

const REQUIRED = {
  email: "approved",
  document: "pending",
  liveness: "pending",
  identity_match: "pending",
  complete: false,
} as const;

function status(overrides: Partial<VerificationStatus> = {}): VerificationStatus {
  return {
    requirements: REQUIRED,
    identity_start_available: true,
    identity: null,
    ...overrides,
  };
}

function renderStep(overrides: Partial<Parameters<typeof IdentityVerification>[0]> = {}): {
  navigate: ReturnType<typeof vi.fn>;
  onVerified: ReturnType<typeof vi.fn>;
  onSession: ReturnType<typeof vi.fn>;
} {
  const navigate = vi.fn();
  const onVerified = vi.fn();
  const onSession = vi.fn();
  render(
    <IdentityVerification
      paymentId="pay_1"
      mode="verified_identity"
      payerSession="pps_1"
      onSession={onSession}
      onVerified={onVerified}
      navigate={navigate}
      {...overrides}
    />,
  );
  return { navigate, onVerified, onSession };
}

describe("IdentityVerification", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("explains a matched check, links both privacy notices, and sends the payer to the hosted session", async () => {
    vi.spyOn(payerClient.verification, "status").mockResolvedValue(status());
    const start = vi
      .spyOn(payerClient.verification, "startIdentity")
      .mockResolvedValue({ outcome: { type: "redirect", url: "https://verify.example/s/1" } });
    const { navigate, onVerified } = renderStep();

    const button = await screen.findByRole("button", {
      name: /continue to identity verification/i,
    });
    expect(screen.getByText(/matches the person this invoice names/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: /payday privacy notice/i })).toHaveAttribute(
      "href",
      "https://payday.sh/privacy",
    );
    expect(screen.getByRole("link", { name: /didit privacy notice/i })).toHaveAttribute(
      "href",
      "https://didit.me/privacy-policy",
    );
    expect(screen.getByText(/does not prove ownership of the wallet/)).toBeInTheDocument();
    expect(
      screen.getByText(/never your document, photos, name, or date of birth/),
    ).toBeInTheDocument();

    fireEvent.click(button);
    await waitFor(() => expect(navigate).toHaveBeenCalledWith("https://verify.example/s/1"));
    expect(start).toHaveBeenCalledWith("pay_1", "pps_1");
    expect(onVerified).not.toHaveBeenCalled();
  });

  it("describes an unattributed check as proving a person, not who they are", async () => {
    vi.spyOn(payerClient.verification, "status").mockResolvedValue(status());
    renderStep({ mode: "verified_identity_unattributed" });

    await screen.findByRole("button", { name: /continue to identity verification/i });
    expect(screen.getByText(/does not tell the merchant who you are/)).toBeInTheDocument();
    expect(screen.queryByText(/matches the person this invoice names/)).not.toBeInTheDocument();
  });

  it("unlocks at once when an earlier credential is reused", async () => {
    vi.spyOn(payerClient.verification, "status").mockResolvedValue(status());
    vi.spyOn(payerClient.verification, "startIdentity").mockResolvedValue({
      outcome: { type: "reused" },
    });
    const { navigate, onVerified } = renderStep();

    fireEvent.click(
      await screen.findByRole("button", { name: /continue to identity verification/i }),
    );
    await waitFor(() => expect(onVerified).toHaveBeenCalled());
    expect(navigate).not.toHaveBeenCalled();
  });

  it("resumes after the hosted flow by polling the gateway, never the return URL", async () => {
    vi.useFakeTimers();
    try {
      const read = vi
        .spyOn(payerClient.verification, "status")
        .mockResolvedValueOnce(
          status({ identity: { status: "pending", attempt_number: 1, retry_available: true } }),
        )
        .mockResolvedValueOnce(
          status({
            requirements: {
              ...REQUIRED,
              document: "approved",
              liveness: "approved",
              identity_match: "approved",
              complete: true,
            },
            identity_start_available: false,
            identity: { status: "approved", attempt_number: 1, retry_available: false },
          }),
        );
      const { onVerified } = renderStep();

      await vi.waitFor(() =>
        expect(screen.getByText(/waiting for the result/i)).toBeInTheDocument(),
      );
      expect(read).toHaveBeenCalledTimes(1);
      await vi.advanceTimersByTimeAsync(4_000);
      await vi.waitFor(() => expect(onVerified).toHaveBeenCalled());
      expect(read).toHaveBeenCalledTimes(2);
    } finally {
      vi.useRealTimers();
    }
  });

  it("offers one retry after a decline, with the appeal contact, and none once review is required", async () => {
    const read = vi.spyOn(payerClient.verification, "status").mockResolvedValue(
      status({
        requirements: { ...REQUIRED, document: "declined" },
        identity: { status: "declined", attempt_number: 1, retry_available: true },
      }),
    );
    vi.spyOn(payerClient.verification, "startIdentity").mockRejectedValue(
      new PaydayError("stopped", "review_required", 409),
    );
    renderStep();

    const retry = await screen.findByRole("button", { name: /try the identity check again/i });
    expect(screen.getByText(/did not confirm that the document matches/)).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "support@payday.sh" })).toHaveAttribute(
      "href",
      "mailto:support@payday.sh",
    );
    expect(read).toHaveBeenCalledTimes(1);

    fireEvent.click(retry);
    await waitFor(() =>
      expect(screen.getByText(/automated identity verification has stopped/i)).toBeInTheDocument(),
    );
    expect(
      screen.queryByRole("button", { name: /try the identity check again/i }),
    ).not.toBeInTheDocument();
    expect(screen.getByRole("link", { name: "support@payday.sh" })).toBeInTheDocument();
  });

  it("says when the partner is still reviewing, and offers nothing to click", async () => {
    vi.spyOn(payerClient.verification, "status").mockResolvedValue(
      status({
        identity_start_available: false,
        identity: { status: "in_review", attempt_number: 1, retry_available: false },
      }),
    );
    renderStep();
    expect(
      await screen.findByText(/being reviewed by the verification partner/i),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("drops a session the gateway no longer accepts", async () => {
    vi.spyOn(payerClient.verification, "status").mockRejectedValue(
      new PaydayError("gone", "payer_session_invalid", 401),
    );
    const { onSession } = renderStep();
    await waitFor(() => expect(onSession).toHaveBeenCalledWith(null));
  });
});
