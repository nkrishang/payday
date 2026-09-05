"use client";

import * as Dialog from "@radix-ui/react-dialog";
import { Loader2, X } from "lucide-react";
import Image from "next/image";
import { useRouter } from "next/navigation";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { HOME_PATH } from "@/components/dashboard/session";
import { cn } from "@/lib/cn";
import { describeError } from "@/lib/attachment-upload";
import {
  confirmEmailOtp,
  EMAIL_OTP_LIFETIME_MS,
  EmailOtpError,
  merchantSession,
  startEmailOtp,
} from "@/lib/merchant-payday";

/**
 * Signing up from the landing page.
 *
 * There is nothing to fill in beyond a mailbox. The API provisions an account
 * the first time it sees a verified identity, so the emailed code that signs a
 * returning merchant in is the same code that creates a new one — this is the
 * dashboard's own exchange in the landing page's clothes, and it ends in the
 * same session: a short-lived token in memory and this tab's `sessionStorage`,
 * never an API key.
 *
 * Like the landing page around it, the dialog is dark in both colour schemes,
 * so it names its colours rather than reading the theme tokens. Its ground is
 * the page's own #070707 because the wordmark bakes that colour in, both as a
 * backing rect and as the knockouts that cut the mark's two arches.
 */

const fieldStyles =
  "h-12 w-full rounded-[8px] border border-brand-grey/30 bg-white/[0.04] px-3.5 text-[15px] " +
  "text-brand-white transition-colors placeholder:text-brand-grey/60 focus:border-brand-green " +
  "disabled:opacity-50";

const labelStyles = "block text-[12px] font-medium text-brand-grey";

const submitStyles =
  "mt-4 flex h-13 w-full items-center justify-center gap-2.5 rounded-[8px] bg-brand-green " +
  "px-5 text-[15px] font-medium text-brand-black transition-colors hover:bg-brand-green/90 " +
  // Dimmed green reads as olive against this ground, so waiting is grey.
  "disabled:pointer-events-none disabled:bg-white/[0.07] disabled:text-brand-grey";

