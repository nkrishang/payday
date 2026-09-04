"use client";

import type { PayerPayment } from "@payday/sdk";
import { ArrowUpRight, Lock } from "lucide-react";
import Image from "next/image";
import { useEffect, useState } from "react";
import { Amount } from "@/components/ui/amount";
import { CopyButton } from "@/components/ui/copy-button";
import { StatusDot } from "@/components/ui/status-dot";
import { config } from "@/lib/config";
import { truncateAddress } from "@/lib/format";
import { payerClient } from "@/lib/payday";
import { statusLabel, statusTone } from "./labels";

/**
 * What the payer sees, beside the merchant's own view of the same request.
 *
 * Read through the public payer route — the same projection the checkout at
 * `payment_url` renders — with no payer session, so a gated request shows here
 * exactly what an unverified visitor would see: the issuer, the heading, and
 * nothing else until they prove what the policy asks. That is the point of
 * showing it. It is a preview, not the checkout: no pay button, because the
 * merchant is not the one paying.
 */
export function PayerPreview({ id, paymentUrl }: { id: string; paymentUrl: string }) {
  const [payer, setPayer] = useState<PayerPayment | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let disposed = false;
    payerClient.payments
      .get(id)
      .then((data) => {
        if (!disposed) setPayer(data);
      })
      .catch(() => {
        if (!disposed) setFailed(true);
      });
    return () => {
      disposed = true;
    };
  }, [id]);

  return (
    <section
      aria-label="Payer's view"
      className="min-w-0 rounded-[16px] border border-line bg-surface p-5"
    >
      <div className="flex items-center justify-between gap-3">
        <h2 className="text-[13px] font-semibold tracking-tight">Payer&apos;s view</h2>
        <a
          href={paymentUrl}
          target="_blank"
          rel="noreferrer noopener"
          className="inline-flex items-center gap-1 text-[12px] text-muted transition-colors hover:text-ink"
        >
          Open
          <ArrowUpRight className="size-3" />
        </a>
      </div>

      <div className="mt-3 flex items-center gap-2 rounded-[10px] border border-line bg-raised py-1.5 pr-1.5 pl-3">
        <span className="min-w-0 flex-1 truncate font-mono text-[12px] text-muted">
          {paymentUrl.replace(/^https?:\/\//, "")}
        </span>
        <CopyButton value={paymentUrl} label="shareable link" className="bg-surface" />
      </div>

      <div className="mt-4 rounded-[12px] border border-line-strong bg-canvas p-4">
        {failed ? (
          <p className="text-[13px] text-muted">The payer page could not be read just now.</p>
        ) : payer === null ? (
          <p role="status" className="text-[13px] text-faint">
            Loading…
          </p>
        ) : (
          <>
            <p className="text-[12px] text-faint">{payer.issuer_name}</p>
            <p className="mt-1 text-[15px] font-medium">{payer.heading ?? "Deposit request"}</p>

            {payer.content_unlocked && payer.amount ? (
              <Amount value={payer.amount} className="mt-3 text-[22px] font-medium" />
            ) : (
              <p className="mt-3 inline-flex items-center gap-2 rounded-[8px] border border-line-strong px-2.5 py-1.5 text-[12.5px] text-muted">
                <Lock className="size-3.5" />
                Amount and address hidden until the payer verifies
              </p>
            )}

            <div className="mt-4 flex flex-wrap items-center gap-x-5 gap-y-2 border-t border-line pt-3 text-[12.5px]">
              <span className="inline-flex items-center gap-2">
                <StatusDot tone={statusTone(payer.status)} />
                {statusLabel(payer.status)}
              </span>
              {payer.address ? (
                <span className="font-mono text-faint">{truncateAddress(payer.address)}</span>
              ) : null}
              {payer.content_unlocked && payer.address ? (
                <Image
                  src={`${config.apiUrl}/v1/payer/payments/${encodeURIComponent(id)}/qr`}
                  width={72}
                  height={72}
                  unoptimized
                  alt="Payment QR code"
                  className="ml-auto size-[72px] rounded-[6px] bg-white p-1"
                />
              ) : null}
            </div>
          </>
        )}
      </div>
    </section>
  );
}
