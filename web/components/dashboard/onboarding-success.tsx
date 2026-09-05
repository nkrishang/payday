"use client";

import type { PayerPayment, Payment } from "@payday/sdk";
import { Loader2 } from "lucide-react";
import dynamic from "next/dynamic";
import Image from "next/image";
import { useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Problem } from "@/components/ui/field";
import { describeError } from "@/lib/attachment-upload";
import { formatDisplayAmount } from "@/lib/format";
import { payerClient } from "@/lib/payday";
import { storePayerSession } from "@/lib/payer-session";
import { useMerchant } from "./session";

// The real checkout brings wagmi and its wallet-connector stack with it —
// fine for the one payment page that needs them, wrong to bundle into every
// dashboard load for a preview most sessions never reach. Loaded only once
// this screen actually renders, and only in the browser: wagmi reads
// `window` at module-init time.
const Checkout = dynamic(
  () => import("@/components/checkout/checkout").then((module) => module.Checkout),
  { ssr: false },
);

/**
 * The onboarding walkthrough's success screen: not the usual three-CTA
 * `Issued` screen, because there is nothing to track or share yet — this
 * request only exists to demonstrate the product. Instead it shows the real
 * payer's view, live, so the merchant watches their own invoice move from
 * locked to verified to settled before doing anything else.
 *
 * The one real backend call this makes (`onboardingPayment`) both completes
 * verification and submits the real on-chain transfer; from here on, the
 * embedded checkout and the settlement poll below are just watching the real
 * system do its own thing, exactly as a customer's browser would.
 */
export function OnboardingSuccess({
  payment,
  onDone,
}: {
  payment: Payment;
  onDone: () => void;
}) {
  const { client } = useMerchant();
  const [initial, setInitial] = useState<PayerPayment | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [settled, setSettled] = useState(false);
  const started = useRef(false);

  const start = async () => {
    setFailure(null);
    try {
      const { payer_session } = await client.payments.onboardingPayment(payment.id);
      storePayerSession(payment.id, payer_session);
      const unlocked = await payerClient.payments.get(payment.id, { payerSession: payer_session });
      setInitial(unlocked);
    } catch (cause) {
      setFailure(describeError(cause));
    }
  };

  useEffect(() => {
    if (started.current) return;
    started.current = true;
    void start();
    // Runs once, on mount: `payment` is the just-issued request and never changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Independent of the embedded checkout's own polling: this only exists to
  // know when to enable "Get started", using the merchant's own long poll
  // rather than reaching into the payer-facing component's internals.
  useEffect(() => {
    let cancelled = false;
    async function poll() {
      while (!cancelled) {
        try {
          const current = await client.payments.get(payment.id, {
            waitForChange: true,
            timeout: 30,
          });
          if (cancelled) return;
          if (current.status === "settled") {
            setSettled(true);
            return;
          }
        } catch {
          if (cancelled) return;
          await new Promise((resolve) => setTimeout(resolve, 2000));
        }
      }
    }
    void poll();
    return () => {
      cancelled = true;
    };
  }, [client, payment.id]);

  return (
    <div className="dash-hero mx-auto max-w-[520px] pt-6 text-center sm:pt-12">
      <svg viewBox="0 0 64 64" className="mx-auto size-14" fill="none" aria-hidden="true">
        <circle
          className="dash-check-ring"
          cx="32"
          cy="32"
          r="27"
          stroke="#a3d277"
          strokeWidth="2.5"
          strokeLinecap="round"
          transform="rotate(-90 32 32)"
        />
        <path
          className="dash-check-mark"
          d="M21 33.5 28.5 41 43 24"
          stroke="#a3d277"
          strokeWidth="3"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>

      <h1 className="dash-rise dash-delay-2 font-heading mt-6 text-[30px] leading-tight font-medium tracking-[-0.045em]">
        Deposit request issued<span className="text-brand-yellow">.</span>
      </h1>
      <p className="dash-rise dash-delay-3 mx-auto mt-3 max-w-[420px] text-[15px] text-muted">
        <span className="tabular inline-flex items-center gap-1 text-ink">
          {formatDisplayAmount(payment.amount)}
          <Image
            src="/payment-icons/usdc.svg"
            width={64}
            height={64}
            alt=""
            className="size-4 shrink-0 rounded-full"
          />
          {payment.token.symbol}
        </span>{" "}
        from {payment.bill_to.name}
      </p>

      <div className="dash-rise dash-delay-4 mt-8 text-left">
        <p className="mb-3 text-center text-[12px] font-medium tracking-[0.1em] text-faint uppercase">
          This is what your customer sees
        </p>
        <div className="mx-auto flex justify-center">
          {failure ? (
            <div className="w-full max-w-[440px] rounded-[16px] border border-line bg-surface p-6 text-center">
              <Problem>{failure}</Problem>
              <div className="mt-4">
                <Button type="button" variant="secondary" size="sm" onClick={start}>
                  Try again
                </Button>
              </div>
            </div>
          ) : initial ? (
            <Checkout initial={initial} embedded />
          ) : (
            <div
              role="status"
              aria-label="Setting up the live preview"
              className="flex h-[280px] w-full max-w-[440px] items-center justify-center rounded-[16px] border border-line bg-surface"
            >
              <Loader2 className="size-5 animate-spin text-faint" />
            </div>
          )}
        </div>
      </div>

      <div className="dash-rise dash-delay-5 mt-7">
        <Button type="button" onClick={onDone} disabled={!settled} size="lg" className="w-auto px-8">
          {settled ? null : <Loader2 className="size-4 animate-spin" />}
          Get started
        </Button>
      </div>
    </div>
  );
}
