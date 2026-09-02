import { PaydayError } from "@payday/sdk";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { payerClient } from "@/lib/payday";
import { EmailVerification } from "./email-verification";

const APPROVED = {
  requirements: {
    email: "approved",
    document: "not_required",
    liveness: "not_required",
    identity_match: "not_required",
    complete: true,
  },
  identity_start_available: false,
} as const;

function renderForm(overrides: Partial<Parameters<typeof EmailVerification>[0]> = {}) {
  const handlers = {
    onSession: vi.fn(),
    onCodeSent: vi.fn(),
    onVerified: vi.fn(),
  };
  const view = render(
    <EmailVerification
      paymentId="pay_1"
      hint="a****@e***.com"
      payerSession={null}
      {...handlers}
      {...overrides}
    />,
  );
  return { ...view, ...handlers };
}

describe("EmailVerification", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("sends the code without naming a mailbox, stores the session, then verifies with it", async () => {
    const start = vi
      .spyOn(payerClient.verification, "startEmail")
      .mockResolvedValue({ payer_session: "pps_new", expires_at: "2026-09-03T00:00:00Z" });
    const confirm = vi.spyOn(payerClient.verification, "confirmEmail").mockResolvedValue(APPROVED);
    const { onSession, onCodeSent, onVerified, rerender, container } = renderForm();

    fireEvent.click(screen.getByRole("button", { name: /send a code to a\*\*\*\*@e\*\*\*\.com/i }));

    await waitFor(() => expect(onSession).toHaveBeenCalledWith("pps_new"));
    expect(start).toHaveBeenCalledWith("pay_1", {});
    expect(onCodeSent).toHaveBeenCalled();
    expect(container.innerHTML).not.toContain("pps_new");

    // The parent stores the session and hands it back down.
    rerender(
      <EmailVerification
        paymentId="pay_1"
        hint="a****@e***.com"
        payerSession="pps_new"
        onSession={onSession}
        onCodeSent={onCodeSent}
        onVerified={onVerified}
      />,
    );
    const code = screen.getByRole("textbox", { name: /one-time code/i });
    expect(screen.getByRole("button", { name: /^verify$/i })).toBeDisabled();
    fireEvent.change(code, { target: { value: " 123456 " } });
    fireEvent.click(screen.getByRole("button", { name: /^verify$/i }));

    await waitFor(() => expect(onVerified).toHaveBeenCalled());
    expect(confirm).toHaveBeenCalledWith("pay_1", "123456", "pps_new");
  });

  it("keeps the form up and says so when the code is wrong", async () => {
    vi.spyOn(payerClient.verification, "startEmail").mockResolvedValue({
      payer_session: "pps",
      expires_at: "2026-09-03T00:00:00Z",
    });
    vi.spyOn(payerClient.verification, "confirmEmail").mockRejectedValue(
      new PaydayError("nope", "otp_invalid", 401),
    );
    const { onVerified, rerender, onSession, onCodeSent } = renderForm();
    fireEvent.click(screen.getByRole("button", { name: /send a code/i }));
    await screen.findByRole("textbox", { name: /one-time code/i });
    rerender(
      <EmailVerification
        paymentId="pay_1"
        hint="a****@e***.com"
        payerSession="pps"
        onSession={onSession}
        onCodeSent={onCodeSent}
        onVerified={onVerified}
      />,
    );

    fireEvent.change(screen.getByRole("textbox", { name: /one-time code/i }), {
      target: { value: "000000" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^verify$/i }));

    expect(await screen.findByRole("alert")).toHaveTextContent(/not accepted/i);
    expect(onVerified).not.toHaveBeenCalled();
    expect(screen.getByRole("textbox", { name: /one-time code/i })).toBeInTheDocument();
  });

  it("resends on the same session and lets a recent code be entered during the cooldown", async () => {
    const start = vi
      .spyOn(payerClient.verification, "startEmail")
      .mockRejectedValue(new PaydayError("wait", "otp_resend_cooldown", 429));
    renderForm({ payerSession: "pps_stored" });

    fireEvent.click(screen.getByRole("button", { name: /send a code/i }));

    expect(await screen.findByRole("alert")).toHaveTextContent(/sent a moment ago/i);
    expect(start).toHaveBeenCalledWith("pay_1", { payerSession: "pps_stored" });
    expect(screen.getByRole("textbox", { name: /one-time code/i })).toBeInTheDocument();
  });

  it("drops an expired session and starts over", async () => {
    vi.spyOn(payerClient.verification, "confirmEmail").mockRejectedValue(
      new PaydayError("gone", "payer_session_invalid", 401),
    );
    vi.spyOn(payerClient.verification, "startEmail").mockRejectedValue(
      new PaydayError("wait", "otp_resend_cooldown", 429),
    );
    const { onSession } = renderForm({ payerSession: "pps_stale" });
    // Reach the code step through the cooldown path, then confirm.
    fireEvent.click(screen.getByRole("button", { name: /send a code/i }));
    fireEvent.change(await screen.findByRole("textbox", { name: /one-time code/i }), {
      target: { value: "123456" },
    });
    fireEvent.click(screen.getByRole("button", { name: /^verify$/i }));

    expect(await screen.findByRole("alert")).toHaveTextContent(/session expired/i);
    expect(onSession).toHaveBeenCalledWith(null);
    expect(screen.getByRole("button", { name: /send a code/i })).toBeInTheDocument();
  });
});
