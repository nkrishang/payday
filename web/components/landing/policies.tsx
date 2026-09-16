import Image from "next/image";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import { CHAINS, LogoGrid } from "./chain-grid";
import { Countdown } from "./countdown";

/**
 * What a deposit address can be told: who may fund it, how long it lives,
 * where it settles, what it takes. Four cells on the page's rule, each
 * leading with the control rather than a claim about it.
 */
export function Policies() {
  return (
    <div className="grid gap-px border border-gum-grey/30 bg-gum-grey/30 sm:grid-cols-2 xl:grid-cols-4">
      <Cell
        index={0}
        title="Who can fund it"
        body="Permissionless, a verified email, or a user signed in to your app."
      >
        <ul className="grid gap-1.5 text-[13px]">
          <Option>Permissionless</Option>
          <Option>
            A verified email <span className="font-mono text-gum-grey">jordan@acme.co</span>
          </Option>
          <Option selected>
            A signed-in user <span className="font-mono text-gum-white/80">user_8f21</span>
          </Option>
        </ul>
      </Cell>

      <Cell
        index={1}
        title="When it expires"
        body="Control the time window in which the address accepts deposits."
      >
        <p className="text-[44px] leading-none font-semibold tracking-tight">
          <Countdown />
        </p>
      </Cell>

      <Cell
        index={2}
        title="Where it settles"
        body="Let users pay on any chain. Settle funds on the chain you want."
      >
        <LogoGrid logos={CHAINS} columns={5} label="Chains" className="grid-rows-2" />
      </Cell>

      <Cell
        index={3}
        title="What it accepts"
        body="Accept deposits in the right currency for your app."
      >
        <ul className="flex items-center gap-3">
          {(
            [
              ["USDC", "/payment-icons/usdc.svg", true],
              ["USDT", "/payment-icons/usdt.svg", false],
              ["AUSD", "/payment-icons/ausd.svg", false],
            ] as const
          ).map(([name, src, selected]) => (
            <li
              key={name}
              className={cn(
                "flex items-center gap-2 rounded-full border py-1 pr-3 pl-1 text-[13px] font-medium transition-colors",
                selected ? "border-gum-pink bg-gum-pink text-gum-white" : "border-gum-grey/30",
              )}
            >
              <Image src={src} width={64} height={64} alt="" className="size-6 rounded-full" />
              {name}
            </li>
          ))}
        </ul>
      </Cell>
    </div>
  );
}

function Cell({
  index,
  title,
  body,
  children,
}: {
  index: number;
  title: string;
  body: string;
  children: ReactNode;
}) {
  return (
    <article
      data-reveal=""
      style={{ "--i": index } as React.CSSProperties}
      className="flex min-h-[300px] flex-col bg-gum-white p-6 sm:p-7"
    >
      <h3 className="text-[20px] leading-snug font-medium tracking-[-0.02em]">{title}</h3>
      <p className="mt-2 text-[14px] leading-relaxed text-gum-grey">{body}</p>
      <div className="mt-auto pt-8">{children}</div>
    </article>
  );
}

function Option({ selected, children }: { selected?: boolean; children: ReactNode }) {
  return (
    <li
      className={cn(
        "flex items-center gap-2.5 rounded-[8px] border px-3 py-2",
        selected ? "border-gum-pink bg-gum-pink text-gum-white" : "border-gum-grey/30",
      )}
    >
      <span
        className={cn(
          "size-3 shrink-0 rounded-full border-2",
          selected ? "border-gum-white bg-gum-white" : "border-gum-grey/50",
        )}
      />
      <span className="flex flex-wrap gap-x-1.5">{children}</span>
    </li>
  );
}
