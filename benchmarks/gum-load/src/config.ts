/**
 * Configuration: a versioned JSON file plus environment secrets plus explicit
 * CLI flags. CLI flags may narrow but never widen approved limits. Secrets
 * travel only through the environment; they are never written to the
 * manifest or report.
 */

import { readFileSync } from "node:fs";
import { LOCAL_CHAINS, REAL_CHAIN_DEFAULTS, type ChainConfig } from "./chains.js";
import { decimalToUnits, type DisplayMode, type Experiment, type Profile } from "./model.js";

export const CONFIG_VERSION = 1;

export interface Limits {
  /** Maximum deposit amount per request, decimal string. */
  maxAmount: string;
  /** Maximum gross stablecoin volume per run, decimal string. */
  maxGrossVolume: string;
  /** Maximum simultaneously funded-but-unsettled principal, decimal string. */
  maxOutstandingPrincipal: string;
  /** Maximum concurrently funded (broadcast, unsettled) lifecycles. */
  maxConcurrentFunded: number;
  /** Maximum deposit requests admitted per run, including warm-up and repeats. */
  maxRequests: number;
  /** Maximum native gas spend per chain per run, whole native units, decimal string. */
  maxNativeSpend: string;
  /** Abort admission when the oldest funded-unsettled lifecycle exceeds this age, seconds. */
  maxOldestUnsettledSeconds: number;
  /** Per-lifecycle observation timeout, seconds, after which admission stops in real mode. */
  perOpTimeoutSeconds: number;
  /** Fraction of the amount reserved as failure headroom, e.g. 0.25. */
  reserveFraction: number;
}

export interface HarnessConfig {
  version: number;
  mode: "local" | "real";
  /** Real mode only: which deployment the approval covers. */
  target?: "staging" | "production";
  api: { url: string };
  chains: Record<string, ChainConfig>;
  currency: "USDC" | "USDT";
  amount: string;
  payout: { mode: "payer_wallet" } | { mode: "fixed"; address: string };
  wallets: { count: number; /** Env var naming an HD mnemonic; local mode generates one when absent. */ seedEnvVar: string };
  webhook: { enabled: boolean; listenPort: number; /** Public HTTPS URL the API can reach; local needs a tunnel. */ publicUrl?: string };
  observation: { pollMs: number; apiPollMs: number; logRangeBlocks: number };
  limits: Limits;
  /** Origin sent on payer writes, matching the deployment's hosted checkout. */
  checkoutOrigin: string;
}

export interface CliArgs {
  config?: string;
  mode?: "local" | "real";
  target?: "staging" | "production";
  experiment?: Experiment;
  profile?: Profile;
  chain?: string;
  currency?: string;
  amount?: string;
  count?: number;
  rate?: number;
  duration?: string;
  warmup?: number;
  repeat?: number;
  webhook?: string;
  display?: DisplayMode;
  fundLocal?: boolean;
  runDir?: string;
  seed?: string;
  timeoutSeconds?: number;
  jitter?: boolean;
}

const REAL_API_ORIGINS: Record<string, string> = {
  staging: "https://api.staging.gum.money",
  production: "https://api.gum.money",
};

const CHECKOUT_ORIGINS: Record<string, string> = {
  local: "http://127.0.0.1:3002",
  staging: "http://127.0.0.1:3002",
  production: "https://gum.money",
};

export function defaultLimits(mode: "local" | "real"): Limits {
  if (mode === "local") {
    return {
      maxAmount: "1000", maxGrossVolume: "100000", maxOutstandingPrincipal: "10000",
      maxConcurrentFunded: 256, maxRequests: 100000, maxNativeSpend: "10",
      maxOldestUnsettledSeconds: 600, perOpTimeoutSeconds: 300, reserveFraction: 0.25,
    };
  }
  return {
    maxAmount: "0.01", maxGrossVolume: "1", maxOutstandingPrincipal: "1",
    maxConcurrentFunded: 2, maxRequests: 30, maxNativeSpend: "0.01",
    maxOldestUnsettledSeconds: 900, perOpTimeoutSeconds: 1800, reserveFraction: 1,
  };
}

