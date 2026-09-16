import Image from "next/image";
import type { CSSProperties } from "react";

/**
 * The chains a deposit can arrive from and settle on, as a tray of tiles
 * sliding past. Each tile holds the chain's mark as a grey halftone until the
 * pointer arrives, when it lights up in colour (the `.landing-chain` rules).
 * The strip is drawn twice so the slide never runs out; the second copy is
 * hidden from assistive technology, which reads the first one as a list.
 */
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

export function ChainGrid() {
  return (
    <div className="h-full w-full overflow-hidden [mask-image:linear-gradient(to_right,transparent,#000_5%,#000_95%,transparent)]">
      <div className="landing-marquee items-center py-3">
        {[0, 1].map((copy) => (
          <ul
            key={copy}
            aria-hidden={copy === 1 || undefined}
            aria-label={copy === 0 ? "Chains" : undefined}
            className="mr-4 grid shrink-0 grid-flow-col grid-rows-2 gap-px border border-gum-grey/30 bg-gum-grey/30"
          >
            {CHAINS.map((chain, index) => (
              <li
                key={chain.name}
                title={chain.name}
                data-reveal=""
                style={{ "--i": index, "--step": "28ms" } as CSSProperties}
                className="landing-chain relative size-[60px] bg-gum-white sm:size-[66px]"
              >
                {/* The mark as a grey halftone, until the pointer arrives. */}
                <span
                  aria-hidden="true"
                  className="landing-chain-halftone"
                  style={{ "--logo": `url(${chain.src})` } as CSSProperties}
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
        ))}
      </div>
    </div>
  );
}
