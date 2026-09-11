import Image from "next/image";
import type { ReactNode } from "react";
import { PricingDialog } from "@/components/pricing-dialog";
import { BuildWith } from "./build-with";
import { LogoDrop, Mark } from "./logo-cloud";

/**
 * Three things worth knowing before Get Started, under the hero: the chains
 * a deposit can settle on, the checkouts and wallets a payer can fund it
 * from, and that the whole thing is open source and runs on your own
 * infrastructure. Each card leads with the real marks rather than a claim.
 */

const CHAINS = [
  { name: "Monad", src: "/payment-icons/monad.svg" },
  { name: "Base", src: "/logos/base.svg" },
  { name: "Arbitrum", src: "/logos/arbitrum.svg" },
] as const;

/** Where a payer's funds already are: wallets, exchanges, on-ramps. */
const CHECKOUTS = [
  { name: "Coinbase", src: "/logos/coinbase.svg" },
  { name: "MetaMask", src: "/logos/metamask.svg" },
  { name: "Binance", src: "/logos/binance.svg" },
  { name: "Phantom", src: "/logos/phantom.svg" },
  { name: "Kraken", src: "/logos/kraken.svg" },
  { name: "fun.xyz", src: "/logos/fun.svg" },
  { name: "Rainbow", src: "/logos/rainbow.svg" },
  { name: "OKX", src: "/logos/okx.svg" },
  { name: "Stripe", src: "/logos/stripe.svg" },
  { name: "Ledger", src: "/logos/ledger.svg" },
  { name: "WalletConnect", src: "/logos/walletconnect.svg" },
  { name: "Bybit", src: "/logos/bybit.svg" },
  { name: "Trust Wallet", src: "/logos/trust.svg" },
  { name: "Robinhood", src: "/logos/robinhood.svg" },
  { name: "KuCoin", src: "/logos/kucoin.svg" },
  { name: "PayPal", src: "/logos/paypal.svg" },
  { name: "Crypto.com", src: "/logos/crypto-com.svg" },
  { name: "Rabby", src: "/logos/rabby.svg" },
  { name: "Gemini", src: "/logos/gemini.svg" },
  { name: "Revolut", src: "/logos/revolut.svg" },
  { name: "Safe", src: "/logos/safe.svg" },
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
        <LogoDrop className="mt-auto flex justify-around gap-4 pt-8 pb-1">
          {CHAINS.map((chain) => (
            <Mark key={chain.name} src={chain.src} name={chain.name} size="lg" />
          ))}
        </LogoDrop>
      </Card>

      <Card
        eyebrow="Works with every checkout"
        title="Not another checkout."
        body="Payday issues a destination address for every deposit intent, fund-able by every hosted checkout solution, wallet, exchange or on-ramp."
      >
        <LogoDrop className="relative mt-auto h-[160px] w-full">
          {CHECKOUTS.map((logo, index) => (
            <Mark
              key={logo.name}
              src={logo.src}
              name={logo.name}
              className="absolute"
              style={resting(index, CHECKOUTS.length)}
            />
          ))}
        </LogoDrop>
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

/**
 * Where mark `index` of `count` comes to rest on the floor of the pile: a
 * spot along the width, a little height, a lean. Deterministic in the
 * index, so the server and the browser agree on every position.
 */
function resting(index: number, count: number): React.CSSProperties {
  const noise = (k: number) => (((index + 1) * 9301 + 49297 * k) % 233280) / 233280;
  // Spread across the width in a shuffled order, so neighbours in the list
  // do not land side by side.
  const slot = (index * 11) % count;
  const left = 1 + (slot / (count - 1)) * 86 + (noise(1) - 0.5) * 6;
  const bottom = Math.round(70 * noise(2) ** 1.6);
  const lean = Math.round((noise(3) - 0.5) * 44);
  return {
    left: `${left.toFixed(1)}%`,
    bottom: `${bottom}px`,
    zIndex: index,
    "--rest": `${lean}deg`,
  } as React.CSSProperties;
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
    <article className="relative flex min-h-[340px] flex-col overflow-hidden rounded-[18px] border border-brand-black/[0.14] p-6 sm:p-7">
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