export function loadConfigFile(path: string): Partial<HarnessConfig> {
  const raw = JSON.parse(readFileSync(path, "utf8")) as Record<string, unknown>;
  if (raw["version"] !== CONFIG_VERSION) {
    throw new Error(`config ${path} has version ${String(raw["version"])}; expected ${CONFIG_VERSION}`);
  }
  return raw as Partial<HarnessConfig>;
}

/**
 * Resolve the effective configuration. Precedence: CLI flags > config file >
 * defaults. Real mode requires explicit target, API origin, chain, and
 * workload size; it inherits no production default from anywhere.
 */
export function resolveConfig(args: CliArgs): HarnessConfig {
  const file = args.config ? loadConfigFile(args.config) : {};
  const mode = args.mode ?? file.mode;
  if (mode !== "local" && mode !== "real") {
    throw new Error("mode is required: --mode local or --mode real");
  }
  const target = args.target ?? file.target;
  const apiFromEnv = process.env["GUM_API_URL"];
  let api: { url: string };
  if (mode === "local") {
    api = { url: apiFromEnv ?? "http://127.0.0.1:3000" };
  } else {
    if (!target) throw new Error("real mode requires --target staging|production");
    const approved = REAL_API_ORIGINS[target]!;
    api = { url: apiFromEnv ?? REAL_API_ORIGINS[target]! };
    if (!apiFromEnv && !args.config) {
      // Only an explicit config file or env var may pin a nonstandard origin.
      api = { url: REAL_API_ORIGINS[target]! };
    }
    // The effective origin must be the approved target's origin: a staging
    // approval must never be able to fire authenticated traffic elsewhere.
    if (api.url !== approved) {
      throw new Error(`real mode: API origin ${api.url} does not match the approved ${target} origin ${approved}; use --target ${target} without overriding GUM_API_URL/api.url`);
    }
  }

  const chains: Record<string, ChainConfig> = {};
  const fileChains = file.chains ?? {};
  if (mode === "local") {
    for (const [id, chain] of Object.entries(LOCAL_CHAINS)) {
      chains[id] = {
        ...chain,
        rpcUrl: process.env[`GUM_RPC_URL_${id}`] ?? fileChains[id]?.rpcUrl ?? chain.rpcUrl,
        tokens: { ...chain.tokens, ...fileChains[id]?.tokens },
      };
    }
  } else {
    const requested = args.chain ?? Object.keys(REAL_CHAIN_DEFAULTS)[0]!;
    const base = REAL_CHAIN_DEFAULTS[requested];
    if (!base) throw new Error(`unknown real chain ${requested}; supported: ${Object.keys(REAL_CHAIN_DEFAULTS).join(", ")}`);
    const rpcUrl = process.env[`GUM_RPC_URL_${requested}`] ?? fileChains[requested]?.rpcUrl;
    if (!rpcUrl) throw new Error(`real mode requires GUM_RPC_URL_${requested} (or chains.${requested}.rpcUrl in the config file)`);
    chains[requested] = {
      ...base,
      rpcUrl,
      tokens: { ...base.tokens, ...fileChains[requested]?.tokens },
      finality: fileChains[requested]?.finality ?? base.finality,
      explorerUrl: fileChains[requested]?.explorerUrl,
    };
  }

  const currency = ((args.currency ?? file.currency ?? "USDC").toUpperCase() === "USDT" ? "USDT" : "USDC") as "USDC" | "USDT";
  const chainId = args.chain ?? (mode === "local" ? "31337" : args.target ? requestedChainId(chains) : undefined);
  if (!chainId) throw new Error("chain is required in real mode: --chain 143|8453|42161");
  if (!chains[chainId]) throw new Error(`chain ${chainId} is not configured in this mode`);

  const defaults = defaultLimits(mode);
  const limits: Limits = { ...defaults, ...(file.limits ?? {}) };
  applyCliLimitNarrowing(limits, args, mode);

  const amount = args.amount ?? file.amount ?? (mode === "local" ? "0.01" : defaults.maxAmount);
  const webhook = {
    enabled: args.webhook !== undefined ? true : file.webhook?.enabled ?? false,
    listenPort: file.webhook?.listenPort ?? 8819,
    publicUrl: args.webhook ?? file.webhook?.publicUrl,
  };

  return {
    version: CONFIG_VERSION,
    mode,
    ...(target ? { target } : {}),
    api,
    chains,
    currency,
    amount,
    payout: file.payout ?? { mode: "payer_wallet" },
    wallets: { count: file.wallets?.count ?? 8, seedEnvVar: file.wallets?.seedEnvVar ?? "GUM_LOAD_PAYER_SEED" },
    webhook,
    observation: {
      pollMs: file.observation?.pollMs ?? (mode === "local" ? 500 : 2000),
      apiPollMs: file.observation?.apiPollMs ?? (mode === "local" ? 1000 : 3000),
      logRangeBlocks: file.observation?.logRangeBlocks ?? 500,
    },
    limits,
    checkoutOrigin: file.checkoutOrigin ?? CHECKOUT_ORIGINS[mode === "local" ? "local" : target ?? "production"],
  };
}

