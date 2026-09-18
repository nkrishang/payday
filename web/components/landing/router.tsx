"use client";

import Image from "next/image";
import { useRef, type CSSProperties } from "react";
import { cn } from "@/lib/cn";
import { CHAINS, LogoGrid } from "./chain-grid";
import { CheckoutCard, CURRENCIES, type Currency } from "./checkout-card";
import { PROVIDERS, useProviderCycle } from "./pay-with";
import { useInView } from "./stage";

/**
 * The route a deposit takes, left to right: from wherever the payer's funds
 * are, through the deposit's own address, to the user's balance in the
 * merchant's app, settled on the chain the merchant chose. Every cycle of
 * the pay button lights the source it names, a chain the funds could be on,
 * and the chain they settle on; the packets on the connectors never stop.
 */

/** Where a deposit can settle. */
const SETTLES_ON = [
  { name: "Monad", src: "/payment-icons/monad.svg" },
  { name: "Base", src: "/logos/base.svg" },
  { name: "Arbitrum", src: "/logos/arbitrum.svg" },
] as const;

/** Whose balance the deposit lands in: a user of the merchant's app. */
const USER = "james@myapp.xyz";

/** Where each currency can settle: USDT has no Base contract Gum will use. */
const SETTLES_FOR: Record<Currency, readonly number[]> = {
  USDC: [0, 1, 2],
  USDT: [0, 2],
  AUSD: [0, 1, 2],
};

/**
 * A different pick each cycle that does not simply walk the list: a
 * small integer hash, so the same cycle always gives the same picture.
 */
function hash(n: number, salt: number): number {
  let x = (n + 1) * 2_654_435_761 + salt * 40_503;
  x ^= x >>> 15;
  x = Math.imul(x, 2_246_822_519);
  x ^= x >>> 13;
  return x >>> 0;
}

export function Router() {
  const frameRef = useRef<HTMLDivElement>(null);
  const inView = useInView(frameRef);
  const { index, leaving } = useProviderCycle();
  const provider = PROVIDERS[index]?.name;
  const currencies = Object.keys(CURRENCIES) as Currency[];
  const currency = currencies[hash(index, 1) % currencies.length]!;
  const from = CHAINS[hash(index, 2) % CHAINS.length]!;
  const settles = SETTLES_FOR[currency];
  const to = SETTLES_ON[settles[hash(index, 3) % settles.length]!]!;

  return (
    <div
      ref={frameRef}
      className={cn(
        "grid items-center gap-8 lg:grid-cols-[minmax(0,1fr)_auto_350px_auto_minmax(0,1fr)] lg:gap-0",
        !inView && "[&_.landing-packet]:[animation-play-state:paused]",
      )}
    >
      <div data-reveal="" className="grid gap-5">
        <Labelled label="Any wallet or exchange">
          <LogoGrid
            logos={PROVIDERS}
            columns={7}
            active={provider}
            label="Wallets, exchanges and on-ramps"
            className="grid-rows-3"
          />
        </Labelled>
        <Labelled label="Any chain">
          <LogoGrid
            logos={CHAINS}
            columns={10}
            active={from.name}
            label="Chains a deposit can arrive from"
            className="grid-rows-1 [&>li]:min-h-[44px]"
          />
        </Labelled>
      </div>

      <Connector />

      <div
        data-reveal="scale"
        style={{ "--i": 1 } as CSSProperties}
        className="overflow-hidden rounded-[14px] border border-gum-grey/30 shadow-[0_24px_60px_-40px_rgb(18_18_18/0.5)]"
      >
        <CheckoutCard index={index} leaving={leaving} currency={currency} />
        <p className="flex items-center gap-2 border-t border-gum-grey/30 bg-gum-white px-4 py-2.5 text-[11.5px] text-gum-grey">
          <span className="size-1.5 rounded-full bg-gum-pink" />
          Paid on {from.name}, settling on {to.name}
        </p>
      </div>

      <Connector />

      <div
        data-reveal=""
        style={{ "--i": 2 } as CSSProperties}
        className="rounded-[14px] border border-gum-grey/30 bg-gum-white p-5"
      >
        <p className="text-[11px] font-medium tracking-[0.14em] text-gum-grey uppercase">
          App balance
        </p>
        <p className="mt-1.5 flex items-center gap-2 text-[15px] font-medium">
          <span
            aria-hidden="true"
            className="flex size-5 items-center justify-center rounded-full bg-gum-black text-[10px] font-semibold text-gum-white"
          >
            J
          </span>
          {USER}
        </p>
        <p className="mt-5 text-[11px] font-medium tracking-[0.14em] text-gum-grey uppercase">
          Settles on
        </p>
        <ul className="mt-2 flex flex-wrap gap-2">
          {SETTLES_ON.map((chain) => (
            <li
              key={chain.name}
              className={cn(
                "flex h-8 items-center gap-1.5 rounded-full border px-2.5 text-[12.5px] font-medium transition-colors duration-300",
                chain.name === to.name
                  ? "border-gum-pink bg-gum-pink text-gum-white"
                  : "border-gum-grey/30 text-gum-grey",
              )}
            >
              <Image src={chain.src} width={32} height={32} alt="" className="size-4" />
              {chain.name}
            </li>
          ))}
        </ul>
        <p
          key={index}
          className="landing-pop tabular mt-5 flex items-center gap-2 text-[22px] leading-none font-semibold tracking-tight"
        >
          +250.00
          <Image
            src={CURRENCIES[currency]}
            width={64}
            height={64}
            alt={currency}
            className="size-5 rounded-full"
          />
        </p>
      </div>
    </div>
  );
}

function Labelled({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div>
      <p className="mb-2 text-[11px] font-medium tracking-[0.14em] text-gum-grey uppercase">
        {label}
      </p>
      {children}
    </div>
  );
}

/** A short line between two stops, with a packet forever crossing it. */
function Connector() {
  return (
    <span aria-hidden="true" className="relative mx-2 hidden h-[2px] w-12 bg-gum-grey/30 lg:block">
      <span
        className="landing-packet absolute top-1/2 left-0 size-2.5 -translate-y-1/2 rounded-full bg-gum-pink"
        style={{ "--travel": "38px" } as CSSProperties}
      />
    </span>
  );
}
