"use client";

import type { PayerPayment } from "@payday/sdk";
import { checkoutView } from "@/lib/checkout-state";
import { StatusDot } from "@/components/ui/status-dot";
import { AddressRow } from "./address-row";
import { AmountDue, ReceivedProgress } from "./amount";
import { AssetNotice } from "./asset-notice";
import { Countdown } from "./countdown";
import { CheckoutFrame } from "./frame";
import { WalletProviders } from "./providers";
import { QrPanel } from "./qr-panel";
import { Resolved } from "./resolved";
import { usePayment, useSecondsRemaining } from "./use-payment";
import { WalletPay } from "./wallet-pay";

export function Checkout({ initial, qrUrl }: { initial: PayerPayment; qrUrl: string }) {
  return (
    <WalletProviders>
      <CheckoutBody initial={initial} qrUrl={qrUrl} />
    </WalletProviders>
  );
}

function CheckoutBody({ initial, qrUrl }: { initial: PayerPayment; qrUrl: string }) {
  const { payment, receivedAt, reconnecting, pendingTxHash, markSent } = usePayment(initial);
  const secondsRemaining = useSecondsRemaining(payment, receivedAt);
  const view = checkoutView(payment, { secondsRemaining, pendingTxHash });

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

      {view.showInstructions ? (
        <section aria-label={view.title} className="px-5 py-6 sm:px-6">
          <AmountDue payment={payment} />
          <ReceivedProgress payment={payment} />

          <p className="mt-4 text-[13px] leading-relaxed text-muted">{view.detail}</p>

          <div className="mt-6">
            <WalletPay payment={payment} onSent={markSent} />
          </div>

          <div className="my-6 flex items-center gap-3" aria-hidden>
            <span className="h-px flex-1 bg-line" />
            <span className="text-[11px] font-medium tracking-[0.14em] text-faint uppercase">
              or send it yourself
            </span>
            <span className="h-px flex-1 bg-line" />
          </div>

          <QrPanel payment={payment} qrUrl={qrUrl} />

          <div className="mt-6 space-y-4">
            <AddressRow payment={payment} />
            <AssetNotice payment={payment} />
          </div>
        </section>
      ) : (
        <Resolved payment={payment} view={view} pendingTxHash={pendingTxHash} />
      )}
    </CheckoutFrame>
  );
}