function requestedChainId(chains: Record<string, ChainConfig>): string {
  const ids = Object.keys(chains);
  if (ids.length !== 1) throw new Error("exactly one real chain must be configured per run");
  return ids[0]!;
}

function applyCliLimitNarrowing(limits: Limits, args: CliArgs, mode: "local" | "real"): void {
  const defaults = defaultLimits(mode);
  if (args.count !== undefined && mode === "local") {
    // Workload sizing: a larger count widens the ceiling to cover the requested run.
    limits.maxRequests = Math.max(limits.maxRequests, args.count * (args.repeat ?? 1) * ((args.warmup ?? 0) + 1) + 16);
  }
  if (args.timeoutSeconds !== undefined && args.timeoutSeconds < limits.perOpTimeoutSeconds) {
    limits.perOpTimeoutSeconds = args.timeoutSeconds;
  }
  if (mode === "local") return;
  // Real mode never widens anything from the CLI. Config-file approvals are
  // preserved when stricter than the built-in ceilings; nothing may run above
  // them. The requested workload must fit or the run is refused.
  const unit = (value: string): bigint => BigInt(decimalToUnits(value));
  for (const key of ["maxAmount", "maxGrossVolume", "maxOutstandingPrincipal", "maxNativeSpend"] as const) {
    if (unit(limits[key]) > unit(defaults[key])) limits[key] = defaults[key];
  }
  limits.maxConcurrentFunded = Math.min(limits.maxConcurrentFunded, defaults.maxConcurrentFunded);
  const requested = (args.count ?? 0) * (args.repeat ?? 1) * ((args.warmup ?? 0) + 1);
  if (requested > limits.maxRequests) {
    throw new Error(`real mode: requested workload (${requested} ops) exceeds the approved maxRequests (${limits.maxRequests}); raise it in the config file, not on the CLI`);
  }
}

export function requireApiKey(mode: "local" | "real"): string {
  const key = process.env["GUM_API_KEY"];
  if (!key) {
    throw new Error(
      mode === "local"
        ? "GUM_API_KEY is required; mint one with `just seed` + scripts/local-api-key.sh, or the dashboard"
        : "GUM_API_KEY is required (a dedicated benchmark merchant account's key)",
    );
  }
  return key;
}

/** Parses `15m`-style durations into seconds. */
export function parseDuration(text: string): number {
  const match = /^(\d+)(ms|s|m|h)$/.exec(text);
  if (!match) throw new Error(`invalid duration ${text}; use e.g. 90s, 15m, 2h`);
  const value = Number(match[1]);
  switch (match[2]) {
    case "ms": return value / 1000;
    case "s": return value;
    case "m": return value * 60;
    case "h": return value * 3600;
    default: throw new Error(`invalid duration unit in ${text}`);
  }
}
