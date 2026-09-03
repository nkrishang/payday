"use client";

import { Loader2 } from "lucide-react";
import { useRouter } from "next/navigation";
import { useEffect, useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Field, Input, Problem } from "@/components/ui/field";
import { describeError } from "@/lib/attachment-upload";
import {
  confirmEmailOtp,
  EmailOtpError,
  merchantSession,
  startEmailOtp,
} from "@/lib/merchant-payday";
import { HOME_PATH } from "./session";

/**
 * Email, then the emailed code. The exchange happens against the issuer from
 * the browser; the resulting API token is the session and never leaves memory
 * and this tab's `sessionStorage`.
 */
export function LoginForm() {
  const router = useRouter();
  const [email, setEmail] = useState("");
  const [otp, setOtp] = useState("");
  const [step, setStep] = useState<"email" | "code">("email");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // An open session skips the form.
  useEffect(() => {
    if (merchantSession.get()) router.replace(HOME_PATH);
  }, [router]);

  const sendCode = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await startEmailOtp(email.trim());
      setStep("code");
    } catch (cause) {
      setError(describeError(cause));
    } finally {
      setBusy(false);
    }
  };

  const signIn = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await confirmEmailOtp(email.trim(), otp.trim());
      router.replace(HOME_PATH);
    } catch (cause) {
      setError(
        cause instanceof EmailOtpError && cause.code === "invalid_grant"
          ? "That code is not valid. Check the email, or start over to request a new one."
          : describeError(cause),
      );
      setBusy(false);
    }
  };

  return (
    <div className="w-full max-w-[400px]">
      <div className="rounded-[16px] border border-line bg-surface px-6 py-7">
        <h1 className="text-[19px] font-semibold tracking-tight">Sign in to Payday</h1>
        <p className="mt-1.5 text-[13px] leading-relaxed text-muted">
          {step === "email"
            ? "Enter your merchant email and we will send a one-time code."
            : `We sent a code to ${email.trim()}. It expires in a few minutes.`}
        </p>

        {step === "email" ? (
          <form onSubmit={sendCode} className="mt-6 grid gap-4">
            <Field label="Email">
              <Input
                type="email"
                name="email"
                autoComplete="email"
                required
                value={email}
                onChange={(event) => setEmail(event.target.value)}
              />
            </Field>
            <Problem>{error}</Problem>
            <Button type="submit" disabled={busy}>
              {busy ? <Loader2 className="size-4 animate-spin" /> : null}
              Send code
            </Button>
          </form>
        ) : (
          <form onSubmit={signIn} className="mt-6 grid gap-4">
            <Field label="One-time code">
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
            <Button type="submit" disabled={busy}>
              {busy ? <Loader2 className="size-4 animate-spin" /> : null}
              Sign in
            </Button>
            <button
              type="button"
              disabled={busy}
              onClick={() => {
                setStep("email");
                setOtp("");
                setError(null);
              }}
              className="text-[13px] font-medium text-muted transition-colors hover:text-ink"
            >
              Use a different email
            </button>
          </form>
        )}
      </div>
      <p className="mt-5 text-center text-xs leading-relaxed text-faint">
        The dashboard holds no API key. Your session ends when this tab closes.
      </p>
    </div>
  );
}
