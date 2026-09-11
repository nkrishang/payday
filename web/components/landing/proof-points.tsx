import Image from "next/image";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { BuildWith } from "./build-with";

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

/**
 * Where a payer's funds already are. Marks in the first row, the wider
 * wordmarks spread across both so neither row reads as the important one.
 */
const CHECKOUTS = [
  [
    { name: "Coinbase", src: "/logos/coinbase.svg" },
    { name: "MetaMask", src: "/logos/metamask.svg" },
    { name: "Binance", src: "/logos/binance.svg" },
    { name: "MoonPay", src: "/logos/moonpay.svg", wide: true },
    { name: "Phantom", src: "/logos/phantom.svg" },
    { name: "Kraken", src: "/logos/kraken.svg" },
    { name: "fun.xyz", src: "/logos/fun.svg" },
    { name: "Rainbow", src: "/logos/rainbow.svg" },
    { name: "OKX", src: "/logos/okx.svg" },
    { name: "Stripe", src: "/logos/stripe.svg" },
    { name: "Ledger", src: "/logos/ledger.svg" },
  ],
  [
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
  ],
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
        <ul className="mt-auto grid grid-cols-3 gap-3 pt-6">
          {CHAINS.map((chain) => (
            <li
              key={chain.name}
              className="flex flex-col items-center gap-3 rounded-[14px] border border-brand-black/10 bg-white/60 py-5"
            >
              <Image src={chain.src} width={64} height={64} alt="" className="size-11" />
              <span className="text-[13px] font-medium">{chain.name}</span>
            </li>
          ))}
        </ul>
      </Card>

      <Card
        eyebrow="Works with every checkout"
        title="A deposit destination, not another checkout."
        body="Payday issues the address. Your users fund it from whatever they already use: a wallet, an exchange, an on-ramp."
      >
        <div className="landing-marquee-fade -mx-6 mt-auto grid gap-3 pt-6 sm:-mx-7">
          {CHECKOUTS.map((row, index) => (
            <Marquee key={index} logos={row} reverse={index === 1} />
          ))}
        </div>
      </Card>

      <Card
        eyebrow="Open source"
        title="Self-hostable. No sales call."
        body="Clone the repository, run it on your own infrastructure, and read every line that touches a deposit."
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
  body: string;
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

type Logo = { name: string; src: string; wide?: boolean };

/**
 * A row of logos that drifts sideways forever. The list is laid twice, end
 * to end, and the track slides by exactly one copy before it snaps back,
 * which the eye never catches. The second copy is hidden from assistive
 * tech, so a screen reader hears each name once.
 */
function Marquee({ logos, reverse }: { logos: readonly Logo[]; reverse?: boolean }) {
  return (
    <div className="overflow-hidden">
      <ul className={cn("landing-marquee flex w-max gap-3", reverse && "landing-marquee-reverse")}>
        {[false, true].map((copy) =>
          logos.map((logo) => (
            <li
              key={`${logo.name}-${copy ? "b" : "a"}`}
              title={logo.name}
              aria-hidden={copy || undefined}
              className="flex h-14 shrink-0 items-center justify-center rounded-[12px] border border-brand-black/10 bg-white/60 px-4"
            >
              <Image
                src={logo.src}
                width={96}
                height={28}
                alt={copy ? "" : logo.name}
                className={cn("w-auto", logo.wide ? "h-6" : "h-7")}
              />
            </li>
          )),
        )}
      </ul>
    </div>
  );
}
