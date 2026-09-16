"use client";

import { Wallet } from "lucide-react";
import Image from "next/image";
import { useRef } from "react";
import { cn } from "@/lib/cn";
import { Editor, Highlighted } from "./hero-scenes";
import { Stage, useInView, useLoopClock, useReducedMotion } from "./stage";

/**
 * The two ways a deposit request reaches a payer: the hosted page Gum
 * serves for it, and whatever the merchant builds from the same fields. A
 * switch flips between them on its own, and the picture under it swaps.
 */

const STAGE = { width: 600, height: 340 } as const;

const MODES = ["Hosted", "Headless"] as const;
const HOLD_MS = 4_400;
const TOTAL_MS = HOLD_MS * MODES.length;

const HEADLESS_CODE = `const deposit = await gum.depositRequests.create({
  amount: "250.00",
  currency: "USDC",
  payer: { name: user.displayName },
});

// Render it however you like.
deposit.address      // "0x9a3F…A0c2"
deposit.expires_at   // "2026-09-16T14:20:00Z"
deposit.hosted_url   // "https://gum.money/pay/dr_0198…"`;

export function EmbedModes() {
  const reduced = useReducedMotion();
  const frameRef = useRef<HTMLDivElement>(null);
  const inView = useInView(frameRef);
  const t = useLoopClock(TOTAL_MS, reduced ? 200 : null, inView);
  const mode = Math.floor(t / HOLD_MS) % MODES.length;

  return (
    <div ref={frameRef}>
      <Stage
        width={STAGE.width}
        height={STAGE.height}
        label="A deposit request as the hosted payment page Gum serves for it, and as the fields a merchant renders themselves."
        className="rounded-[18px] bg-gum-black text-gum-white"
      >
        <div className="flex h-full flex-col p-5">
          <div className="flex items-center justify-between">
            <div
              role="tablist"
              aria-label="Integration"
              className="relative grid grid-cols-2 rounded-[10px] bg-gum-white/[0.08] p-1 text-[13px] font-medium"
            >
              <span
                aria-hidden="true"
                className="absolute top-1 bottom-1 left-1 w-[calc(50%-4px)] rounded-[7px] bg-gum-white transition-transform duration-[380ms] ease-[cubic-bezier(0.16,1,0.3,1)]"
                style={{ transform: `translateX(${mode * 100}%)` }}
              />
              {MODES.map((label, index) => (
                <span
                  key={label}
                  role="tab"
                  aria-selected={index === mode}
                  className={cn(
                    "relative z-10 px-5 py-1.5 text-center transition-colors duration-300",
                    index === mode ? "text-gum-black" : "text-gum-white/55",
                  )}
                >
                  {label}
                </span>
              ))}
            </div>
            <p className="font-mono text-[12px] text-gum-white/55">
              {mode === 0 ? "gum.money/pay/dr_0198f80c…" : "server.ts"}
            </p>
          </div>

          <div className="relative mt-4 min-h-0 flex-1">
            {mode === 0 ? (
              <HostedPage key="hosted" />
            ) : (
              <div key="headless" className="landing-scene-step h-full">
                <Editor
                  file="server.ts"
                  footer={
                    <span className="flex items-center gap-2 text-gum-pink">
                      <span className="size-2 rounded-full bg-gum-pink" />
                      201 Created
                    </span>
                  }
                >
                  <Highlighted code={HEADLESS_CODE} />
                </Editor>
              </div>
            )}
          </div>
        </div>
      </Stage>
    </div>
  );
}

/** The hosted payment page, small: a merchant's name over a deposit and a pay button. */
function HostedPage() {
  return (
    <div className="landing-scene-step flex h-full flex-col overflow-hidden rounded-[14px] bg-gum-white text-gum-black">
      <div className="flex items-center gap-2 border-b border-gum-black/[0.08] px-4 py-2.5">
        <span className="flex gap-1.5" aria-hidden="true">
          <span className="size-2.5 rounded-full bg-gum-black/15" />
          <span className="size-2.5 rounded-full bg-gum-black/15" />
          <span className="size-2.5 rounded-full bg-gum-black/15" />
        </span>
        <span className="mx-auto rounded-[6px] bg-gum-black/[0.05] px-3 py-0.5 font-mono text-[11.5px] text-gum-grey">
          gum.money/pay/dr_0198f80c…f700
        </span>
      </div>
      <div className="grid flex-1 grid-cols-[minmax(0,1fr)_minmax(0,1.1fr)] gap-6 px-6 py-5">
        <div>
          <p className="flex items-center gap-2 text-[13px] font-semibold">
            <span className="flex size-5 items-center justify-center rounded-[6px] bg-gum-black text-[10px] font-bold text-gum-white">
              A
            </span>
            Acme
          </p>
          <p className="mt-4 text-[11px] font-medium tracking-[0.14em] text-gum-grey uppercase">
            Deposit
          </p>
          <p className="tabular mt-1.5 flex items-baseline gap-2 text-[30px] leading-none font-semibold tracking-tight">
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
          <p className="mt-3 text-[12px] text-gum-grey">
            For <span className="text-gum-black">Jordan Diaz</span> · expires in 59:52
          </p>
        </div>
        <div className="flex flex-col justify-end gap-2.5 text-[12.5px]">
          <div className="flex items-center justify-between gap-3 rounded-[9px] border border-gum-grey/30 px-3 py-2">
            <span className="text-gum-grey">One-time address</span>
            <span className="font-mono">0x9a3F…A0c2</span>
          </div>
          <div className="flex items-center justify-between gap-3 rounded-[9px] border border-gum-grey/30 px-3 py-2">
            <span className="text-gum-grey">Network</span>
            <span className="flex items-center gap-1.5 font-medium">
              <Image
                src="/payment-icons/monad.svg"
                width={64}
                height={64}
                alt=""
                className="size-4"
              />
              Monad
            </span>
          </div>
          <span className="flex h-11 items-center justify-center gap-2 rounded-[9px] bg-gum-black text-[13.5px] font-medium text-gum-white">
            <Wallet className="size-4" />
            Pay from wallet
          </span>
          <p className="flex items-center justify-center gap-1.5 text-[11px] text-gum-grey">
            <span className="size-1.5 rounded-full bg-gum-pink" />
            Secured by Gum
          </p>
        </div>
      </div>
    </div>
  );
}
