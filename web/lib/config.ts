/**
 * Public browser configuration.
 *
 * Every value here ships to the client, so nothing secret belongs in it. In
 * particular `rpcUrl` must be a public endpoint — never the operator RPC that
 * gatewayd reads from Secrets Manager.
 *
 * `process.env.NEXT_PUBLIC_*` must be read through literal member access for
 * Next to inline it at build time; do not refactor these into a lookup.
 */

function required(value: string | undefined, name: string): string {
  if (!value) {
    throw new Error(
      `${name} is not set. Copy web/.env.example to web/.env.local and fill it in.`,
    );
  }
  return value;
}

function trimTrailingSlash(value: string): string {
  return value.replace(/\/+$/, "");
}

/** Monad and its testnet pay gas in MON; everything else we run is ETH-denominated. */
function defaultNativeSymbol(chainId: number): string {
  return chainId === 143 || chainId === 10143 ? "MON" : "ETH";
}

const chainId = Number.parseInt(
  required(process.env.NEXT_PUBLIC_CHAIN_ID, "NEXT_PUBLIC_CHAIN_ID"),
  10,
);

if (!Number.isSafeInteger(chainId) || chainId <= 0) {
  throw new Error("NEXT_PUBLIC_CHAIN_ID must be a positive integer");
}

const explorerUrl = process.env.NEXT_PUBLIC_EXPLORER_BASE_URL;
const walletConnectProjectId = process.env.NEXT_PUBLIC_WALLETCONNECT_PROJECT_ID;

export const config = {
  /** Origin of the Payday API, e.g. https://api.payday.sh. */
  apiUrl: trimTrailingSlash(
    required(process.env.NEXT_PUBLIC_PAYDAY_API_URL, "NEXT_PUBLIC_PAYDAY_API_URL"),
  ),
  chainId,
  chainName: required(process.env.NEXT_PUBLIC_CHAIN_NAME, "NEXT_PUBLIC_CHAIN_NAME"),
  rpcUrl: required(process.env.NEXT_PUBLIC_RPC_URL, "NEXT_PUBLIC_RPC_URL"),
  /** The deployment's exact Circle-issued native USDC contract, lowercased for comparison. */
  usdcAddress: required(
    process.env.NEXT_PUBLIC_USDC_ADDRESS,
    "NEXT_PUBLIC_USDC_ADDRESS",
  ).toLowerCase(),
  explorerUrl: explorerUrl ? trimTrailingSlash(explorerUrl) : null,
  walletConnectProjectId: walletConnectProjectId || null,
  nativeSymbol: process.env.NEXT_PUBLIC_NATIVE_SYMBOL || defaultNativeSymbol(chainId),
} as const;

export type PublicConfig = typeof config;
