"use client";

import {
  getIdentityToken,
  useCreateWallet,
  useLoginWithEmail,
  usePrivy,
  useUser,
} from "@privy-io/react-auth";
import * as Dialog from "@radix-ui/react-dialog";
import { Loader2, X } from "lucide-react";
import Image from "next/image";
import { useRouter } from "next/navigation";
import { useEffect, useRef, useState, type FormEvent } from "react";
import { HOME_PATH } from "@/components/dashboard/session";
import { cn } from "@/lib/cn";
import {
  createMerchantClient,
  describeLoginError,
  pregenerateWallet,
  RESEND_COOLDOWN_MS,
} from "@/lib/merchant-payday";

const WALLET_POLL_INTERVAL_MS = 900;
const WALLET_POLL_TIMEOUT_MS = 30_000;

/**
 * Confirms gatewayd reports a `wallet_address` for this account.
 *
 * By the time this runs, `signIn` has already explicitly created the wallet
 * and asked Privy to refresh the identity token, so this is normally a single
 * check, not a real wait — it stayed a short poll rather than one bare call
 * because wallet creation is not documented as a guaranteed identity-token
 * refresh trigger (docs/user-management/users/identity-tokens), so a fresh
 * token here can still lag by a beat. gatewayd reads the wallet straight out
 * of whatever identity token is presented (crates/gatewayd/src/api/auth.rs),
 * so once account.get() reports one, Privy's side is genuinely done.
 *
 * Gives up after WALLET_POLL_TIMEOUT_MS rather than hang forever; from there
 * the dashboard's own "Check again" affordance (account-section.tsx,
 * onboarding-walkthrough.tsx) covers whatever is left.
 */
async function waitForWallet(): Promise<void> {
  const deadline = Date.now() + WALLET_POLL_TIMEOUT_MS;
  for (;;) {
    try {
      const token = await getIdentityToken();
      if (token) {
        const account = await createMerchantClient(token).account.get();
        if (account.wallet_address) return;
      }
    } catch {
      // A fresh token or a slow first request — keep polling.
    }
    if (Date.now() >= deadline) return;
    await new Promise((resolve) => setTimeout(resolve, WALLET_POLL_INTERVAL_MS));
  }
}

