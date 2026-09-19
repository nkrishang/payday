/**
 * Payer wallet pool: dedicated accounts, one nonce owner per (chain, wallet),
 * exactly one unincluded payment per wallet at a time, and signed
 * transactions durably journaled before broadcast.
 */

import { english, generateMnemonic, mnemonicToAccount, privateKeyToAccount, type HDAccount, type PrivateKeyAccount } from "viem/accounts";
import { createPublicClient, createWalletClient, http, type Chain, type WalletClient } from "viem";
import { existsSync, mkdirSync, readFileSync, writeFileSync, chmodSync } from "node:fs";
import { dirname, join } from "node:path";
import { isAnvilDevKey, type ChainConfig } from "./chains.js";

export interface PayerWallet {
  index: number;
  address: `0x${string}`;
  account: HDAccount | PrivateKeyAccount;
}

export interface ChainClients {
  publicClient: ReturnType<typeof createPublicClient>;
  rpcUrl: string;
}

export class WalletPool {
  private readonly wallets: PayerWallet[] = [];
  private readonly nonceOwners = new Map<string, number>();
  private readonly pending = new Map<string, Promise<unknown>>();
  private readonly chainClients = new Map<string, ChainClients>();

  constructor(
    private readonly mnemonic: string,
    private readonly chains: Record<string, ChainConfig>,
    walletCount: number,
    private readonly onEvent: (kind: "warn", text: string) => void,
  ) {
    for (let index = 0; index < walletCount; index++) {
      const account = accountFromSeed(mnemonic, index);
      this.wallets.push({ index, address: account.address, account });
    }
    for (const chain of Object.values(chains)) {
      this.chainClients.set(chain.id, {
        rpcUrl: chain.rpcUrl,
        publicClient: createPublicClient({ chain: viemChain(chain), transport: http(chain.rpcUrl) }),
      });
    }
  }

  all(): PayerWallet[] {
    return this.wallets;
  }

  addresses(): string[] {
    return this.wallets.map((wallet) => wallet.address);
  }

  /** Resolves the next nonce on demand; leases are serialized per (chain, wallet). */
  private async nextNonce(chain: ChainConfig, wallet: PayerWallet): Promise<number> {
    const key = `${chain.id}:${wallet.address}`;
    const clients = this.chainClients.get(chain.id)!;
    if (this.nonceOwners.get(key) === undefined) {
      const count = await clients.publicClient.getTransactionCount({ address: wallet.address });
      this.nonceOwners.set(key, count);
    }
    const nonce = this.nonceOwners.get(key)!;
    this.nonceOwners.set(key, nonce + 1);
    return nonce;
  }

  /**
   * Runs `work` holding the wallet's single submission slot and its nonce
   * sequence on this chain. Serialized per (chain, wallet); never a separate
   * poll loop per wallet.
   */
  async withSubmissionSlot<T>(chain: ChainConfig, wallet: PayerWallet, work: (lease: { nonce: number }) => Promise<T>): Promise<T> {
    const key = `${chain.id}:${wallet.address}`;
    const previous = this.pending.get(key);
    let release!: () => void;
    const gate = new Promise<void>((resolve) => (release = resolve));
    this.pending.set(key, gate);
    if (previous) await previous.catch(() => {});
    try {
      const nonce = await this.nextNonce(chain, wallet);
      return await work({ nonce });
    } finally {
      release();
      void previous;
      const tail = this.pending.get(key) === gate ? undefined : this.pending.get(key);
      if (!tail) this.pending.delete(key);
      else this.pending.set(key, tail);
    }
  }

