"use client";

import { useEffect, useRef } from "react";
import { useInView, useReducedMotion } from "./stage";

/**
 * Gum, alive. The mark is a blob of gum in an SVG "goo" filter: a blur and
 * an alpha threshold, so anything pink that comes near it stretches into
 * it and merges. Deposits drip in from wallets on the left, get absorbed,
 * and a drop peels off the far side into the merchant's wallet. The
 * pointer is sticky too: a hidden drop follows it, so near the blob a
 * strand of gum reaches out.
 *
 * Everything is a pure function of one clock, driven imperatively: each
 * frame writes a few attributes on a few elements, nothing re-renders.
 */

const W = 720;
const H = 560;

const GUM = { x: 410, y: 280, size: 236 } as const;
const WALLET = { x: 650, y: 280 } as const;
const SOURCE_X = 54;

const SOURCES = [
  { name: "Coinbase", src: "/logos/coinbase.svg", y: 96 },
  { name: "MetaMask", src: "/logos/metamask.svg", y: 188 },
  { name: "Binance", src: "/logos/binance.svg", y: 280 },
  { name: "Phantom", src: "/logos/phantom.svg", y: 372 },
  { name: "Stripe", src: "/logos/stripe.svg", y: 464 },
] as const;

/** One deposit after another, each from a source and each of an amount. */
const DEPOSITS = [
  { amount: "250.00", source: 0 },
  { amount: "1,200.00", source: 2 },
  { amount: "75.00", source: 1 },
  { amount: "18,000.00", source: 4 },
  { amount: "420.00", source: 3 },
  { amount: "60.00", source: 0 },
  { amount: "5,000.00", source: 2 },
  { amount: "980.00", source: 1 },
] as const;

/** A deposit every PERIOD; it travels for TRAVEL, sits, then settles out. */
const PERIOD_MS = 1_500;
const TRAVEL_MS = 1_100;
const SETTLE_AT_MS = 1_900;
const SETTLE_MS = 800;
const LIFE_MS = SETTLE_AT_MS + SETTLE_MS;
const TOTAL_MS = PERIOD_MS * DEPOSITS.length;
/** Drops in flight at once, at most. */
const SLOTS = 3;

function easeInOut(f: number): number {
  return f < 0.5 ? 4 * f * f * f : 1 - Math.pow(-2 * f + 2, 3) / 2;
}

function clamp01(v: number): number {
  return v < 0 ? 0 : v > 1 ? 1 : v;
}

