"use client";

import type { ReadyPayerPayment } from "@/lib/checkout-state";
import { useEffect, useState } from "react";
import { cn } from "@/lib/cn";
import { payerClient } from "@/lib/payday";

/** The outcome of one fetch, tagged with the inputs it was made for. */
type QrOutcome = { key: string; status: "ready"; src: string } | { key: string; status: "failed" };

/**
 * The QR comes from the API rather than being drawn here: it encodes the same
 * EIP-681 request the wallet button uses, for the amount still due, and it is
 * the gateway that decides when an address must stop being shown — it answers
 * 410 the moment a payment is no longer payable, which arrives here as a
 * failed fetch.
 *
 * It is fetched, not linked: a gated invoice's code needs this tab's session,
 * which travels as a header and must never be part of an image URL. The SVG
 * is shown from an object URL that lives only in this page.
 *
 * A partial payment lowers the remaining amount and therefore changes the code,
 * so the fetch re-runs when the remaining base units move.
 */
export function QrPanel({
  payment,
  payerSession = null,
}: {
  payment: ReadyPayerPayment;
  payerSession?: string | null;
}) {
  const { id, remaining_base_units: remaining } = payment;
  // Everything that changes the code, so an outcome for stale inputs reads
  // as "loading" rather than being painted for a frame.
  const key = `${id}\u0000${remaining}\u0000${payerSession ?? ""}`;
  const [outcome, setOutcome] = useState<QrOutcome | null>(null);
  const state = outcome?.key === key ? outcome : { status: "loading" as const };

  useEffect(() => {
    const controller = new AbortController();
    let objectUrl: string | null = null;
    payerClient.payments
      .qr(id, payerSession ?? undefined, { signal: controller.signal })
      .then((svg) => {
        if (controller.signal.aborted) return;
        objectUrl = URL.createObjectURL(svg);
        setOutcome({ key, status: "ready", src: objectUrl });
      })
      .catch(() => {
        if (!controller.signal.aborted) setOutcome({ key, status: "failed" });
      });
    return () => {
      controller.abort();
      if (objectUrl) URL.revokeObjectURL(objectUrl);
    };
  }, [key, id, payerSession]);

  if (state.status === "failed") return null;

  return (
    <div className="flex flex-col items-center">
      <div className="relative rounded-[12px] border border-line bg-white p-3">
        {state.status === "loading" ? (
          <div className="size-[196px] animate-pulse rounded-md bg-neutral-200" aria-hidden />
        ) : (
          // eslint-disable-next-line @next/next/no-img-element
          <img
            src={state.src}
            alt={`QR code to pay ${payment.remaining} ${payment.token.symbol}`}
            width={196}
            height={196}
            className={cn("size-[196px]")}
          />
        )}
      </div>
      <p className="mt-3 text-[13px] text-muted">Scan with a wallet on {payment.chain.name}</p>
    </div>
  );
}
