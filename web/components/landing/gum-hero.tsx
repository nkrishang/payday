"use client";

import Image from "next/image";
import { useEffect, useRef, type CSSProperties, type ReactNode } from "react";
import { cn } from "@/lib/cn";
import { Stage, useInView, useLoopClock, useReducedMotion } from "./stage";

/**
 * The product in one picture, with gum doing the work.
 *
 * Payers on the left, each with the wallet they use. In the middle, Gum:
 * the mark, as a blob under an SVG "goo" filter, so anything pink that
 * comes near it stretches and merges. For every deposit, a drop of gum
 * splits off the blob and settles beside the payer: that is their
 * address, and it says so, with its terms. The payer's coin flies in and
 * sticks to it. The drop then carries the funds back through Gum and out
 * to the merchant's wallet on the right, where a ledger row appears with
 * the payer's name. One address per deposit; the deposit sticks to the
 * one who made it.
 *
 * Everything is a function of one clock. The pointer is sticky too: a
 * hidden drop follows it, so near the blob a strand of gum reaches out.
 */

const W = 800;
const H = 560;

const PAYER_X = 60;
const CHIP_X = 216;
const GUM = { x: 488, y: 292, size: 204 } as const;
const WALLET = { x: 608, y: 150, width: 180, height: 264 } as const;
const LAND = { x: WALLET.x + 6, y: 292 } as const;

const ROW_Y = [98, 190, 282, 374, 466] as const;

const PAYERS = [
  { name: "Jordan Diaz", initials: "JD", wallet: "MetaMask", src: "/logos/metamask.svg" },
  { name: "Maya Patel", initials: "MP", wallet: "Coinbase", src: "/logos/coinbase.svg" },
  { name: "Acme Corp", initials: "AC", wallet: "Binance", src: "/logos/binance.svg" },
  { name: "Lena Fischer", initials: "LF", wallet: "Phantom", src: "/logos/phantom.svg" },
  { name: "Tomás Rivera", initials: "TR", wallet: "Stripe", src: "/logos/stripe.svg" },
] as const;

const DEPOSITS = [
  { payer: 0, amount: "250.00", currency: "USDC", address: "0x9a3F…A0c2", expires: "59:52" },
  { payer: 1, amount: "1,200.00", currency: "USDT", address: "0x41c9…7E1b", expires: "14:59" },
  { payer: 2, amount: "18,000.00", currency: "USDC", address: "0xB70d…33fA", expires: "23:58" },
  { payer: 3, amount: "75.00", currency: "USDC", address: "0x6E5c…91C8", expires: "59:58" },
  { payer: 4, amount: "420.00", currency: "USDT", address: "0xF2a8…0b4D", expires: "29:59" },
  { payer: 0, amount: "60.00", currency: "USDC", address: "0x8d3A…4E7f", expires: "09:59" },
  { payer: 1, amount: "5,000.00", currency: "USDC", address: "0x0C4e…bD12", expires: "59:51" },
  { payer: 2, amount: "980.00", currency: "USDT", address: "0x2B9f…C6a1", expires: "44:57" },
] as const;

/** A deposit begins every PERIOD; each beat of its life, in ms after that. */
const PERIOD_MS = 2_000;
const AT = {
  minted: 620,
  pay: 900,
  paid: 1_650,
  settle: 2_350,
  absorbed: 3_150,
  out: 3_250,
  landed: 4_050,
  end: 4_200,
} as const;
const TOTAL_MS = PERIOD_MS * DEPOSITS.length;
/** Deposits alive at once, at most. */
const SLOTS = 3;
/** Rows the wallet shows. */
const ROWS = 4;
const ROW_HEIGHT = 40;

function easeInOut(f: number): number {
  return f < 0.5 ? 4 * f * f * f : 1 - Math.pow(-2 * f + 2, 3) / 2;
}
function easeOut(f: number): number {
  return 1 - Math.pow(1 - f, 3);
}
function easeIn(f: number): number {
  return f * f * f;
}
function span(age: number, from: number, to: number): number {
  const f = (age - from) / (to - from);
  return f < 0 ? 0 : f > 1 ? 1 : f;
}
function lerp(a: number, b: number, f: number): number {
  return a + (b - a) * f;
}
function mod(n: number, m: number): number {
  return ((n % m) + m) % m;
}

type Live = {
  number: number;
  key: number;
  age: number;
  deposit: (typeof DEPOSITS)[number];
  payer: (typeof PAYERS)[number];
  y: number;
};

