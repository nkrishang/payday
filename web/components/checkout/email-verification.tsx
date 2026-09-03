"use client";

import { PaydayError } from "@payday/sdk";
import { Loader2 } from "lucide-react";
import { useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Field, Input, Problem } from "@/components/ui/field";
import { payerClient } from "@/lib/payday";

/**
 * Proves the payer owns the mailbox the invoice was issued to. The payer
 * never types an address: the gateway sends the code to the merchant's
 * asserted email and only the code comes back here. The session the gateway
 * mints on `start` is handed up to be stored for this tab and sent on every
 * read from then on.
 */
export function EmailVerification({
  paymentId,
  hint,
  payerSession,
  onSession,
  onCodeSent,
  onVerified,
}: {
  paymentId: string;
  hint: string | null;
  payerSession: string | null;
  onSession: (token: string | null) => void;
  onCodeSent: () => void;
  onVerified: () => void;
}) {
  const [step, setStep] = useState<"send" | "code">("send");
  const [otp, setOtp] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const sendCode = async () => {
    setBusy(true);
    setError(null);
    try {
      const started = await payerClient.verification.startEmail(paymentId, {
        ...(payerSession === null ? {} : { payerSession }),
      });
      onSession(started.payer_session);
      setStep("code");
      onCodeSent();
    } catch (cause) {
      if (cause instanceof PaydayError && cause.code === "otp_resend_cooldown") {
        // Only a session that initiated that send can exchange its code.
        if (payerSession !== null) setStep("code");
        setError(
          payerSession === null
            ? "A code was sent a moment ago in another tab. Wait briefly, then request a new one here."
            : "A code was sent a moment ago. Check your inbox before requesting another.",
        );
      } else if (cause instanceof PaydayError && cause.code === "payer_session_invalid") {
        onSession(null);
        setError("This tab's verification session expired. Request a new code.");
      } else {
        setError(describe(cause, "The code could not be sent. Try again in a moment."));
      }
    } finally {
      setBusy(false);
    }
  };

  const confirm = async (event: FormEvent) => {
    event.preventDefault();
    if (payerSession === null) {
      setStep("send");
      setError("Request a code first.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await payerClient.verification.confirmEmail(paymentId, otp.trim(), payerSession);
      setOtp("");
      onVerified();
    } catch (cause) {
      if (cause instanceof PaydayError && cause.code === "otp_invalid") {
        setError("That code was not accepted. Check it, or request a new one.");
      } else if (cause instanceof PaydayError && cause.code === "payer_session_invalid") {
        onSession(null);
        setStep("send");
        setError("This tab's verification session expired. Request a new code.");
      } else if (cause instanceof PaydayError && cause.code === "verification_not_started") {
        setStep("send");
        setError("Request a new code.");
      } else {
        setError(describe(cause, "The code could not be checked. Try again in a moment."));
      }
    } finally {
      setBusy(false);
    }
  };

  if (step === "send") {
    return (
      <div className="grid gap-3">
        <Button type="button" onClick={sendCode} disabled={busy}>
          {busy ? <Loader2 className="size-4 animate-spin" /> : null}
          {hint ? `Send a code to ${hint}` : "Send a verification code"}
        </Button>
        <Problem>{error}</Problem>
      </div>
    );
  }

  return (
    <form onSubmit={confirm} className="grid gap-3">
      <Field label="One-time code" hint="It expires in a few minutes.">
        <Input
          type="text"
          name="otp"
          inputMode="numeric"
          autoComplete="one-time-code"
          required
          autoFocus
          value={otp}
          onChange={(event) => setOtp(event.target.value)}
        />
      </Field>
      <Problem>{error}</Problem>
      <Button type="submit" disabled={busy || otp.trim() === ""}>
        {busy ? <Loader2 className="size-4 animate-spin" /> : null}
        Verify
      </Button>
      <button
        type="button"
        disabled={busy}
        onClick={sendCode}
        className="text-[13px] font-medium text-muted transition-colors hover:text-ink disabled:opacity-50"
      >
        Resend code
      </button>
    </form>
  );
}

function describe(cause: unknown, fallback: string): string {
  return cause instanceof PaydayError && cause.code === "identity_provider_unavailable"
    ? "The verification service did not respond. Try again shortly."
    : fallback;
}
