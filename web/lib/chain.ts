import type { Chain } from "viem";
import { createConfig, http } from "wagmi";
import { injected, walletConnect } from "wagmi/connectors";
import { config } from "./config";
import { viemChains } from "./viem-chain";

/**
 * A deployment serves several chains, each with its own USDC contract; the
 * payer picks one when they sign, and gatewayd rejects a challenge for any
 * other. The wallet stack is configured once from env with every one of
 * them (`viem-chain.ts`), so the wallet can be switched to whichever the
 * payer chose. The checkout still verifies a deposit request's chain and
 * token against these values before it will let anyone sign or pay.
 */

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
              name: "Gum",
              description: "Stablecoin deposits that stick",
              url: "https://gum.money",
              icons: [],
            },
          }),
        ]
      : []),
  ],
});