/** m:ss, for a countdown that never shows a bare number of seconds. */
function countdown(ms: number): string {
  const seconds = Math.ceil(ms / 1000);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

export function StartBuilding() {
  const router = useRouter();
  const [open, setOpen] = useState(false);
  const [wasOpen, setWasOpen] = useState(false);
  const [email, setEmail] = useState("");
  const [otp, setOtp] = useState("");
  const [step, setStep] = useState<"email" | "code">("email");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [sentAt, setSentAt] = useState(0);
  const [now, setNow] = useState(0);
  const emailField = useRef<HTMLInputElement>(null);
  const otpField = useRef<HTMLInputElement>(null);

  // The code outlives nothing but its window, so the page holds a clock only
  // while one is outstanding and stops it the moment the window closes.
  const remaining = sentAt === 0 ? 0 : Math.max(0, sentAt + EMAIL_OTP_LIFETIME_MS - now);
  const expired = sentAt !== 0 && remaining === 0;
  useEffect(() => {
    if (sentAt === 0) return;
    const tick = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(tick);
  }, [sentAt]);

  // Both stamps move together, so the first frame of a countdown is a whole
  // window rather than a leftover from the last one.
  const markSent = (at: number) => {
    setSentAt(at);
    setNow(at);
  };

  // Reopening starts clean; a code half-typed before a stray Escape is not
  // something to come back to.
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) {
      setStep("email");
      setOtp("");
      setError(null);
      setBusy(false);
      markSent(0);
    }
  }

  // Someone still holding a session has nothing to sign up for.
  const openChange = (next: boolean) => {
    if (next && merchantSession.get()) {
      router.push(HOME_PATH);
      return;
    }
    setOpen(next);
  };

  const sendCode = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await startEmailOtp(email.trim());
      markSent(Date.now());
      setStep("code");
    } catch (cause) {
      setError(describeError(cause));
    } finally {
      setBusy(false);
    }
  };

  // Offered only once the code on its way has expired, so a merchant is never
  // holding two live codes and wondering which one the page wants.
  const resend = async () => {
    setBusy(true);
    setError(null);
    setOtp("");
    try {
      await startEmailOtp(email.trim());
      markSent(Date.now());
      otpField.current?.focus();
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
      // Stays busy through the navigation: the dialog is done, and a second
      // submit would spend a code that no longer exists.
      router.push(HOME_PATH);
    } catch (cause) {
      setError(
        cause instanceof EmailOtpError && cause.code === "invalid_grant"
          ? "That code is not valid. Check the email and try again."
          : describeError(cause),
      );
      setBusy(false);
      // Selected, not cleared: the next attempt is usually a correction.
      otpField.current?.select();
    }
  };

  return (
    <Dialog.Root open={open} onOpenChange={openChange}>
      <Dialog.Trigger className="flex h-14 items-center justify-center gap-3 rounded-[8px] bg-brand-green px-5 text-[16px] font-medium text-brand-black transition-transform hover:-translate-y-0.5">
        Start Building
        <ArrowRight />
      </Dialog.Trigger>

      <Dialog.Portal>
        <Dialog.Overlay className="landing-dialog-scrim fixed inset-0 z-50 bg-black/75 backdrop-blur-[3px]" />
        <Dialog.Content
          // Radix would open on the close button; the mailbox is the point.
          onOpenAutoFocus={(event) => {
            event.preventDefault();
            emailField.current?.focus();
          }}
          className="landing-dialog fixed top-1/2 left-1/2 z-50 w-[calc(100vw-28px)] max-w-[432px] -translate-x-1/2 -translate-y-1/2 rounded-[14px] border border-brand-grey/25 bg-[#070707] px-6 pt-6 pb-7 text-brand-white sm:px-7"
        >
          <div className="flex items-start justify-between gap-4">
            <Image
              src="/payday-logo-full.svg"
              width={3200}
              height={1000}
              alt="Payday"
              className="mt-0.5 h-auto w-[84px]"
            />
            <Dialog.Close
              aria-label="Close"
              className="-mt-1.5 -mr-1.5 rounded-[6px] p-1.5 text-brand-grey transition-colors hover:bg-white/[0.06] hover:text-brand-white"
            >
              <X className="size-4" />
            </Dialog.Close>
          </div>

          <Dialog.Title className="font-heading mt-5 text-[27px] leading-[1.12] font-medium tracking-[-0.045em]">
            {step === "email" ? "Start building" : "Check your email"}
            <span className="text-brand-yellow">.</span>
          </Dialog.Title>
          <Dialog.Description className="mt-2.5 text-[14.5px] leading-[1.6] text-[#b0afa9]">
            {step === "email"
              ? "Enter your email and we'll send a one-time code. No passwords or cards."
              : expired
                ? `The code we sent to ${email.trim()} has expired. Send yourself a new one.`
                : `We sent a six-digit code to ${email.trim()}.`}
          </Dialog.Description>

          {step === "email" ? (
            <form onSubmit={sendCode} className="mt-6">
              <label className="block">
                <span className={labelStyles}>Email</span>
                <input
                  ref={emailField}
                  type="email"
                  name="email"
                  autoComplete="email"
                  placeholder="you@company.com"
                  required
                  value={email}
                  onChange={(event) => setEmail(event.target.value)}
                  className={cn(fieldStyles, "mt-2")}
                />
              </label>
              <Problem>{error}</Problem>
              <button type="submit" disabled={busy} className={submitStyles}>
                {busy ? <Loader2 className="size-4 animate-spin" /> : null}
                Send code
              </button>
            </form>
          ) : (
            <form onSubmit={signIn} className="mt-6">
              <label className="block">
                <span className={labelStyles}>One-time code</span>
                <input
                  ref={otpField}
                  type="text"
                  name="otp"
                  inputMode="numeric"
                  autoComplete="one-time-code"
                  maxLength={6}
                  placeholder="000000"
                  required
                  autoFocus
                  value={otp}
                  // Digits only, so a pasted "123 456" still lands.
                  onChange={(event) => setOtp(event.target.value.replace(/\D/g, "").slice(0, 6))}
                  className={cn(
                    fieldStyles,
                    "mt-2 text-center font-mono text-[21px] tracking-[0.42em] [text-indent:0.42em]",
                  )}
                />
              </label>
              <Problem>{error}</Problem>
              <button type="submit" disabled={busy || otp.length < 6} className={submitStyles}>
                {busy ? <Loader2 className="size-4 animate-spin" /> : null}
                Continue
              </button>
              <div className="mt-3 flex items-center justify-between gap-3 text-[13px] font-medium">
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => {
                    setStep("email");
                    setOtp("");
                    setError(null);
                    markSent(0);
                  }}
                  className="text-brand-grey transition-colors hover:text-brand-white disabled:opacity-50"
                >
                  Use a different email
                </button>
                <button
                  type="button"
                  disabled={busy || !expired}
                  onClick={resend}
                  className="text-brand-green transition-colors hover:text-brand-green/80 disabled:cursor-default disabled:text-brand-grey disabled:hover:text-brand-grey"
                >
                  {expired ? "Resend code" : `Resend in ${countdown(remaining)}`}
                </button>
              </div>
            </form>
          )}

          <p className="mt-6 border-t border-brand-grey/20 pt-4 text-[12px] leading-relaxed text-brand-grey">
            Already have an account? The same code signs you in. Payday stores no password, and your
            session ends when this tab closes.
          </p>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

/** Inline problem text, announced, in the dark palette's `--danger` — named
 *  rather than read, because this panel is dark under either colour scheme. */
function Problem({ children }: { children: React.ReactNode }) {
  if (!children) return null;
  return (
    <p role="alert" className="mt-3 text-[13px] leading-relaxed text-[#f87171]">
      {children}
    </p>
  );
}

function ArrowRight() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" className="size-5" fill="none">
      <path d="M5 12h14M14 7l5 5-5 5" stroke="currentColor" strokeWidth="1.8" />
    </svg>
  );
}
