import Image from "next/image";
import type { ReactNode } from "react";
import { PricingDialog } from "@/components/pricing-dialog";
import { BuildWith } from "./build-with";
import { PayWith } from "./pay-with";

/**
 * Three things worth knowing before Get Started, under the hero: the chains
 * a deposit can settle on, the checkouts and wallets a payer can fund it
 * from, and that the whole thing is open source and runs on your own
 * infrastructure. Each card leads with the real marks rather than a claim.
 */

/** Where a deposit can come from or go to, in the order the grid shows them. */
const CHAINS = [
  { name: "Ethereum", src: "/logos/ethereum.svg" },
  { name: "Solana", src: "/logos/solana.svg" },
  { name: "Monad", src: "/payment-icons/monad.svg" },
  { name: "Arbitrum", src: "/logos/arbitrum.svg" },
  { name: "Base", src: "/logos/base.svg" },
  { name: "Hyperliquid", src: "/logos/hyperliquid.png" },
  { name: "Polygon", src: "/logos/polygon.svg" },
  { name: "Tron", src: "/logos/tron.svg" },
  { name: "BNB Chain", src: "/logos/bnb-chain.svg" },
  { name: "Tempo", src: "/logos/tempo.svg" },
] as const;

const REPOSITORY = "https://github.com/nkrishang/payday";

export function ProofPoints() {
  return (
    <section
      aria-label="Why Payday"
      className="mx-auto grid w-full max-w-[1320px] gap-5 px-5 sm:px-8 lg:grid-cols-3"
    >
      <Card
        eyebrow="Multi-chain"
        title="Any chain in. Your chain out."
        body="Convert better by letting your users pay where they already have funds. You choose the chain and destination where those funds settle."
      >
        <ul
          aria-label="Chains"
          className="mt-auto grid grid-cols-5 gap-px overflow-hidden rounded-[12px] border border-brand-black/10 bg-brand-black/10"
        >
          {CHAINS.map((chain) => (
            <li
              key={chain.name}
              title={chain.name}
              className="landing-chain relative aspect-square bg-brand-white"
            >
              {/* The mark as a grey halftone, until the pointer arrives. */}
              <span
                aria-hidden="true"
                className="landing-chain-halftone"
                style={{ "--logo": `url(${chain.src})` } as React.CSSProperties}
              />
              <Image
                src={chain.src}
                width={64}
                height={64}
                alt={chain.name}
                className="landing-chain-logo"
              />
            </li>
          ))}
        </ul>
      </Card>

      <Card
        eyebrow="Works with every checkout"
        title="Not another checkout."
        body="Payday issues a destination address for every deposit intent, fund-able by every hosted checkout solution, wallet, exchange or on-ramp."
      >
        <div className="mt-auto pt-6">
          <div className="rounded-[14px] border border-brand-black/10 bg-brand-white p-4 shadow-[0_18px_40px_-28px_rgb(15_15_14/0.45)]">
            <div className="flex items-baseline justify-between gap-3">
              <p className="text-[11px] font-medium tracking-[0.14em] text-brand-subtle uppercase">
                Deposit
              </p>
              <p className="tabular text-[11px] text-brand-subtle">Expires in 59:52</p>
            </div>
            <p className="tabular mt-1.5 flex items-baseline gap-2 text-[26px] leading-none font-semibold tracking-tight">
              250.00
              <span className="flex items-center gap-1.5 text-[13px] font-medium text-brand-subtle">
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
              <span className="text-brand-subtle">One-time address</span>
              <span className="font-mono">0x9a3F…A0c2</span>
            </div>
            <div className="mt-3.5">
              <PayWith />
            </div>
            <p className="mt-3 flex items-center justify-center gap-1.5 text-[11px] text-brand-subtle">
              <span className="size-1.5 rounded-full bg-brand-green" />
              Secured by Payday
            </p>
          </div>
        </div>
      </Card>

      <Card
        eyebrow="Open source"
        title="Self-hostable. Build with your agent."
        body={
          <>
            You can run Payday on your own infrastructure. Customize it with your AI agents. Or, use
            the hosted service with{" "}
            <PricingDialog
              appearance="light"
              trigger="transparent pricing"
              triggerClassName="font-medium text-brand-black underline decoration-brand-black/30 underline-offset-[3px] transition-colors hover:decoration-brand-black"
            />
            .
          </>
        }
      >
        <div className="mt-auto grid gap-4 pt-6">
          <a
            href={REPOSITORY}
            target="_blank"
            rel="noopener noreferrer"
            className="flex h-12 items-center gap-3 rounded-[12px] border border-brand-black/10 bg-white/60 px-4 text-[13.5px] font-medium transition-colors hover:border-brand-black/40"
          >
            <Image src="/logos/github.svg" width={24} height={24} alt="" className="size-5" />
            nkrishang/payday
            <span className="ml-auto text-[12px] font-normal text-brand-subtle">View source</span>
          </a>
          <BuildWith />
        </div>
      </Card>
    </section>
  );
}

function Card({
  eyebrow,
  title,
  body,
  children,
}: {
  eyebrow: string;
  title: string;
  body: ReactNode;
  children: ReactNode;
}) {
  return (
    <article className="flex min-h-[340px] flex-col overflow-hidden rounded-[18px] border border-brand-black/[0.14] p-6 sm:p-7">
      <p className="text-[11px] font-medium tracking-[0.14em] text-brand-subtle uppercase">
        {eyebrow}
      </p>
      <h2 className="mt-2.5 text-[20px] leading-snug font-medium tracking-[-0.02em] text-balance">
        {title}
      </h2>
      <p className="mt-2 text-[14px] leading-relaxed text-brand-subtle">{body}</p>
      {children}
    </article>
  );
}
