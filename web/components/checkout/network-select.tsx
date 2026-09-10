"use client";

import type { Network } from "@payday/sdk";
import { Check } from "lucide-react";
import { cn } from "@/lib/cn";
import { chainById } from "@/lib/config";

/**
 * The network step: the payer chooses which chain they will pay on. The
 * choice is committed into the one-time address like the wallet is (the API
 * derives the address from the signature the wallet makes under that
 * chain's domain), so it comes before the signature and cannot change after
 * it. The list is the deposit request's own `networks`; a chain the browser
 * is not configured for is shown but cannot be chosen here.
 */
export function NetworkSelect({
  networks,
  selected,
  onSelect,
}: {
  networks: Network[];
  /** The chosen `chain.id`, or null while the payer has not picked one. */
  selected: string | null;
  onSelect: (chainId: string) => void;
}) {
  return (
    <fieldset>
      <legend className="text-[11px] font-medium tracking-[0.14em] text-faint uppercase">
        Pay on
      </legend>
      <div className="mt-2 grid gap-2" role="radiogroup" aria-label="Network to pay on">
        {networks.map((network) => {
          const configured = chainById(network.chain.id);
          const checked = selected === network.chain.id;
          return (
            <button
              key={network.chain.id}
              type="button"
              role="radio"
              aria-checked={checked}
              disabled={configured === null}
              onClick={() => onSelect(network.chain.id)}
              className={cn(
                "flex w-full items-center justify-between gap-3 rounded-[10px] border px-3.5 py-3 text-left transition-colors",
                checked
                  ? "border-ink bg-raised"
                  : "border-line hover:border-muted disabled:cursor-not-allowed disabled:opacity-60",
              )}
            >
              <span className="min-w-0">
                <span className="block text-[14px] font-medium">{network.chain.name}</span>
                <span className="mt-0.5 block text-[12px] text-faint">
                  {network.token.symbol} · gas in {network.chain.native_symbol}
                  {configured ? ` · ${configured.confirmation}` : " · not available in this checkout"}
                </span>
              </span>
              <span
                aria-hidden
                className={cn(
                  "flex size-5 shrink-0 items-center justify-center rounded-full border",
                  checked ? "border-ink bg-ink text-surface" : "border-line",
                )}
              >
                {checked ? <Check className="size-3" /> : null}
              </span>
            </button>
          );
        })}
      </div>
    </fieldset>
  );
}
