"use client";

import Image from "next/image";
import { useEffect, useState } from "react";
import { PayWith } from "./pay-with";

/**
 * A deposit as a checkout that could be anyone's shows it: the amount, the
 * one-time address it settles to, a clock, and a pay button whose provider
 * keeps changing. Gum is none of the chrome; it is the address.
 */
const EXPIRES_FROM_S = 59 * 60 + 52;

export function CheckoutCard() {
  const [seconds, setSeconds] = useState(EXPIRES_FROM_S);

  useEffect(() => {
    const timer = setInterval(
      () => setSeconds((current) => (current > 0 ? current - 1 : EXPIRES_FROM_S)),
      1_000,
    );
    return () => clearInterval(timer);
  }, []);

  return (
    <div className="flex h-full flex-col justify-between bg-gum-white p-4 text-gum-black">
      <div className="flex items-baseline justify-between gap-3">
        <p className="text-[11px] font-medium tracking-[0.14em] text-gum-grey uppercase">Deposit</p>
        <p className="tabular text-[11px] text-gum-grey">
          Expires in {Math.floor(seconds / 60)}:{String(seconds % 60).padStart(2, "0")}
        </p>
      </div>
      <p className="tabular mt-1 flex items-baseline gap-2 text-[26px] leading-none font-semibold tracking-tight">
        250.00
        <span className="flex items-center gap-1.5 text-[13px] font-medium text-gum-grey">
          <Image
            src="/payment-icons/usdc.svg"
            width={64}
            height={64}
            alt=""
            className="size-4 rounded-full"
          />
          USDC
        </span>
      </p>
      <div className="mt-3 flex items-center justify-between gap-3 text-[12px]">
        <span className="text-gum-grey">One-time address</span>
        <span className="font-mono">0x9a3F…A0c2</span>
      </div>
      <div className="mt-3.5">
        <PayWith />
      </div>
    </div>
  );
}
