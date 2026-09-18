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
 * One stablecoin the deployment serves on a chain: the issuer's exact contract
 * (Circle's native USDC; Tether's USDT0 on Monad and Arbitrum), which the pay
 * button refuses to sign a transfer for anything but.
 */
export interface PublicToken {
  /** `USDC` or `USDT`: the currency a request is priced in. */
  currency: string;
  /** The contract's own symbol, as a wallet shows it (`USDT0` on Monad). */
  symbol: string;
  /** Lowercased for comparison. */
  address: string;
  decimals: number;
}

/**
 * One network the deployment offers. The API is the authority on which
 * networks a deposit request can be paid on and what each is called; this
 * carries only what the browser needs on its own: a public RPC for wallet
 * reads, the explorer, the gas token, and the exact stablecoin contracts
 * served there.
 */
export interface PublicChain {
  id: number;
  name: string;
  rpcUrl: string;
  explorerUrl: string | null;
  nativeSymbol: string;
  /** The stablecoins served on this chain, in the order the dashboard offers them. */
  tokens: PublicToken[];
  /** How long a deposit typically takes to be credited, shown next to the choice. */
  confirmation: string;
  /**
   * Circle's CCTP on this chain and the WithdrawalForwarder deployed against
   * it, when the chain can bridge; null on a chain that only supports
   * same-chain withdrawals (local Anvil). Shown to a merchant so they can
   * check the payee their bridge authorization names.
   */
  cctp: PublicCctp | null;
}

export interface PublicCctp {
  /** Circle's domain id (Arbitrum 3, Base 6, Monad 15). */
  domain: number;
  forwarder: string;
  tokenMessenger: string;
  messageTransmitter: string;
}

/**
 * `NEXT_PUBLIC_CHAINS`: a JSON array of `{id, name, rpcUrl, tokens,
 * explorerUrl?, nativeSymbol?, confirmation?, cctp?}`, in the order the
 * checkout offers the networks, where `tokens` is `[{currency, address,
 * symbol?, decimals?}]`. Must list the same chains and contracts as
 * gatewayd's PAYDAY_CHAINS.
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
    const cctp = parseCctp(item.cctp, index);
    const tokens = parseTokens(item.tokens, id, index);
    if (cctp && !tokens.some((token) => token.currency === "USDC")) {
      throw new Error(`NEXT_PUBLIC_CHAINS[${index}].cctp needs USDC on the chain; CCTP bridges USDC alone`);
    }
    return {
      cctp,
      id,
      name: text("name"),
      rpcUrl: text("rpcUrl"),
      tokens,
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

/** `tokens: [{currency, address, symbol?, decimals?}]`: at least one, no currency twice. */
function parseTokens(raw: unknown, chainId: number, index: number): PublicToken[] {
  if (!Array.isArray(raw) || raw.length === 0) {
    throw new Error(`NEXT_PUBLIC_CHAINS[${index}].tokens must list at least one stablecoin`);
  }
  const tokens = raw.map((entry, position) => {
    const item = entry as Record<string, unknown>;
    const where = `NEXT_PUBLIC_CHAINS[${index}].tokens[${position}]`;
    const currency = item.currency;
    if (currency !== "USDC" && currency !== "USDT") {
      throw new Error(`${where}.currency must be USDC or USDT`);
    }
    const address = item.address;
    if (typeof address !== "string" || !/^0x[0-9a-fA-F]{40}$/.test(address)) {
      throw new Error(`${where}.address must be an EVM address`);
    }
    const decimals = item.decimals === undefined ? 6 : Number(item.decimals);
    if (!Number.isSafeInteger(decimals) || decimals < 0 || decimals > 255) {
      throw new Error(`${where}.decimals must be an integer`);
    }
    return {
      currency,
      symbol:
        typeof item.symbol === "string" && item.symbol ? item.symbol : defaultSymbol(currency, chainId),
      address: address.toLowerCase(),
      decimals,
    };
  });
  const currencies = new Set(tokens.map((token) => token.currency));
  if (currencies.size !== tokens.length) {
    throw new Error(`NEXT_PUBLIC_CHAINS[${index}].tokens lists a currency twice`);
  }
  return tokens;
}

/** Tether's USDT on Monad and Arbitrum is the omnichain USDT0; elsewhere a contract shows its currency's code. */
function defaultSymbol(currency: string, chainId: number): string {
  return currency === "USDT" && (chainId === 143 || chainId === 42161) ? "USDT0" : currency;
}

/** `cctp: {domain, forwarder, tokenMessenger, messageTransmitter}`, optional per chain. */
function parseCctp(raw: unknown, index: number): PublicCctp | null {
  if (raw === undefined || raw === null) return null;
  const item = raw as Record<string, unknown>;
  const domain = Number(item.domain);
  if (!Number.isSafeInteger(domain) || domain < 0) {
    throw new Error(`NEXT_PUBLIC_CHAINS[${index}].cctp.domain must be a non-negative integer`);
  }
  const address = (key: string): string => {
    const value = item[key];
    if (typeof value !== "string" || !/^0x[0-9a-fA-F]{40}$/.test(value)) {
      throw new Error(`NEXT_PUBLIC_CHAINS[${index}].cctp.${key} must be an EVM address`);
    }
    return value;
  };
  return {
    domain,
    forwarder: address("forwarder"),
    tokenMessenger: address("tokenMessenger"),
    messageTransmitter: address("messageTransmitter"),
  };
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
  payerAppealEmail: payerAppealEmail || "support@gum.money",
} as const;

export type PublicConfig = typeof config;

/** Circle's per-message CCTP burn ceiling, in whole USDC display units. */
export const CCTP_BURN_LIMIT_USDC = 10_000_000n;

/** The configured chain behind an API `chain.id` (a decimal string) or a numeric id. */
export function chainById(id: string | number | null | undefined): PublicChain | null {
  if (id === null || id === undefined) return null;
  const wanted = typeof id === "number" ? id : Number(id);
  return chains.find((chain) => chain.id === wanted) ?? null;
}

/** The configured stablecoin behind a contract address on a chain, if the deployment serves it there. */
export function tokenFor(chainId: string | number | null | undefined, address: string | null | undefined): PublicToken | null {
  if (!address) return null;
  const wanted = address.toLowerCase();
  return chainById(chainId)?.tokens.find((token) => token.address === wanted) ?? null;
}

/** The chain's contract for a currency, if it serves it. */
export function tokenOn(chain: PublicChain, currency: string): PublicToken | null {
  return chain.tokens.find((token) => token.currency === currency) ?? null;
}

/** The chains serving a currency, in configured order. */
export function chainsFor(currency: string): PublicChain[] {
  return chains.filter((chain) => tokenOn(chain, currency) !== null);
}

/** Every currency at least one configured chain serves, in code order. */
export function currencies(): string[] {
  return ["USDC", "USDT"].filter((currency) => chainsFor(currency).length > 0);
}

/** The currencies that bridge between chains for the merchant at 1:1: USDC through CCTP. A request in any other must pin its chain and withdraws on that chain alone. */
export function bridges(currency: string): boolean {
  return currency === "USDC";
}
