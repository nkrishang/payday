"use client";

import { type Issuer, PaydayError } from "@payday/sdk";
import { ArrowRight, Check, Loader2 } from "lucide-react";
import { useEffect, useId, useRef, useState, type FormEvent, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { controlStyles } from "@/components/ui/field";
import { describeError } from "@/lib/attachment-upload";
import { cn } from "@/lib/cn";
import { duplicateName, EMAIL, LABEL, PAYOUT_ADDRESS } from "./field-rules";
import { Labeled } from "./labeled";
import { useMerchant } from "./session";

/**
 * Setting up an issuer identity.
 *
 * The whole form is on the page at once. Only the section being answered is
 * enabled; the ones after it are visible but inert, and the ones before it keep
 * their answers on screen with a control to go back and change them. Nothing is
 * hidden, so nothing has to be remembered, and the step that looks
 * irreversible — the emailed code — is not a door that closes behind you.
 *
 * The form resumes from whatever the account already holds, so an abandoned tab
 * reopens at the section that is unfinished rather than at the start.
 */

type Step = 0 | 1 | 2;

const SECTIONS = ["Identity", "Verify email", "Wallet"] as const;

/** Where an account is, given what it already holds. */
export function resumeAt(issuers: Issuer[]): { step: Step; issuer: Issuer | null } {
  const unverified = issuers.find((issuer) => !issuer.email_verified);
  if (unverified) return { step: 1, issuer: unverified };
  const unpaid = issuers.find((issuer) => issuer.payout_addresses.length === 0);
  if (unpaid) return { step: 2, issuer: unpaid };
  return { step: 0, issuer: null };
}

function countdown(ms: number): string {
  const seconds = Math.ceil(ms / 1000);
  return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
}

export function IssuerSetup({
  issuers,
  onDone,
  onCancel,
}: {
  /** What the account already holds, so the form can pick up where it left off. */
  issuers: Issuer[];
  onDone: (issuer: Issuer) => void;
  onCancel: (() => void) | null;
}) {
  const { client, signOut } = useMerchant();
  // Read once, on mount: reloading the list mid-flow must not throw the
  // merchant back to a section they have already answered.
  const [resumed] = useState(() => resumeAt(issuers));

  const [step, setStep] = useState<Step>(resumed.step);
  const [issuer, setIssuer] = useState<Issuer | null>(resumed.issuer);
  const [name, setName] = useState(resumed.issuer?.name ?? "");
  const [contactEmail, setContactEmail] = useState(resumed.issuer?.contact_email ?? "");
  const [otp, setOtp] = useState("");
  const [address, setAddress] = useState("");
  const [label, setLabel] = useState("");
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // The API allows another code a minute after the last one; the page counts
  // the same window down rather than letting a merchant hit a 429.
  const [resendAt, setResendAt] = useState(0);
  const [now, setNow] = useState(0);

  // Completing a section moves the work down the page; the page follows it, so
  // the merchant is not left looking at the section they just finished. Not on
  // the first render: resuming should land where it landed, without a lurch.
  const formId = useId();
  const settled = useRef(false);
  useEffect(() => {
    if (!settled.current) {
      settled.current = true;
      return;
    }
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    document.getElementById(`${formId}-${step}`)?.scrollIntoView({
      behavior: reduced ? "auto" : "smooth",
      block: "center",
    });
  }, [formId, step]);

  const waiting = Math.max(0, resendAt - now);
  useEffect(() => {
    if (resendAt === 0) return;
    const tick = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(tick);
  }, [resendAt]);

  // Checked as it is typed, not on submit: nothing advances until it is right,
  // and the reason is on screen before the button is reached for.
  const nameError = !name.trim()
    ? "Required."
    : duplicateName(name, issuers, issuer?.id)
      ? "Another identity already uses this name."
      : null;
  const emailError = !contactEmail.trim()
    ? "Required."
    : EMAIL.test(contactEmail.trim())
      ? null
      : "Not a valid email address.";
  const addressError = !address.trim()
    ? "Required."
    : PAYOUT_ADDRESS.test(address.trim())
      ? null
      : "Not a valid address: 0x and 40 hex characters.";
  const labelError = !label.trim() || LABEL.test(label.trim()) ? null : "Not a valid label.";

  const failed = (cause: unknown) => {
    if (cause instanceof PaydayError && cause.status === 401 && cause.code === "unauthorized") {
      signOut();
      return;
    }
    setFailure(describeError(cause));
    setBusy(false);
  };

  const armResend = (at: string | undefined) => {
    const parsed = at ? Date.parse(at) : Number.NaN;
    setResendAt(Number.isNaN(parsed) ? Date.now() + 60_000 : parsed);
    setNow(Date.now());
  };

  /** Saves the identity (or moves an existing one) and sends a code. */
  const submitIdentity = async () => {
    if (nameError || emailError) return;
    setBusy(true);
    setFailure(null);
    try {
      const body = { name: name.trim(), contact_email: contactEmail.trim() };
      const saved = issuer
        ? await client.issuers.update(issuer.id, body)
        : await client.issuers.create(body);
      setIssuer(saved);
      if (saved.email_verified) {
        // The address did not move, so the proof stands and there is nothing
        // to send: going back to fix a typo in the name costs no code.
        setStep(2);
        setBusy(false);
        return;
      }
      const started = await client.issuers.startEmailVerification(saved.id);
      armResend(started.resend_available_at);
      setOtp("");
      setStep(1);
      setBusy(false);
    } catch (cause) {
      failed(cause);
    }
  };

  const resend = async () => {
    if (!issuer) return;
    setBusy(true);
    setFailure(null);
    setOtp("");
    try {
      const started = await client.issuers.startEmailVerification(issuer.id);
      armResend(started.resend_available_at);
      setBusy(false);
    } catch (cause) {
      failed(cause);
    }
  };

  const submitOtp = async () => {
    if (!issuer || otp.length < 6) return;
    setBusy(true);
    setFailure(null);
    try {
      const verified = await client.issuers.confirmEmailVerification(issuer.id, otp.trim());
      setIssuer(verified);
      setStep(2);
      setBusy(false);
    } catch (cause) {
      if (cause instanceof PaydayError && cause.code === "otp_invalid") {
        setFailure("That code is not valid. Check the email, or send a new one.");
        setBusy(false);
        return;
      }
      failed(cause);
    }
  };

  const submitAddress = async () => {
    if (!issuer || addressError || labelError) return;
    setBusy(true);
    setFailure(null);
    try {
      const saved = await client.payoutAddresses.create({
        address: address.trim(),
        ...(label.trim() ? { label: label.trim() } : {}),
      });
      const attached = await client.issuers.setPayoutAddresses(issuer.id, [
        ...issuer.payout_addresses.map((entry) => entry.id),
        saved.id,
      ]);
      onDone(attached);
    } catch (cause) {
      failed(cause);
    }
  };

  // The form has one action at a time, in a footer that belongs to the form
  // rather than to any card — and that sticks to the bottom of the viewport, so
  // reaching it never means scrolling past the section being answered. It names
  // the section it is acting on, so its scope is stated rather than implied.
  const actionLabel = (["Send code", "Confirm code", "Save identity"] as const)[step];
  const ready = [
    nameError === null && emailError === null,
    otp.length === 6,
    addressError === null && labelError === null,
  ][step];

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (step === 0) void submitIdentity();
    else if (step === 1) void submitOtp();
    else void submitAddress();
  };

  return (
    <form onSubmit={submit} className="mx-auto max-w-[640px]">
      <ol className="flex gap-2.5">
        {SECTIONS.map((section, index) => (
          <li key={section} className="flex-1">
            <span className="block h-[3px] overflow-hidden rounded-full bg-line">
              <span
                className={cn(
                  "block h-full origin-left rounded-full bg-brand-green transition-transform duration-500 ease-out",
                  index <= step ? "scale-x-100" : "scale-x-0",
                )}
              />
            </span>
            <span
              className={cn(
                "mt-2 block text-[12px] font-medium transition-colors",
                index === step ? "text-ink" : "text-faint",
              )}
            >
              <span className="tabular">{index + 1}</span>
              <span className="ml-1.5">{section}</span>
            </span>
          </li>
        ))}
      </ol>

      <div className="mt-8 grid gap-3">
        <Section
          id={`${formId}-0`}
          index={0}
          title="Identity"
          step={step}
          onEdit={() => {
            setFailure(null);
            setStep(0);
          }}
        >
          <div className="grid gap-5">
            <Labeled
              label="Issued by"
              error={step === 0 && name ? (nameError ?? undefined) : undefined}
              hint="Appears on every deposit request issued under this identity."
            >
              <input
                autoFocus={step === 0}
                maxLength={255}
                placeholder="Acme Inc."
                disabled={step !== 0 || busy}
                value={name}
                onChange={(event) => setName(event.target.value)}
                className={cn(controlStyles, "h-11", step > 0 && "disabled:opacity-100")}
              />
            </Labeled>
            <Labeled
              label="Contact address"
              error={step === 0 && contactEmail ? (emailError ?? undefined) : undefined}
              hint="Shown on the request as your contact. Confirmed by emailed code."
            >
              <input
                type="email"
                placeholder="billing@acme.com"
                disabled={step !== 0 || busy}
                value={contactEmail}
                onChange={(event) => setContactEmail(event.target.value)}
                className={cn(controlStyles, "h-11", step > 0 && "disabled:opacity-100")}
              />
            </Labeled>
          </div>
        </Section>

        <Section id={`${formId}-1`} index={1} title="Verify email" step={step}>
          <div className="grid gap-5">
            <Labeled
              label="One-time code"
              hint={
                issuer && step >= 1
                  ? `Sent to ${issuer.contact_email}.`
                  : "Sent once the identity above is saved."
              }
            >
              <input
                inputMode="numeric"
                autoComplete="one-time-code"
                maxLength={6}
                autoFocus={step === 1}
                placeholder="000000"
                disabled={step !== 1 || busy}
                value={step > 1 ? "••••••" : otp}
                onChange={(event) => setOtp(event.target.value.replace(/\D/g, "").slice(0, 6))}
                className={cn(
                  controlStyles,
                  "h-12 max-w-[240px] text-center font-mono text-[18px] tracking-[0.4em] [text-indent:0.4em]",
                  step > 1 && "disabled:opacity-100",
                )}
              />
            </Labeled>
            {step === 1 ? (
              <div>
                <button
                  type="button"
                  disabled={busy || waiting > 0}
                  onClick={resend}
                  className="text-[13px] font-medium text-brand-green transition-colors hover:text-brand-green/80 disabled:cursor-default disabled:text-faint"
                >
                  {waiting > 0 ? `Resend in ${countdown(waiting)}` : "Resend code"}
                </button>
              </div>
            ) : null}
          </div>
        </Section>

        <Section id={`${formId}-2`} index={2} title="Wallet" step={step}>
          <div className="grid gap-5">
            <Labeled
              label="Payout address"
              error={step === 2 && address ? (addressError ?? undefined) : undefined}
              hint="Deposits settle here once they finalize. More can be added later."
            >
              <input
                autoFocus={step === 2}
                spellCheck={false}
                placeholder="0x…"
                disabled={step !== 2 || busy}
                value={address}
                onChange={(event) => setAddress(event.target.value)}
                className={cn(controlStyles, "h-11 font-mono text-[13px]")}
              />
            </Labeled>
            <Labeled
              label="Label"
              error={step === 2 ? (labelError ?? undefined) : undefined}
              hint="Optional. Up to 20 letters, digits, spaces, and . _ ' &amp; ( ) -"
            >
              <input
                maxLength={20}
                placeholder="Treasury"
                disabled={step !== 2 || busy}
                value={label}
                onChange={(event) => setLabel(event.target.value)}
                className={cn(controlStyles, "h-11")}
              />
            </Labeled>
          </div>
        </Section>
      </div>

      <div className="dash-formbar sticky bottom-0 z-10 mt-3 flex flex-wrap items-center justify-between gap-3 border-t border-line py-4">
        {failure ? (
          <p role="alert" className="min-w-0 text-[13px] text-danger">
            {failure}
          </p>
        ) : (
          <p className="text-[12px] text-faint">
            <span className="tabular">
              Step {step + 1} of {SECTIONS.length}
            </span>
            <span className="mx-1.5">·</span>
            <span className="text-muted">{SECTIONS[step]}</span>
          </p>
        )}
        <div className="flex items-center gap-3">
          {onCancel ? (
            <Button type="button" variant="ghost" onClick={onCancel} disabled={busy}>
              Cancel
            </Button>
          ) : null}
          <Button type="submit" disabled={busy || !ready}>
            {busy ? <Loader2 className="size-4 animate-spin" /> : null}
            {actionLabel}
            <ArrowRight className="size-4" />
          </Button>
        </div>
      </div>
    </form>
  );
}