export function GumHero() {
  const reduced = useReducedMotion();
  const frameRef = useRef<HTMLDivElement>(null);
  const inView = useInView(frameRef);
  const svgRef = useRef<SVGSVGElement>(null);
  const gumRef = useRef<SVGGElement>(null);
  const pointerRef = useRef<SVGCircleElement>(null);
  const dropRefs = useRef<(SVGCircleElement | null)[]>([]);
  const settleRefs = useRef<(SVGCircleElement | null)[]>([]);
  const popRef = useRef<SVGTextElement>(null);
  const pointer = useRef<{ x: number; y: number } | null>(null);

  useEffect(() => {
    if (!inView) return;
    const svg = svgRef.current;
    if (!svg) return;

    // Pointer in the stage's own coordinates.
    const onMove = (event: PointerEvent) => {
      const box = svg.getBoundingClientRect();
      pointer.current = {
        x: ((event.clientX - box.left) / box.width) * W,
        y: ((event.clientY - box.top) / box.height) * H,
      };
    };
    const onLeave = () => {
      pointer.current = null;
    };
    svg.addEventListener("pointermove", onMove);
    svg.addEventListener("pointerleave", onLeave);

    let frame = 0;
    let last = performance.now();
    let t = 0;
    // The blob's centre and the pointer drop, each easing toward a target.
    let gx = GUM.x;
    let gy = GUM.y;
    let px = GUM.x;
    let py = GUM.y;
    let pr = 0;
    let shownPop = -1;

    const tick = (now: number) => {
      const dt = Math.min(now - last, 100);
      last = now;
      if (!reduced) t = (t + dt) % TOTAL_MS;

      // Where the blob wants to be: a little toward the pointer, if any.
      const target = pointer.current;
      let tx = GUM.x;
      let ty = GUM.y;
      if (target) {
        const dx = target.x - GUM.x;
        const dy = target.y - GUM.y;
        const dist = Math.hypot(dx, dy) || 1;
        const reach = Math.min(28, dist * 0.12);
        tx += (dx / dist) * reach;
        ty += (dy / dist) * reach;
      }
      gx += (tx - gx) * 0.08;
      gy += (ty - gy) * 0.08;

      // The pointer's drop: grows when the pointer is here, follows it with a lag.
      if (target) {
        px += (target.x - px) * 0.22;
        py += (target.y - py) * 0.22;
        pr += (22 - pr) * 0.15;
      } else {
        pr += (0 - pr) * 0.15;
      }
      const pointerDrop = pointerRef.current;
      if (pointerDrop) {
        pointerDrop.setAttribute("cx", px.toFixed(1));
        pointerDrop.setAttribute("cy", py.toFixed(1));
        pointerDrop.setAttribute("r", Math.max(0, pr).toFixed(1));
      }

      // The deposits: which are in flight, and the swell of the one just landed.
      let swell = 0;
      const newest = Math.floor(t / PERIOD_MS);
      for (let k = 0; k < SLOTS; k++) {
        const number = newest - k;
        const age = t - number * PERIOD_MS;
        const deposit = DEPOSITS[((number % DEPOSITS.length) + DEPOSITS.length) % DEPOSITS.length]!;
        const slot = ((number % SLOTS) + SLOTS) % SLOTS;
        const drop = dropRefs.current[slot];
        const settle = settleRefs.current[slot];
        if (!drop || !settle) continue;

        if (age < 0 || age >= LIFE_MS) {
          drop.setAttribute("r", "0");
          settle.setAttribute("r", "0");
          continue;
        }

        // In: from the source to the blob, along a shallow arc.
        if (age < TRAVEL_MS + 250) {
          const f = easeInOut(clamp01(age / TRAVEL_MS));
          const source = SOURCES[deposit.source]!;
          const sx = SOURCE_X + 26;
          const sy = source.y;
          const x = sx + (gx - sx) * f;
          const y = sy + (gy - sy) * f - Math.sin(f * Math.PI) * 40;
          drop.setAttribute("cx", x.toFixed(1));
          drop.setAttribute("cy", y.toFixed(1));
          drop.setAttribute("r", (14 - 4 * f).toFixed(1));
        } else {
          drop.setAttribute("r", "0");
        }
        if (age >= TRAVEL_MS - 100 && age < TRAVEL_MS + 600) {
          const f = clamp01((age - (TRAVEL_MS - 100)) / 700);
          swell = Math.max(swell, Math.sin(f * Math.PI) * 0.07);
        }

        // Out: from the blob to the wallet.
        if (age >= SETTLE_AT_MS - 200 && age < LIFE_MS) {
          const f = easeInOut(clamp01((age - (SETTLE_AT_MS - 200)) / (SETTLE_MS + 200)));
          const x = gx + (WALLET.x - 66 - gx) * f;
          const y = gy + (WALLET.y - gy) * f + Math.sin(f * Math.PI) * 26;
          settle.setAttribute("cx", x.toFixed(1));
          settle.setAttribute("cy", y.toFixed(1));
          settle.setAttribute("r", (6 + 7 * Math.sin(f * Math.PI)).toFixed(1));
        } else {
          settle.setAttribute("r", "0");
        }

        // Landed: the wallet says so.
        if (age >= LIFE_MS - 60 && shownPop !== number && popRef.current) {
          shownPop = number;
          popRef.current.textContent = `+${deposit.amount}`;
          popRef.current.classList.remove("landing-hero-pop");
          void popRef.current.getBoundingClientRect();
          popRef.current.classList.add("landing-hero-pop");
        }
      }

      // The blob itself: breathing, leaning, swelling when something lands.
      const gum = gumRef.current;
      if (gum) {
        const breathe = 1 + Math.sin(t / 900) * 0.015 + swell;
        const lean = Math.sin(t / 1300) * 2.5 + (gx - GUM.x) * 0.12;
        gum.setAttribute(
          "transform",
          `translate(${gx.toFixed(1)} ${gy.toFixed(1)}) rotate(${lean.toFixed(2)}) scale(${breathe.toFixed(3)})`,
        );
      }

      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);

    return () => {
      cancelAnimationFrame(frame);
      svg.removeEventListener("pointermove", onMove);
      svg.removeEventListener("pointerleave", onLeave);
    };
  }, [inView, reduced]);

  return (
    <div ref={frameRef} className="relative w-full">
      <svg
        ref={svgRef}
        viewBox={`0 0 ${W} ${H}`}
        role="img"
        aria-label="Deposits from wallets and exchanges stick to Gum and settle into the merchant's wallet."
        className="block h-auto w-full touch-none select-none"
      >
        <defs>
          <filter id="gum-goo" x="-20%" y="-20%" width="140%" height="140%">
            <feGaussianBlur in="SourceGraphic" stdDeviation="9" result="blur" />
            <feColorMatrix
              in="blur"
              mode="matrix"
              values="1 0 0 0 0  0 1 0 0 0  0 0 1 0 0  0 0 0 20 -9"
              result="goo"
            />
          </filter>
          <clipPath id="gum-source-clip">
            <circle r="17" />
          </clipPath>
        </defs>

        {/* The sources, on the left: where a payer's funds already are. */}
        {SOURCES.map((source) => (
          <g key={source.name} transform={`translate(${SOURCE_X} ${source.y})`}>
            <circle r="26" fill="#f7f7f5" stroke="#6f6e6e" strokeOpacity="0.3" />
            <image
              href={source.src}
              x="-14"
              y="-14"
              width="28"
              height="28"
              preserveAspectRatio="xMidYMid meet"
            />
          </g>
        ))}

        {/* The gum, and everything that sticks to it. */}
        <g filter="url(#gum-goo)">
          <g ref={gumRef} transform={`translate(${GUM.x} ${GUM.y})`}>
            <image
              href="/gum/mark.png"
              x={-GUM.size / 2}
              y={-GUM.size / 2}
              width={GUM.size}
              height={GUM.size}
            />
          </g>
          {Array.from({ length: SLOTS }, (_, slot) => (
            <circle
              key={`in-${slot}`}
              ref={(element) => {
                dropRefs.current[slot] = element;
              }}
              r="0"
              fill="#ff5ca8"
            />
          ))}
          {Array.from({ length: SLOTS }, (_, slot) => (
            <circle
              key={`out-${slot}`}
              ref={(element) => {
                settleRefs.current[slot] = element;
              }}
              r="0"
              fill="#ff5ca8"
            />
          ))}
          <circle ref={pointerRef} r="0" fill="#ff5ca8" />
        </g>

        {/* The merchant's wallet, on the right: where it all settles. */}
        <g transform={`translate(${WALLET.x} ${WALLET.y})`}>
          <rect x="-66" y="-46" width="126" height="92" rx="14" fill="#121212" />
          <text
            x="-50"
            y="-22"
            fontSize="9.5"
            fontWeight="500"
            letterSpacing="1.4"
            fill="#f7f7f5"
            fillOpacity="0.55"
            style={{ fontFamily: "inherit" }}
          >
            YOUR WALLET
          </text>
          <text
            x="-50"
            y="4"
            fontSize="13"
            fill="#f7f7f5"
            style={{ fontFamily: "var(--font-mono), ui-monospace, monospace" }}
          >
            0x3C44…93BC
          </text>
          <g transform="translate(-50 20)">
            <image href="/logos/base.svg" x="0" y="0" width="14" height="14" />
            <text
              x="19"
              y="11.5"
              fontSize="11"
              fontWeight="500"
              fill="#f7f7f5"
              fillOpacity="0.85"
              style={{ fontFamily: "inherit" }}
            >
              Base
            </text>
          </g>
          <circle cx="44" cy="-30" r="4" fill="#ff5ca8" />
          <text
            ref={popRef}
            x="60"
            y="-60"
            textAnchor="end"
            className="landing-hero-pop tabular"
            fontSize="16"
            fontWeight="600"
            fill="#ff5ca8"
            style={{ fontFamily: "inherit" }}
          >
            +250.00
          </text>
        </g>
      </svg>
    </div>
  );
}
