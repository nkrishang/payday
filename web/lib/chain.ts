import { defineChain, type Chain } from "viem";
import { createConfig, http } from "wagmi";
import { injected, walletConnect } from "wagmi/connectors";
import { config, type PublicChain } from "./config";

/**
 * A deployment serves several chains, each with its own USDC contract; the
 * payer picks one when they sign, and gatewayd rejects a challenge for any
 * other. The wallet stack is configured once from env with every one of
 * them, so the wallet can be switched to whichever the payer chose. The
 * checkout still verifies a deposit request's chain and token against these
 * values before it will let anyone sign or pay.
 */
function toViemChain(chain: PublicChain): Chain {
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

const viemChains = config.chains.map(toViemChain);

/** The wagmi chain behind an API `chain.id`, or null when this deployment does not offer it. */
export function wagmiChain(id: string | number | null | undefined): Chain | null {
  if (id === null || id === undefined) return null;
  const wanted = typeof id === "number" ? id : Number(id);
  return viemChains.find((chain) => chain.id === wanted) ?? null;
}

export const hasWalletConnect = config.walletConnectProjectId !== null;

/**
 * Browser extensions arrive on their own: wagmi's EIP-6963 discovery turns each
 * announced provider into its own connector, so the connect sheet lists exactly
 * what the payer actually has installed. WalletConnect is added when a project
 * id is configured, and covers phone wallets through its own QR and deep links.
 */
export const wagmiConfig = createConfig({
  chains: viemChains as [Chain, ...Chain[]],
  transports: Object.fromEntries(
    config.chains.map((chain) => [chain.id, http(chain.rpcUrl)]),
  ),
  ssr: true,
  connectors: [
    injected({ shimDisconnect: true }),
    ...(config.walletConnectProjectId
      ? [
          walletConnect({
            projectId: config.walletConnectProjectId,
            showQrModal: true,
            metadata: {
              name: "Payday",
              description: "Stablecoin deposits that settle themselves",
              url: "https://payday.sh",
              icons: [],
            },
          }),
        ]
      : []),
  ],
});
