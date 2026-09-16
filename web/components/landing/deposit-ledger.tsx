"use client";

import { useRef, type CSSProperties } from "react";
import { cn } from "@/lib/cn";
import { Stage, useInView, useLoopClock, useReducedMotion } from "./stage";

/**
 * A merchant's ledger of deposits, one row per deposit request. Every few
 * seconds a new one slides in at the top with its own address, waits for
 * its payer, and settles; the rows below move down to make room. Each row is a pure function
 * of the clock: which deposits are showing, how far down each sits, and
 * whether it has settled all follow from `t`, so the picture is the same
 * whether it has been running for a second or an hour.
 */

const STAGE = { width: 600, height: 340 } as const;

/** How often a deposit arrives, and how long it waits before settling. */
const PERIOD_MS = 3_200;
const SETTLE_MS = 1_800;

const ROW_HEIGHT = 62;
/** Rows showing at once; one more waits above the fold and one slides out below it. */
const ROWS = 4;

const DEPOSITS = [
  { payer: "Jordan Diaz", ref: "user_8f21", address: "0x9a3F…A0c2", amount: "250.00" },
  { payer: "Maya Patel", ref: "user_2c9e", address: "0x41c9…7E1b", amount: "1,200.00" },
  { payer: "Acme Corp", ref: "org_a1b2", address: "0xB70d…33fA", amount: "18,000.00" },
  { payer: "Lena Fischer", ref: "user_d0a4", address: "0x6E5c…91C8", amount: "75.00" },
  { payer: "Tomás Rivera", ref: "user_77be", address: "0xF2a8…0b4D", amount: "420.00" },
  { payer: "Globex LLC", ref: "org_5e3f", address: "0x0C4e…bD12", amount: "5,000.00" },
  { payer: "Priya Nair", ref: "user_b18c", address: "0x8d3A…4E7f", amount: "60.00" },
  { payer: "Sam Okafor", ref: "user_e4f0", address: "0x2B9f…C6a1", amount: "980.00" },
] as const;

const TOTAL_MS = PERIOD_MS * DEPOSITS.length;

function initials(name: string): string {
  return name
    .split(" ")
    .map((word) => word[0] ?? "")
    .join("")
    .slice(0, 2)
    .toUpperCase();
}

export function DepositLedger() {
  const reduced = useReducedMotion();
  const frameRef = useRef<HTMLDivElement>(null);
  const inView = useInView(frameRef);
  // Without motion, hold a moment with one deposit still waiting.
  const t = useLoopClock(TOTAL_MS, reduced ? PERIOD_MS * 3 + 600 : null, inView);

  // The newest deposit's number; the rows are it, the ones before it, and
  // the one after it, waiting above the fold so its arrival is a slide down
  // like everyone else's rather than an appearance. Six rows show at once
  // and the pool holds eight, so a number's place in the pool is a key that
  // stays unique and, at the wrap, lets the clock's reset pass as one more
  // period.
  const newest = Math.floor(t / PERIOD_MS);
  const rows = Array.from({ length: ROWS + 2 }, (_, slot) => {
    const position = slot - 1;
    const number = newest - position;
    const key = ((number % DEPOSITS.length) + DEPOSITS.length) % DEPOSITS.length;
    const age = t - number * PERIOD_MS;
    return { key, position, deposit: DEPOSITS[key]!, settled: age >= SETTLE_MS };
  });

  return (
    <div ref={frameRef}>
      <Stage
        width={STAGE.width}
        height={STAGE.height}
        label="A merchant's ledger of deposits: each request has its own one-time address, and settles on its own."
        className="rounded-[18px] border border-gum-grey/30 bg-gum-white text-gum-black"
      >
        <div className="flex items-center justify-between border-b border-gum-grey/30 px-6 py-4">
          <p className="text-[15px] font-semibold tracking-tight">Deposits</p>
          <p className="flex items-center gap-2 text-[12px] font-medium text-gum-grey">
            <span className="relative flex size-2">
              <span className="absolute inset-0 animate-ping rounded-full bg-gum-pink/60" />
              <span className="relative size-2 rounded-full bg-gum-pink" />
            </span>
            Live
          </p>
        </div>

        <div className="grid grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)_minmax(0,0.8fr)_minmax(0,1fr)] px-6 pt-3 pb-1 text-[11px] font-medium tracking-[0.12em] text-gum-grey uppercase">
          <span>Payer</span>
          <span>One-time address</span>
          <span className="text-right">Amount</span>
          <span className="text-right">Status</span>
        </div>

        <ol className="relative overflow-hidden" style={{ height: ROW_HEIGHT * ROWS }}>
          {rows.map((row) => (
            <li
              key={row.key}
              className={cn(
                "absolute inset-x-0 top-0 grid grid-cols-[minmax(0,1.3fr)_minmax(0,1fr)_minmax(0,0.8fr)_minmax(0,1fr)] items-center border-t border-gum-grey/20 px-6 text-[14px]",
                "transition-transform duration-[480ms] ease-[cubic-bezier(0.16,1,0.3,1)]",
              )}
              style={
                {
                  height: ROW_HEIGHT,
                  transform: `translateY(${row.position * ROW_HEIGHT}px)`,
                } as CSSProperties
              }
            >
              <span className="flex min-w-0 items-center gap-3">
                <span className="flex size-8 shrink-0 items-center justify-center rounded-full bg-gum-black text-[11px] font-semibold text-gum-white">
                  {initials(row.deposit.payer)}
                </span>
                <span className="min-w-0">
                  <span className="block truncate font-medium">{row.deposit.payer}</span>
                  <span className="block truncate font-mono text-[11.5px] text-gum-grey">
                    {row.deposit.ref}
                  </span>
                </span>
              </span>
              <span className="font-mono text-[13px]">{row.deposit.address}</span>
              <span className="tabular text-right font-medium">{row.deposit.amount}</span>
              <span className="flex justify-end">
                {row.settled ? (
                  <span
                    key="settled"
                    className="landing-pop inline-flex h-6 items-center rounded-full bg-gum-pink px-2.5 text-[11.5px] font-semibold text-gum-white"
                  >
                    Settled
                  </span>
                ) : (
                  <span
                    key="awaiting"
                    className="inline-flex h-6 items-center gap-1.5 rounded-full border border-gum-grey/40 px-2.5 text-[11.5px] font-medium text-gum-grey"
                  >
                    <span className="size-1.5 animate-pulse rounded-full bg-gum-grey" />
                    Awaiting
                  </span>
                )}
              </span>
            </li>
          ))}
        </ol>
      </Stage>
    </div>
  );
}
