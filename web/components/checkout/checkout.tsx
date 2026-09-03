"use client";

import type { PayerPayment } from "@payday/sdk";
import { useCallback, useState } from "react";
import { checkoutView, unlockedPayment } from "@/lib/checkout-state";
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
import { WalletPay } from "./wallet-pay";

export function Checkout({ initial }: { initial: PayerPayment }) {
  return (
    <WalletProviders>
      <CheckoutBody initial={initial} />
    </WalletProviders>
  );
}

function CheckoutBody({ initial }: { initial: PayerPayment }) {
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
  // decides the phase and what may enter the tree.
  const unlocked = unlockedPayment(payment);

  return (
    <CheckoutFrame
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

          {view.showInstructions ? (
            <section aria-label={view.title} className="px-5 py-6 sm:px-6">
              <AmountDue payment={unlocked} />
              <ReceivedProgress payment={unlocked} />

              <p className="mt-4 text-[13px] leading-relaxed text-muted">{view.detail}</p>

              <div className="mt-6">
                <WalletPay payment={unlocked} onSent={markSent} />
              </div>

              <div className="my-6 flex items-center gap-3" aria-hidden>
                <span className="h-px flex-1 bg-line" />
                <span className="text-[11px] font-medium tracking-[0.14em] text-faint uppercase">
                  or send it yourself
                </span>
                <span className="h-px flex-1 bg-line" />
              </div>

              <QrPanel payment={unlocked} payerSession={payerSession} />

              <div className="mt-6 space-y-4">
                <AddressRow payment={unlocked} />
                <AssetNotice payment={unlocked} />
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
