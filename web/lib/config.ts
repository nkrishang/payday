/**
 * Public browser configuration.
 *
 * Every value here ships to the client, so nothing secret belongs in it. In
 * particular every `rpcUrl` must be a public endpoint — never the operator RPC
 * that gatewayd reads from Secrets Manager.
 *
 * `process.env.NEXT_PUBLIC_*` must be read through literal member access for
 * Next to inline it at build time; do not refactor these into a lookup.
 */

function required(value: string | undefined, name: string): string {
  if (!value) {
    throw new Error(`${name} is not set. Copy web/.env.example to web/.env.local and fill it in.`);
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

/**
 * One network the deployment offers. The API is the authority on which
 * networks a deposit request can be paid on and what each is called; this
 * carries only what the browser needs on its own: a public RPC for wallet
 * reads, the explorer, the gas token, and the exact USDC contract the pay
 * button refuses to sign a transfer for anything but.
 */
export interface PublicChain {
  id: number;
  name: string;
  rpcUrl: string;
  explorerUrl: string | null;
  nativeSymbol: string;
  /** The chain's exact Circle-issued native USDC contract, lowercased for comparison. */
  usdcAddress: string;
  /** How long a deposit typically takes to be credited, shown next to the choice. */
  confirmation: string;
}

/**
 * `NEXT_PUBLIC_CHAINS`: a JSON array of `{id, name, rpcUrl, usdcAddress,
 * explorerUrl?, nativeSymbol?, confirmation?}`, in the order the checkout
 * offers the networks. Must list the same chains as gatewayd's PAYDAY_CHAINS.
 */
function parseChains(raw: string): PublicChain[] {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error("NEXT_PUBLIC_CHAINS must be a JSON array");
  }
  if (!Array.isArray(parsed) || parsed.length === 0) {
    throw new Error("NEXT_PUBLIC_CHAINS must list at least one chain");
  }
  return parsed.map((entry, index) => {
    const item = entry as Record<string, unknown>;
    const id = Number(item.id);
    if (!Number.isSafeInteger(id) || id <= 0) {
      throw new Error(`NEXT_PUBLIC_CHAINS[${index}].id must be a positive integer`);
    }
    const text = (key: string): string => {
      const value = item[key];
      if (typeof value !== "string" || !value) {
        throw new Error(`NEXT_PUBLIC_CHAINS[${index}].${key} must be a non-empty string`);
      }
      return value;
    };
    const explorer = typeof item.explorerUrl === "string" && item.explorerUrl ? item.explorerUrl : null;
    return {
      id,
      name: text("name"),
      rpcUrl: text("rpcUrl"),
      usdcAddress: text("usdcAddress").toLowerCase(),
      explorerUrl: explorer ? trimTrailingSlash(explorer) : null,
      nativeSymbol:
        typeof item.nativeSymbol === "string" && item.nativeSymbol
          ? item.nativeSymbol
          : defaultNativeSymbol(id),
      confirmation:
        typeof item.confirmation === "string" && item.confirmation
          ? item.confirmation
          : "Credited within a minute",
    };
  });
}

const chains = parseChains(required(process.env.NEXT_PUBLIC_CHAINS, "NEXT_PUBLIC_CHAINS"));

const walletConnectProjectId = process.env.NEXT_PUBLIC_WALLETCONNECT_PROJECT_ID;
const attachmentUploadOrigin = process.env.NEXT_PUBLIC_ATTACHMENT_UPLOAD_ORIGIN;
const payerAppealEmail = process.env.NEXT_PUBLIC_PAYER_APPEAL_EMAIL;

export const config = {
  /** Origin of the Payday API, e.g. https://api.payday.sh. */
  apiUrl: trimTrailingSlash(
    required(process.env.NEXT_PUBLIC_PAYDAY_API_URL, "NEXT_PUBLIC_PAYDAY_API_URL"),
  ),
  /** The networks this deployment offers, in the order the checkout lists them. */
  chains,
  walletConnectProjectId: walletConnectProjectId || null,
  /**
   * Origin the dashboard PUTs attachment bytes to (the presigned upload URL's
   * host), so the page's CSP can admit it. Null when uploads share the API
   * origin, as they do against the e2e stub.
   */
  attachmentUploadOrigin: attachmentUploadOrigin ? trimTrailingSlash(attachmentUploadOrigin) : null,
  /** Where a payer whose identity check was declined writes to appeal. */
  payerAppealEmail: payerAppealEmail || "support@payday.sh",
} as const;

export type PublicConfig = typeof config;

/** The configured chain behind an API `chain.id` (a decimal string) or a numeric id. */
export function chainById(id: string | number | null | undefined): PublicChain | null {
  if (id === null || id === undefined) return null;
  const wanted = typeof id === "number" ? id : Number(id);
  return chains.find((chain) => chain.id === wanted) ?? null;
}