/**
 * Signing up from the landing page.
 *
 * There is nothing to fill in beyond a mailbox. Privy emails a code, the code
 * signs the merchant in, and the API provisions an account the first time it
 * sees that identity — so the code that signs a returning merchant in is the
 * same code that creates a new one. The dialog is Payday's own; only the code
 * exchange underneath is Privy's, and it ends in the same session the
 * dashboard runs on.
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
  const { authenticated } = usePrivy();
  const [open, setOpen] = useState(false);
  const [wasOpen, setWasOpen] = useState(false);
  const [email, setEmail] = useState("");
  const [otp, setOtp] = useState("");
  const [step, setStep] = useState<"email" | "code" | "wallet">("email");
  const [busy, setBusy] = useState(false);
  const { sendCode, loginWithCode } = useLoginWithEmail();
  const { createWallet } = useCreateWallet();
  const { refreshUser } = useUser();
  const [error, setError] = useState<string | null>(null);
  const [sentAt, setSentAt] = useState(0);
  const [now, setNow] = useState(0);
  const emailField = useRef<HTMLInputElement>(null);
  const otpField = useRef<HTMLInputElement>(null);

  // Another code is offered only once the cooldown has run, so the page holds
  // a clock only while one is counting and stops it the moment it ends.
  const remaining = sentAt === 0 ? 0 : Math.max(0, sentAt + RESEND_COOLDOWN_MS - now);
  const canResend = sentAt !== 0 && remaining === 0;
  useEffect(() => {
    if (sentAt === 0 || remaining === 0) return;
    const tick = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(tick);
  }, [sentAt, remaining]);

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
    if (next && authenticated) {
      router.push(HOME_PATH);
      return;
    }
    // Closing here would strand the account mid-setup with no way back in
    // short of signing in again; wait it out instead.
    if (!next && step === "wallet") return;
    setOpen(next);
  };

  const send = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      // Not awaited: gives Privy the time between now and the merchant
      // typing their code back to finish creating the wallet on its own, so
      // signIn's own createWallet call below usually finds it already there.
      pregenerateWallet(email.trim());
      await sendCode({ email: email.trim() });
      markSent(Date.now());
      setStep("code");
    } catch (cause) {
      setError(describeLoginError(cause));
    } finally {
      setBusy(false);
    }
  };

  const resend = async () => {
    setBusy(true);
    setError(null);
    setOtp("");
    try {
      pregenerateWallet(email.trim());
      await sendCode({ email: email.trim() });
      markSent(Date.now());
      otpField.current?.focus();
    } catch (cause) {
      setError(describeLoginError(cause));
    } finally {
      setBusy(false);
    }
  };

  const signIn = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await loginWithCode({ code: otp.trim() });
      // The code is spent and won't come back, so there is nothing left for
      // this form to do; stay busy and hold here until there's a wallet to
      // settle to. Privy's `createOnLogin` never applies to loginWithCode
      // (see merchant-auth.tsx), so this explicit call is the only thing
      // that ever asks it to create one — and calling it here, immediately
      // after the code is verified, starts that clock as early as possible
      // instead of leaving it to fire later from the dashboard.
      setStep("wallet");
      try {
        await createWallet();
      } catch {
        // A returning merchant already has one — createWallet rejects for
        // an account that's already got an embedded wallet — so there is
        // nothing left to create either way.
      }
      // Wallet creation isn't documented as a guaranteed identity-token
      // refresh trigger, so ask directly rather than hope the reactive
      // token catches up before waitForWallet reads it.
      await refreshUser().catch(() => {});
      await waitForWallet();
      router.push(HOME_PATH);
    } catch (cause) {
      setError(describeLoginError(cause));
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
              disabled={step === "wallet"}
              className="-mt-1.5 -mr-1.5 rounded-[6px] p-1.5 text-brand-grey transition-colors hover:bg-white/[0.06] hover:text-brand-white disabled:pointer-events-none disabled:opacity-30"
            >
              <X className="size-4" />
            </Dialog.Close>
          </div>

          <Dialog.Title className="font-heading mt-5 text-[27px] leading-[1.12] font-medium tracking-[-0.045em]">
            {step === "email" ? "Start building" : step === "code" ? "Check your email" : "Almost there"}
            <span className="text-brand-yellow">.</span>
          </Dialog.Title>
          <Dialog.Description className="mt-2.5 text-[14.5px] leading-[1.6] text-[#b0afa9]">
            {step === "email"
              ? "Enter your email and we'll send a one-time code. No passwords or cards."
              : step === "code"
                ? `We sent a six-digit code to ${email.trim()}.`
                : "Setting up your Payday wallet. This only takes a moment."}
          </Dialog.Description>

          {step === "wallet" ? (
            <div className="mt-8 flex flex-col items-center gap-4 py-4">
              <Loader2 className="size-6 animate-spin text-brand-green" aria-hidden="true" />
              <p role="status" className="text-[13px] text-brand-grey">
                Creating your wallet…
              </p>
            </div>
          ) : step === "email" ? (
            <form onSubmit={send} className="mt-6">
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
                  disabled={busy || !canResend}
                  onClick={resend}
                  className="text-brand-green transition-colors hover:text-brand-green/80 disabled:cursor-default disabled:text-brand-grey disabled:hover:text-brand-grey"
                >
                  {canResend ? "Resend code" : `Resend in ${countdown(remaining)}`}
                </button>
              </div>
            </form>
          )}

          {step === "wallet" ? null : (
            <p className="mt-6 border-t border-brand-grey/20 pt-4 text-[12px] leading-relaxed text-brand-grey">
              Already have an account? The same code signs you in. Payday stores no password.
            </p>
          )}
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
