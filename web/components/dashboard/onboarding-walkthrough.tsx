"use client";

import { type Issuer, PaydayError, type DepositRequest } from "@payday/sdk";
import { ArrowLeft, ArrowRight, Loader2, Sparkles } from "lucide-react";
import Image from "next/image";
import { useRef, useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { controlStyles } from "@/components/ui/field";
import { describeError } from "@/lib/attachment-upload";
import { cn } from "@/lib/cn";
import { formatDisplayAmount, truncateAddress } from "@/lib/format";
import { buildCreateDepositRequest, EMPTY_VALUES } from "./create-deposit-request";
import { Labeled } from "./labeled";
import { useMerchant } from "./session";

/**
 * The onboarding walkthrough: a first-class tour of the "New deposit
 * request" flow, not the flow itself. Every field is fixed and inert — the
 * merchant clicks through, they do not fill anything in — and it ends by
 * issuing a real deposit request: Payday billing itself 0.000001 USDC under
 * the identity just created, with a verified-email policy on Payday's own
 * onboarding mailbox. `OnboardingSuccess` takes it from there.
 *
 * There is deliberately no Cancel here. This is the one moment in the
 * product that is not optional: an account with nothing set up has nothing
 * to look at otherwise, and this is what fills that gap instead of a blank
 * composer.
 */

const STEPS = ["Amount", "Billing", "Verification", "Review"] as const;
const AMOUNT = "0.000001";
const BILL_NAME = "Payday";
const BILL_EMAIL = "onboarding@payday.sh";
const HEADING = "Onboarding";

const GUIDANCE = [
  "This is how much you're requesting — Payday will pay this one for real, so you can see the whole flow.",
  "Every deposit request names a payer.",
  "Verification policies control what a payer must prove before they can see and fund a deposit request.",
  "Once you submit this request, you'll create a real on-chain deposit request that's gated by your verification policy.",
] as const;

export function OnboardingWalkthrough({
  issuer,
  payoutAddress,
  onWalletChanged,
  onIssued,
}: {
  issuer: Issuer;
  /**
   * Where the demo request settles: the account's own wallet. Null only
   * while Privy is still creating it, in which case the walkthrough waits.
   */
  payoutAddress: string | null;
  /** Re-reads the account, in case the wallet has arrived. */
  onWalletChanged: () => void;
  onIssued: (payment: DepositRequest) => void;
}) {
  const { client, signOut } = useMerchant();
  const [step, setStep] = useState(0);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const idempotencyKey = useRef(crypto.randomUUID());
  // A customer created for this request, exactly as the real composer does
  // for a freshly-typed payer — so "Payday" shows up in the account's
  // customers table, not just on this one deposit request. Guarded the same way: a
  // retry after a failed create reuses the customer rather than duplicating it.
  const savedCustomer = useRef<string | null>(null);
  const [reference] = useState(() => `ONBOARD-${issuer.id.slice(0, 8).toUpperCase()}`);

  const last = step === STEPS.length - 1;
  const guidance = GUIDANCE[step] ?? GUIDANCE[0];

  const issue = async () => {
    if (!payoutAddress) return;
    setBusy(true);
    setFailure(null);
    try {
      savedCustomer.current ??= (
        await client.customers.create({ name: BILL_NAME, email: BILL_EMAIL })
      ).id;
      const payment = await client.depositRequests.create(
        buildCreateDepositRequest(
          {
            ...EMPTY_VALUES,
            amount: AMOUNT,
            issuerId: issuer.id,
            issuerName: issuer.name,
            issuerEmail: issuer.contact_email,
            payoutAddress,
            expiresInHours: "168",
            customerId: savedCustomer.current,
            billName: BILL_NAME,
            billEmail: BILL_EMAIL,
            heading: HEADING,
            reference,
            mode: "verified_email",
            expectedEmail: BILL_EMAIL,
          },
          null,
        ),
        idempotencyKey.current,
      );
      onIssued(payment);
    } catch (cause) {
      if (cause instanceof PaydayError && cause.status === 401) {
        signOut();
        return;
      }
      setFailure(describeError(cause));
      setBusy(false);
    }
  };

  return (
    <div className="mx-auto max-w-[640px]">
      <h1 className="font-heading text-[26px] leading-tight font-medium tracking-[-0.04em] sm:text-[30px]">
        Welcome to Payday<span className="text-brand-yellow">.</span>
      </h1>
      <p className="mt-2 text-[14px] leading-relaxed text-muted">
        Payday lets you create deposit requests for your customers. You specify the deposit details and
        a verification policy, and Payday handles the rest. Here&apos;s a quick walkthrough.
      </p>

      <ol className="mt-7 flex gap-2.5">
        {STEPS.map((entry, index) => (
          <li key={entry} className="flex-1">
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
              <span className="ml-1.5">{entry}</span>
            </span>
          </li>
        ))}
      </ol>

      <div key={step} className="dash-step mt-8">
        <Highlight guidance={guidance}>
          {step === 0 ? (
            <div className="grid gap-5">
              <Labeled label="Amount">
                <span className="relative block">
                  <input
                    disabled
                    value={AMOUNT}
                    readOnly
                    className={cn(
                      controlStyles,
                      "tabular h-14 pr-[70px] text-[24px] font-medium tracking-tight disabled:opacity-100",
                    )}
                  />
                  <span
                    aria-hidden="true"
                    className="absolute top-1/2 right-4 flex -translate-y-1/2 items-center gap-1.5 text-[13px] text-faint"
                  >
                    <Image
                      src="/payment-icons/usdc.svg"
                      width={64}
                      height={64}
                      alt=""
                      className="size-4 shrink-0 rounded-full"
                    />
                    USDC
                  </span>
                </span>
              </Labeled>
              <div className="grid gap-5 sm:grid-cols-2">
                <Labeled label="Destination">
                  <div className="flex h-11 flex-col justify-center gap-0.5 rounded-[10px] border border-line-strong bg-surface px-3.5">
                    <span className="truncate text-[13px] font-medium leading-tight">
                      {issuer.name}
                    </span>
                    <span className="truncate text-[11.5px] leading-tight text-faint">
                      {issuer.contact_email}
                      <span aria-hidden="true"> · </span>
                      {payoutAddress ? (
                        <span className="font-mono">{truncateAddress(payoutAddress)}</span>
                      ) : (
                        <span>wallet being created…</span>
                      )}
                    </span>
                  </div>
                </Labeled>
                <Labeled label="Expires in">
                  <input
                    disabled
                    readOnly
                    value="7 days"
                    className={cn(controlStyles, "h-11 disabled:opacity-100")}
                  />
                </Labeled>
              </div>
            </div>
          ) : null}

          {step === 1 ? (
            <div className="grid gap-5">
              <div className="grid gap-5 sm:grid-cols-2">
                <Labeled label="Payer">
                  <input
                    disabled
                    readOnly
                    value={BILL_NAME}
                    className={cn(controlStyles, "h-11 disabled:opacity-100")}
                  />
                </Labeled>
                <Labeled label="Email">
                  <input
                    disabled
                    readOnly
                    value={BILL_EMAIL}
                    className={cn(controlStyles, "h-11 disabled:opacity-100")}
                  />
                </Labeled>
              </div>
              <div className="grid gap-5 sm:grid-cols-2">
                <Labeled label="Reason">
                  <input
                    disabled
                    readOnly
                    value={HEADING}
                    className={cn(controlStyles, "h-11 disabled:opacity-100")}
                  />
                </Labeled>
                <Labeled label="Reference">
                  <input
                    disabled
                    readOnly
                    value={reference}
                    className={cn(controlStyles, "h-11 font-mono text-[13px] disabled:opacity-100")}
                  />
                </Labeled>
              </div>
            </div>
          ) : null}

          {step === 2 ? (
            <div className="grid gap-5">
              <label className="flex gap-3 rounded-[12px] border border-brand-green/60 bg-brand-green/[0.07] px-4 py-3.5">
                <input
                  type="radio"
                  checked
                  disabled
                  readOnly
                  aria-label="Verified email"
                  className="mt-1 accent-[#a3d277]"
                />
                <span>
                  <span className="block text-[14px] font-medium">Verified email</span>
                  <span className="mt-0.5 block text-[13px] leading-relaxed text-muted">
                    The payer must prove ownership of the expected email before the amount,
                    details, and address are shown.
                  </span>
                </span>
              </label>
              <Labeled label="Expected payer email">
                <input
                  disabled
                  readOnly
                  value={BILL_EMAIL}
                  className={cn(controlStyles, "h-11 disabled:opacity-100")}
                />
              </Labeled>
            </div>
          ) : null}

          {step === 3 ? (
            <div className="grid gap-4">
              <dl
                aria-label="Request summary"
                className="grid gap-3 rounded-[12px] border border-line bg-surface px-4 py-4 text-[13.5px]"
              >
                <Row label="Amount">
                  <span className="tabular inline-flex items-center justify-end gap-1">
                    {formatDisplayAmount(AMOUNT)}
                    <Image
                      src="/payment-icons/usdc.svg"
                      width={64}
                      height={64}
                      alt=""
                      className="size-3.5 shrink-0 rounded-full"
                    />
                    USDC
                  </span>
                </Row>
                <Row label="Issued by">{issuer.name}</Row>
                <Row label="Settles to">
                  <span className="font-mono text-[12.5px]">
                    {payoutAddress ? truncateAddress(payoutAddress) : "—"}
                  </span>
                  <span className="text-muted"> · Payday wallet</span>
                </Row>
                <Row label="Payer">
                  {BILL_NAME} <span className="text-muted">· {BILL_EMAIL}</span>
                </Row>
                <Row label="Verification">
                  Verified email <span className="text-muted">· {BILL_EMAIL}</span>
                </Row>
                <Row label="Expires in">7 days</Row>
              </dl>
              {failure ? (
                <p role="alert" className="text-[13px] text-danger">
                  {failure}
                </p>
              ) : null}
              {!payoutAddress ? (
                <p className="flex flex-wrap items-center gap-3 text-[13px] text-muted">
                  Your Payday wallet is still being created; it is where this settles.
                  <Button type="button" variant="secondary" size="sm" onClick={onWalletChanged}>
                    Check again
                  </Button>
                </p>
              ) : null}
            </div>
          ) : null}
        </Highlight>
      </div>

      <div className="mt-8 flex items-center gap-3">
        {step > 0 ? (
          <Button type="button" variant="ghost" onClick={() => setStep(step - 1)} disabled={busy}>
            <ArrowLeft className="size-4" />
            Back
          </Button>
        ) : null}
        <div className="flex-1" />
        {last ? (
          <Button type="button" onClick={issue} disabled={busy || !payoutAddress}>
            {busy ? <Loader2 className="size-4 animate-spin" /> : null}
            Issue deposit request
          </Button>
        ) : (
          <Button type="button" onClick={() => setStep(step + 1)}>
            Continue
            <ArrowRight className="size-4" />
          </Button>
        )}
      </div>
    </div>
  );
}

/** Guidance copy over the fields it explains, both inside one highlighted block. */
function Highlight({ guidance, children }: { guidance: string; children: ReactNode }) {
  return (
    <div className="rounded-[16px] border border-brand-green/45 bg-surface p-5">
      <p className="mb-5 flex items-start gap-2.5 text-[13.5px] leading-relaxed text-ink">
        <Sparkles className="mt-0.5 size-4 shrink-0 text-brand-green" />
        {guidance}
      </p>
      {children}
    </div>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex min-w-0 items-baseline justify-between gap-4">
      <dt className="text-faint">{label}</dt>
      <dd className="min-w-0 truncate text-right">{children}</dd>
    </div>
  );
}
