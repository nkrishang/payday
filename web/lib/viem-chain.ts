import { defineChain, type Chain } from "viem";
import { config, type PublicChain } from "./config";

/**
 * A configured chain as viem (and so wagmi and Privy) describe one. Kept
 * apart from `chain.ts` so the dashboard, which signs typed data through
 * Privy and never connects an external wallet, does not pull the wagmi
 * connectors in with it.
 */
export function toViemChain(chain: PublicChain): Chain {
  return defineChain({
    id: chain.id,
    name: chain.name,
    nativeCurrency: { name: chain.nativeSymbol, symbol: chain.nativeSymbol, decimals: 18 },
    rpcUrls: { default: { http: [chain.rpcUrl] } },
    ...(chain.explorerUrl
      ? {
          blockExplorers: {
            default: { name: `${chain.name} explorer`, url: chain.explorerUrl },
          },
        }
      : {}),
    testnet: ![1, 143, 8453, 42161].includes(chain.id),
  });
}

/** Every configured chain, in the order the deployment lists them. */
export const viemChains = config.chains.map(toViemChain) as [Chain, ...Chain[]];