export function GumHero() {
  const reduced = useReducedMotion();
  const frameRef = useRef<HTMLDivElement>(null);
  const inView = useInView(frameRef);
  const t = useLoopClock(TOTAL_MS, reduced ? PERIOD_MS * 3 + 1_800 : null, inView);
  const leanRef = useRef<SVGGElement>(null);
  const pointerRef = useRef<SVGCircleElement>(null);

  // The pointer, at sixty frames a second, on two elements and nothing else.
  useEffect(() => {
    const frame = frameRef.current;
    if (!frame || !inView) return;
    let target: { x: number; y: number } | null = null;
    let px = GUM.x;
    let py = GUM.y;
    let pr = 0;
    let lx = 0;
    let ly = 0;
    const onMove = (event: PointerEvent) => {
      const box = frame.getBoundingClientRect();
      target = {
        x: ((event.clientX - box.left) / box.width) * W,
        y: ((event.clientY - box.top) / box.height) * H,
      };
    };
    const onLeave = () => {
      target = null;
    };
    frame.addEventListener("pointermove", onMove);
    frame.addEventListener("pointerleave", onLeave);
    let raf = 0;
    const tick = () => {
      let tx = 0;
      let ty = 0;
      if (target) {
        const dx = target.x - GUM.x;
        const dy = target.y - GUM.y;
        const dist = Math.hypot(dx, dy) || 1;
        const reach = Math.min(26, dist * 0.12);
        tx = (dx / dist) * reach;
        ty = (dy / dist) * reach;
        px += (target.x - px) * 0.22;
        py += (target.y - py) * 0.22;
        pr += (22 - pr) * 0.15;
      } else {
        pr += (0 - pr) * 0.15;
      }
      lx += (tx - lx) * 0.08;
      ly += (ty - ly) * 0.08;
      leanRef.current?.setAttribute("transform", `translate(${lx.toFixed(1)} ${ly.toFixed(1)})`);
      const drop = pointerRef.current;
      if (drop) {
        drop.setAttribute("cx", px.toFixed(1));
        drop.setAttribute("cy", py.toFixed(1));
        drop.setAttribute("r", Math.max(0, pr).toFixed(1));
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(raf);
      frame.removeEventListener("pointermove", onMove);
      frame.removeEventListener("pointerleave", onLeave);
    };
  }, [inView]);

  // Which deposits are alive, and how far along each is.
  const newest = Math.floor(t / PERIOD_MS);
  const live: Live[] = [];
  for (let k = 0; k < SLOTS; k++) {
    const number = newest - k;
    const age = t - number * PERIOD_MS;
    if (age < 0 || age >= AT.end) continue;
    const key = mod(number, DEPOSITS.length);
    const deposit = DEPOSITS[key]!;
    live.push({
      number,
      key,
      age,
      deposit,
      payer: PAYERS[deposit.payer]!,
      y: ROW_Y[deposit.payer]!,
    });
  }

  // The wallet's rows: everything that has landed, newest first.
  const rows: { key: number; position: number; deposit: (typeof DEPOSITS)[number] }[] = [];
  for (let k = 2; k < 2 + ROWS + 1; k++) {
    const number = newest - k;
    if (t - number * PERIOD_MS < AT.landed) continue;
    rows.push({
      key: mod(number, DEPOSITS.length),
      position: rows.length,
      deposit: DEPOSITS[mod(number, DEPOSITS.length)]!,
    });
  }
  const justLanded = live.some((entry) => entry.age >= AT.landed);

  // The blob: breathing, and swelling as something is absorbed.
  let swell = 0;
  for (const entry of live) {
    if (entry.age >= AT.absorbed - 200 && entry.age < AT.absorbed + 500) {
      swell = Math.max(
        swell,
        Math.sin(span(entry.age, AT.absorbed - 200, AT.absorbed + 500) * Math.PI) * 0.07,
      );
    }
  }
  const breathe = 1 + Math.sin(t / 900) * 0.015 + swell;
  const tilt = Math.sin(t / 1300) * 2.5;

  return (
    <div ref={frameRef} className="touch-none select-none">
      <Stage
        width={W}
        height={H}
        label="Payers with their own wallets on the left. For each deposit, Gum mints an address beside the payer; their payment sticks to it, passes through Gum, and lands in the merchant's wallet with the payer's name."
      >
        <Caption x={PAYER_X - 30} y={40}>
          Any wallet, any chain
        </Caption>
        <Caption x={CHIP_X + 26} y={40}>
          One address per deposit
        </Caption>

        {/* The payers. */}
        {PAYERS.map((payer, index) => (
          <div
            key={payer.name}
            className="absolute flex w-[118px] -translate-x-1/2 flex-col items-center"
            style={{ left: PAYER_X, top: ROW_Y[index]! - 26 }}
          >
            <span className="relative flex size-[48px] items-center justify-center rounded-full bg-gum-black text-[13px] font-semibold text-gum-white">
              {payer.initials}
              <span className="absolute -right-1.5 -bottom-1.5 flex size-[22px] items-center justify-center rounded-full bg-gum-white ring-1 ring-gum-grey/30">
                <Image
                  src={payer.src}
                  width={24}
                  height={24}
                  alt={payer.wallet}
                  className="size-3.5"
                />
              </span>
            </span>
            <span className="mt-2 text-[11px] leading-none font-medium whitespace-nowrap">
              {payer.name}
            </span>
          </div>
        ))}

        {/* Gum, and everything pink that sticks to it. */}
        <svg viewBox={`0 0 ${W} ${H}`} className="absolute inset-0 size-full" aria-hidden="true">
          <defs>
            <filter id="gum-goo" x="-10%" y="-10%" width="120%" height="120%">
              <feGaussianBlur in="SourceGraphic" stdDeviation="8" result="blur" />
              <feColorMatrix
                in="blur"
                mode="matrix"
                values="1 0 0 0 0  0 1 0 0 0  0 0 1 0 0  0 0 0 20 -9"
              />
            </filter>
          </defs>
          <g filter="url(#gum-goo)">
            <g ref={leanRef}>
              <g
                transform={`translate(${GUM.x} ${GUM.y}) rotate(${tilt.toFixed(2)}) scale(${breathe.toFixed(3)})`}
              >
                <image
                  href="/gum/mark.png"
                  x={-GUM.size / 2}
                  y={-GUM.size / 2}
                  width={GUM.size}
                  height={GUM.size}
                />
              </g>
            </g>
            {live.map((entry) => (
              <Drops key={entry.key} entry={entry} />
            ))}
            <circle ref={pointerRef} r="0" fill="#ff5ca8" />
          </g>

          {/* The coin's own face, riding on top of the goo until it is absorbed. */}
          {live.map((entry) => {
            if (entry.age < AT.pay || entry.age >= AT.paid) return null;
            const f = easeInOut(span(entry.age, AT.pay, AT.paid));
            const x = lerp(PAYER_X + 34, CHIP_X, f);
            const y = lerp(entry.y, entry.y, f) - Math.sin(f * Math.PI) * 18;
            const opacity = f < 0.78 ? 1 : 1 - (f - 0.78) / 0.22;
            return (
              <g
                key={entry.key}
                transform={`translate(${x.toFixed(1)} ${y.toFixed(1)})`}
                opacity={opacity}
              >
                <circle r="9" fill="#f7f7f5" />
                <image
                  href={
                    entry.deposit.currency === "USDT"
                      ? "/payment-icons/usdt.svg"
                      : "/payment-icons/usdc.svg"
                  }
                  x="-8"
                  y="-8"
                  width="16"
                  height="16"
                />
              </g>
            );
          })}
        </svg>

        {/* Each minted address, beside its payer. */}
        {live.map((entry) => {
          if (entry.age < AT.minted - 120 || entry.age >= AT.settle + 200) return null;
          const funded = entry.age >= AT.paid;
          const leaving = entry.age >= AT.settle;
          return (
            <div
              key={entry.key}
              className={cn(
                "landing-scene-line absolute w-[140px] leading-[1.2] transition-opacity duration-200",
                leaving && "opacity-0",
              )}
              style={{ left: CHIP_X + 26, top: entry.y - 15 }}
            >
              <p className="font-mono text-[12.5px] text-gum-black">{entry.deposit.address}</p>
              <p
                className={cn(
                  "mt-1 text-[10.5px] font-medium whitespace-nowrap transition-colors",
                  funded ? "text-gum-pink" : "text-gum-grey",
                )}
              >
                {funded
                  ? `Funded · ${entry.deposit.amount} ${entry.deposit.currency}`
                  : entry.age >= AT.pay
                    ? `Awaiting ${entry.deposit.amount} ${entry.deposit.currency}`
                    : `${entry.deposit.amount} ${entry.deposit.currency} · ${entry.deposit.expires} left`}
              </p>
            </div>
          );
        })}

        {/* The merchant's wallet: where it all lands, with names on it. */}
        <div
          className="absolute overflow-hidden rounded-[16px] bg-gum-black text-gum-white"
          style={{ left: WALLET.x, top: WALLET.y, width: WALLET.width, height: WALLET.height }}
        >
          <div className="px-4 pt-4">
            <p className="flex items-center justify-between text-[10px] font-medium tracking-[0.14em] text-gum-white/55 uppercase">
              Your wallet
              <span
                className={cn(
                  "size-2 rounded-full bg-gum-pink transition-transform duration-300",
                  justLanded ? "scale-150" : "scale-100",
                )}
              />
            </p>
            <p className="mt-1.5 font-mono text-[13px]">0x3C44…93BC</p>
            <p className="mt-1.5 flex items-center gap-1.5 text-[11px] font-medium text-gum-white/85">
              <Image src="/logos/base.svg" width={24} height={24} alt="" className="size-3.5" />
              Base
            </p>
          </div>
          <div className="mx-4 mt-3 border-t border-gum-white/[0.1]" />
          <ol className="relative mx-4" style={{ height: ROW_HEIGHT * ROWS }}>
            {rows.map((row) => (
              <li
                key={row.key}
                className="absolute inset-x-0 top-0 flex items-center justify-between gap-2 border-b border-gum-white/[0.06] text-[11.5px] transition-transform duration-[480ms] ease-[cubic-bezier(0.16,1,0.3,1)]"
                style={{
                  height: ROW_HEIGHT,
                  transform: `translateY(${row.position * ROW_HEIGHT}px)`,
                }}
              >
                <span className="truncate text-gum-white/85">
                  {PAYERS[row.deposit.payer]!.name}
                </span>
                <span className="tabular shrink-0 font-semibold text-gum-pink">
                  +{row.deposit.amount}
                </span>
              </li>
            ))}
          </ol>
        </div>
      </Stage>
    </div>
  );
}

/** The pink drops of one deposit, through its life, inside the goo. */
function Drops({ entry }: { entry: Live }) {
  const { age, y } = entry;
  const circles: ReactNode[] = [];

  // Minted: a drop leaves the blob and settles beside the payer.
  if (age < AT.settle) {
    const f = easeOut(span(age, 0, AT.minted));
    const x = lerp(GUM.x - 70, CHIP_X, f);
    const cy = lerp(GUM.y, y, f) - Math.sin(f * Math.PI) * 30;
    const pulse = age >= AT.paid ? Math.sin(span(age, AT.paid, AT.paid + 500) * Math.PI) * 5 : 0;
    circles.push(<circle key="chip" cx={x} cy={cy} r={lerp(5, 17, f) + pulse} fill="#ff5ca8" />);
  }

  // Paid: the payer's coin flies in and sticks to it.
  if (age >= AT.pay && age < AT.paid) {
    const f = easeInOut(span(age, AT.pay, AT.paid));
    const x = lerp(PAYER_X + 34, CHIP_X, f);
    const cy = y - Math.sin(f * Math.PI) * 18;
    circles.push(<circle key="coin" cx={x} cy={cy} r={11} fill="#ff5ca8" />);
  }

  // Settling: the drop carries the funds back into Gum…
  if (age >= AT.settle && age < AT.absorbed) {
    const f = easeIn(span(age, AT.settle, AT.absorbed));
    const x = lerp(CHIP_X, GUM.x - 40, f);
    const cy = lerp(y, GUM.y, f);
    circles.push(<circle key="settle" cx={x} cy={cy} r={lerp(17, 11, f)} fill="#ff5ca8" />);
  }

  // …and out the far side into the wallet.
  if (age >= AT.out && age < AT.landed) {
    const f = easeInOut(span(age, AT.out, AT.landed));
    const x = lerp(GUM.x + 40, LAND.x, f);
    const cy = lerp(GUM.y, LAND.y, f) - Math.sin(f * Math.PI) * 22;
    circles.push(<circle key="out" cx={x} cy={cy} r={lerp(12, 7, f)} fill="#ff5ca8" />);
  }

  return <>{circles}</>;
}

function Caption({ x, y, children }: { x: number; y: number; children: ReactNode }) {
  return (
    <p
      className="absolute text-[10px] font-medium tracking-[0.14em] whitespace-nowrap text-gum-grey uppercase"
      style={{ left: x, top: y } as CSSProperties}
    >
      {children}
    </p>
  );
}
