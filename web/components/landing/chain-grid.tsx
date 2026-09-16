import Image from "next/image";
import type { CSSProperties } from "react";
import { cn } from "@/lib/cn";

/**
 * A grid of marks, each a grey halftone until the pointer arrives, when it
 * lights up in colour (the `.landing-chain` rules). The grid fills whatever
 * it is given: the tiles stretch, and the mark sits in the middle of each.
 */
export function LogoGrid({
  logos,
  columns,
  active,
  label,
  className,
}: {
  logos: readonly { name: string; src: string }[];
  columns: number;
  /** The one tile lit without the pointer, if any. */
  active?: string | undefined;
  label: string;
  className?: string;
}) {
  return (
    <ul
      aria-label={label}
      className={cn("grid gap-px border border-gum-grey/30 bg-gum-grey/30", className)}
      style={{ gridTemplateColumns: `repeat(${columns}, minmax(0, 1fr))` }}
    >
      {logos.map((logo, index) => (
        <li
          key={logo.name}
          title={logo.name}
          data-reveal=""
          data-active={logo.name === active || undefined}
          style={{ "--i": index, "--step": "28ms" } as CSSProperties}
          className="landing-chain relative min-h-[56px] bg-gum-white"
        >
          {/* The mark as a grey halftone, until it is lit. */}
          <span
            aria-hidden="true"
            className="landing-chain-halftone"
            style={{ "--logo": `url(${logo.src})` } as CSSProperties}
          />
          <Image
            src={logo.src}
            width={64}
            height={64}
            alt={logo.name}
            className="landing-chain-logo"
          />
        </li>
      ))}
    </ul>
  );
}

/** The chains a deposit can arrive from and settle on. */
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
    <LogoGrid
      logos={CHAINS}
      columns={5}
      label="Chains"
      className="h-full grid-rows-2 border-0 max-lg:aspect-[5/2]"
    />
  );
}