  /**
   * Reconciles the nonce after an ambiguous broadcast: re-reads the chain and
   * resets the owner. Never assumes the nonce was consumed.
   */
  async reconcileNonce(chain: ChainConfig, wallet: PayerWallet): Promise<void> {
    const clients = this.chainClients.get(chain.id)!;
    const count = await clients.publicClient.getTransactionCount({ address: wallet.address });
    const key = `${chain.id}:${wallet.address}`;
    const owned = this.nonceOwners.get(key) ?? count;
    if (count !== owned) {
      this.onEvent("warn", `wallet ${wallet.address} nonce reconciled on chain ${chain.id}: chain ${count}, owner ${owned}`);
      this.nonceOwners.set(key, count);
    }
  }

  walletClient(chain: ChainConfig, wallet: PayerWallet): WalletClient {
    const base = this.chainClients.get(chain.id)!;
    return createWalletClient({
      account: wallet.account,
      chain: viemChain(chain),
      transport: http(base.rpcUrl),
    }) as WalletClient;
  }

  assertNoAnvilKeys(mode: "local" | "real"): void {
    if (mode === "real" && isAnvilDevKey(this.mnemonic.startsWith("0x") ? this.mnemonic : "")) {
      throw new Error("real mode refused: the configured payer seed is a known Anvil development key");
    }
  }
}

export function viemChain(config: ChainConfig): Chain {
  return {
    id: Number(config.id),
    name: config.name,
    nativeCurrency: { name: config.nativeSymbol, symbol: config.nativeSymbol, decimals: 18 },
    rpcUrls: { default: { http: [config.rpcUrl] } },
    network: `gum-bench-${config.id}`,
  } as Chain;
}

/**
 * Resolves the payer seed. Real mode requires an explicit env mnemonic or
 * private-key file. Local mode may generate and persist a fresh seed inside
 * the run's private directory — never regenerated on resume.
 */
export function resolveSeed(options: {
  mode: "local" | "real";
  seedEnvVar: string;
  runPrivateDir: string;
  sharedSeedFile?: string;
  runId: string;
}): string {
  const fromEnv = process.env[options.seedEnvVar];
  if (fromEnv) {
    assertNotAnvilSeed(fromEnv, options.mode);
    return fromEnv.trim();
  }
  if (options.mode === "real") {
    throw new Error(
      `real mode requires an explicit payer seed in ${options.seedEnvVar} ` +
      "(a dedicated HD mnemonic or a 0x private key; never a wallet holding other funds)",
    );
  }
  const shared = options.sharedSeedFile;
  if (shared && existsSync(shared)) return readFileSync(shared, "utf8").trim();
  mkdirSync(dirname(shared ?? options.runPrivateDir), { recursive: true, mode: 0o700 });
  const persisted = shared ?? join(options.runPrivateDir, "payer-seed.txt");
  const mnemonic = generateMnemonic(english, 256);
  writeFileSync(persisted, `${mnemonic}\n`, { mode: 0o600 });
  chmodSync(persisted, 0o600);
  return mnemonic;
}

/** Well-known development mnemonics (Anvil/Foundry/Hardhat defaults). */
const DEV_MNEMONICS = [
  "test test test test test test test test test test test junk", // Anvil
  "myth like bonus scare over problem client lizard pioneer submit female collect", // Hardhat
  "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about", // BIP39 vector
];

function assertNotAnvilSeed(seed: string, mode: "local" | "real"): void {
  if (mode !== "real") return;
  const trimmed = seed.trim().toLowerCase();
  if (trimmed.startsWith("0x") && isAnvilDevKey(trimmed)) {
    throw new Error("real mode refused: the payer seed is a known Anvil development key");
  }
  if (DEV_MNEMONICS.includes(trimmed)) {
    throw new Error("real mode refused: the payer seed is a well-known development mnemonic");
  }
}

export function accountFromSeed(seed: string, index: number): HDAccount | PrivateKeyAccount {
  if (seed.startsWith("0x")) {
    if (index !== 0) throw new Error("a raw private-key seed supports a single payer wallet; use an HD mnemonic for a pool");
    return privateKeyToAccount(seed as `0x${string}`);
  }
  return mnemonicToAccount(seed, { addressIndex: index });
}
