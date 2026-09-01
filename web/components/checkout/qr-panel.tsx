"use client";

import type { PayerPayment } from "@payday/sdk";
import { useState } from "react";
import { cn } from "@/lib/cn";

/**
 * The QR comes from the API rather than being drawn here: it encodes the same
 * EIP-681 request the wallet button uses, for the amount still due, and it is
 * the gateway that decides when an address must stop being shown — it answers
 * 410 the moment a payment is no longer payable, which arrives here as a load
 * error.
 *
 * A partial payment lowers the remaining amount and therefore changes the code,
 * so the URL carries the remaining base units and re-fetches when they move.
 */
export function QrPanel({ payment, qrUrl }: { payment: PayerPayment; qrUrl: string }) {
  const src = `${qrUrl}?v=${payment.remaining_base_units}`;
  const [state, setState] = useState<"loading" | "ready" | "failed">("loading");
  const [shown, setShown] = useState(src);

  // A partial payment changes the code. Reset while rendering rather than in an
  // effect, so the stale image is never painted for a frame.
  if (src !== shown) {
    setShown(src);
    setState("loading");
  }

  if (state === "failed") return null;

  return (
    <div className="flex flex-col items-center">
      <div className="relative rounded-[12px] border border-line bg-white p-3">
        {state === "loading" ? (
          <div className="absolute inset-3 animate-pulse rounded-md bg-neutral-200" aria-hidden />
        ) : null}
        {/* eslint-disable-next-line @next/next/no-img-element */}
        <img
          key={src}
          src={src}
          alt={`QR code to pay ${payment.remaining} ${payment.token.symbol}`}
          width={196}
          height={196}
          className={cn(
            "size-[196px] transition-opacity duration-200",
            state === "ready" ? "opacity-100" : "opacity-0",
          )}
          onLoad={() => setState("ready")}
          onError={() => setState("failed")}
        />
      </div>
      <p className="mt-3 text-[13px] text-muted">Scan with a wallet on {payment.chain.name}</p>
    </div>
  );
}