/**
 * One block of the form. Answered blocks stay on screen with what was entered
 * and, where they can be revisited, a control that makes them current again.
 */
function Section({
  id,
  index,
  title,
  step,
  onEdit,
  children,
}: {
  id: string;
  index: number;
  title: string;
  step: number;
  onEdit?: () => void;
  children: ReactNode;
}) {
  const state = index === step ? "current" : index < step ? "done" : "upcoming";
  return (
    <section
      id={id}
      aria-label={title}
      aria-current={state === "current" ? "step" : undefined}
      className={cn(
        "rounded-[16px] border p-5 transition-colors",
        state === "current" ? "border-brand-green/45 bg-surface" : "border-line bg-surface/60",
        state === "upcoming" ? "opacity-45" : "",
      )}
    >
      <div className="mb-4 flex items-center justify-between gap-3">
        <p className="flex items-center gap-2 text-[12px] font-medium tracking-[0.08em] uppercase">
          <span className={cn("tabular", state === "current" ? "text-brand-green" : "text-faint")}>
            {index + 1}
          </span>
          <span className={state === "current" ? "text-ink" : "text-faint"}>{title}</span>
          {state === "done" ? <Check className="size-3.5 text-brand-green" /> : null}
        </p>
        {state === "done" && onEdit ? (
          <button
            type="button"
            onClick={onEdit}
            className="rounded-md px-2 py-1 text-[12px] font-medium text-muted transition-colors hover:bg-raised hover:text-ink"
          >
            Change
          </button>
        ) : null}
      </div>
      {children}
    </section>
  );
}
