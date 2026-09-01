import { defineChain } from "viem";
import { createConfig, http } from "wagmi";
import { injected, walletConnect } from "wagmi/connectors";
import { config } from "./config";

/**
 * A deployment serves exactly one chain and one USDC contract — gatewayd
 * rejects any other `chain_id` or `token_address` with 422 — so the wallet
 * stack is configured once from env rather than discovered per payment. The
 * checkout still verifies the payment's chain and token against these values
 * before it will let anyone sign.
 */
export const paydayChain = defineChain({
  id: config.chainId,
  name: config.chainName,
  nativeCurrency: { name: config.nativeSymbol, symbol: config.nativeSymbol, decimals: 18 },
  rpcUrls: { default: { http: [config.rpcUrl] } },
  ...(config.explorerUrl
    ? {
        blockExplorers: {
          default: { name: `${config.chainName} explorer`, url: config.explorerUrl },
        },
      }
    : {}),
  testnet: config.chainId !== 1 && config.chainId !== 143,
});

export const hasWalletConnect = config.walletConnectProjectId !== null;

/**
 * Browser extensions arrive on their own: wagmi's EIP-6963 discovery turns each
 * announced provider into its own connector, so the connect sheet lists exactly
 * what the payer actually has installed. WalletConnect is added when a project
 * id is configured, and covers phone wallets through its own QR and deep links.
 */
export const wagmiConfig = createConfig({
  chains: [paydayChain],
  transports: { [paydayChain.id]: http(config.rpcUrl) },
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
              description: "Stablecoin payments that settle themselves",
              url: "https://payday.sh",
              icons: [],
            },
          }),
        ]
      : []),
  ],
});
