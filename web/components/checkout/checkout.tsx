"use client";

import type { PayerPayment } from "@payday/sdk";
import { useCallback, useState } from "react";
import { checkoutView, readyPayment, unlockedPayment } from "@/lib/checkout-state";
import { usePayerSession } from "@/lib/payer-session";
import { StatusDot } from "@/components/ui/status-dot";
import { AddressRow } from "./address-row";
import { AmountDue, ReceivedProgress } from "./amount";
import { AssetNotice } from "./asset-notice";
import { Countdown } from "./countdown";
import { CheckoutFrame } from "./frame";
import { InvoiceDetails } from "./invoice-details";
import { WalletProviders } from "./providers";
import { QrPanel } from "./qr-panel";
import { Resolved } from "./resolved";
import { usePayment, useSecondsRemaining } from "./use-payment";
import { VerificationGate } from "./verification-gate";
import { WalletAttestation } from "./wallet-attestation";
import { WalletPay } from "./wallet-pay";

export function Checkout({
  initial,
  embedded,
}: {
  initial: PayerPayment;
  /** See `CheckoutFrame`: renders without page-owning chrome. */
  embedded?: boolean | undefined;
}) {
  return (
    <WalletProviders>
      <CheckoutBody initial={initial} embedded={embedded} />
    </WalletProviders>
  );
}

function CheckoutBody({
  initial,
  embedded,
}: {
  initial: PayerPayment;
  embedded?: boolean | undefined;
}) {
  // This tab's payer session, if it has one. It rides along on every read,
  // so a gated invoice unlocks here after verification and stays unlocked
  // across a reload; it never reaches the server-rendered page.
  const [payerSession, setPayerSession] = usePayerSession(initial.id);
  const { payment, receivedAt, reconnecting, pendingTxHash, markSent, refresh } = usePayment(
    initial,
    payerSession,
  );
  const [emailCodeSent, setEmailCodeSent] = useState(false);
  const updateSession = useCallback(
    (token: string | null) => {
      if (token === null) setEmailCodeSent(false);
      setPayerSession(token);
    },
    [setPayerSession],
  );
  const secondsRemaining = useSecondsRemaining(payment, receivedAt);
  const view = checkoutView(payment, { secondsRemaining, pendingTxHash, emailCodeSent });
  // Null exactly when the phase is one of the locked ones: the same narrowing
  // decides the phase and what may enter the tree. `ready` is null until the
  // payer's wallet is bound and the address exists.
  const unlocked = unlockedPayment(payment);
  const ready = unlocked === null ? null : readyPayment(unlocked);

  return (
    <CheckoutFrame
      embedded={embedded}
      aside={
        reconnecting ? (
          <span className="text-[12px] text-faint" role="status">
            Reconnecting…
          </span>
        ) : null
      }
    >
      <div className="flex items-center justify-between gap-3 border-b border-line px-5 py-3 sm:px-6">
        <span className="flex items-center gap-2 text-[13px] font-medium">
          <StatusDot tone={view.tone} pulse={!view.isTerminal} />
          {view.label}
        </span>
        {view.showInstructions ? <Countdown seconds={secondsRemaining} /> : null}
      </div>

      {unlocked === null ? (
        <VerificationGate
          payment={payment}
          view={view}
          payerSession={payerSession}
          onSession={updateSession}
          onCodeSent={() => setEmailCodeSent(true)}
          onVerified={refresh}
        />
      ) : (
        <>
          <InvoiceDetails payment={unlocked} payerSession={payerSession} />

          {view.showWalletStep ? (
            <section aria-label={view.title} className="px-5 py-6 sm:px-6">
              <AmountDue payment={unlocked} />

              <h2 className="mt-5 text-[15px] font-semibold tracking-tight">{view.title}</h2>
              <p className="mt-2 text-[13px] leading-relaxed text-muted">{view.detail}</p>

              <div className="mt-6">
                <WalletAttestation
                  payment={unlocked}
                  payerSession={payerSession}
                  onSession={updateSession}
                  onBound={refresh}
                />
              </div>
            </section>
          ) : view.showInstructions && ready !== null ? (
            <section aria-label={view.title} className="px-5 py-6 sm:px-6">
              <AmountDue payment={ready} />
              <ReceivedProgress payment={ready} />

              <p className="mt-4 text-[13px] leading-relaxed text-muted">{view.detail}</p>

              <div className="mt-6">
                <WalletPay payment={ready} onSent={markSent} />
              </div>

              <div className="my-6 flex items-center gap-3" aria-hidden>
                <span className="h-px flex-1 bg-line" />
                <span className="text-[11px] font-medium tracking-[0.14em] text-faint uppercase">
                  or send it yourself
                </span>
                <span className="h-px flex-1 bg-line" />
              </div>

              <QrPanel payment={ready} payerSession={payerSession} />

              <div className="mt-6 space-y-4">
                <AddressRow payment={ready} />
                <AssetNotice payment={ready} />
              </div>
            </section>
          ) : (
            <Resolved payment={unlocked} view={view} pendingTxHash={pendingTxHash} />
          )}
        </>
      )}
    </CheckoutFrame>
  );
}
