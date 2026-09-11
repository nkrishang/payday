"use client";

import Image from "next/image";
import { useEffect, useState } from "react";
import { cn } from "@/lib/cn";

/**
 * The pay button of a checkout that could be anyone's, cycling through the
 * places a payer's funds already are. One provider shows at a time: it
 * pops in, holds, pops out, and the next pops in. Each rides a white pill,
 * so a black mark reads on the black button as well as a coloured one.
 */
const PROVIDERS = [
  { name: "Coinbase", src: "/logos/coinbase.svg" },
  { name: "MetaMask", src: "/logos/metamask.svg" },
  { name: "Binance", src: "/logos/binance.svg" },
  { name: "MoonPay", src: "/logos/moonpay.svg", wordmark: true },
  { name: "Phantom", src: "/logos/phantom.svg" },
  { name: "Kraken", src: "/logos/kraken.svg" },
  { name: "Rainbow", src: "/logos/rainbow.svg" },
  { name: "OKX", src: "/logos/okx.svg" },
  { name: "Stripe", src: "/logos/stripe.svg" },
  { name: "Ledger", src: "/logos/ledger.svg" },
  { name: "WalletConnect", src: "/logos/walletconnect.svg" },
  { name: "Trust Wallet", src: "/logos/trust.svg" },
  { name: "Robinhood", src: "/logos/robinhood.svg" },
  { name: "KuCoin", src: "/logos/kucoin.svg" },
  { name: "PayPal", src: "/logos/paypal.svg" },
  { name: "Crypto.com", src: "/logos/crypto-com.svg" },
  { name: "Rabby", src: "/logos/rabby.svg" },
  { name: "Gemini", src: "/logos/gemini.svg" },
  { name: "Revolut", src: "/logos/revolut.svg" },
  { name: "Safe", src: "/logos/safe.svg" },
  { name: "fun.xyz", src: "/logos/fun.svg" },
] as const;

/** How long each provider holds before the next, and how long it takes to leave. */
const HOLD_MS = 1_100;
const LEAVE_MS = 140;

export function PayWith() {
  const [index, setIndex] = useState(0);
  const [leaving, setLeaving] = useState(false);

  useEffect(() => {
    const leave = setTimeout(() => setLeaving(true), HOLD_MS);
    const next = setTimeout(() => {
      setLeaving(false);
      setIndex((current) => (current + 1) % PROVIDERS.length);
    }, HOLD_MS + LEAVE_MS);
    return () => {
      clearTimeout(leave);
      clearTimeout(next);
    };
  }, [index]);

  const provider = PROVIDERS[index] ?? PROVIDERS[0];

  return (
    <span
      role="img"
      aria-label={`Pay with ${PROVIDERS.map((entry) => entry.name).join(", ")}`}
      className="flex h-12 items-center justify-center gap-2.5 rounded-[10px] bg-brand-black px-4 text-[14.5px] font-medium text-brand-white"
    >
      Pay with
      <span
        key={index}
        aria-hidden="true"
        className={cn(
          "landing-pop inline-flex h-8 items-center gap-2 rounded-[7px] bg-brand-white px-2.5 text-[13px] font-semibold text-brand-black",
          leaving && "landing-pop-out",
        )}
      >
        <Image
          src={provider.src}
          width={96}
          height={24}
          alt=""
          className={cn("w-auto", "wordmark" in provider ? "h-[15px]" : "h-[18px]")}
        />
        {"wordmark" in provider ? null : provider.name}
      </span>
    </span>
  );
}
