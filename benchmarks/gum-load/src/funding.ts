/**
 * Local-only payer funding: Anvil account #1 (never #0 — it is a Gum sweep
 * signer) transfers native gas and calls the mock stablecoins' open
 * `mint` for each payer wallet. Restricted to local chain ids and the mock
 * token addresses from the local defaults; a failed mint fails loudly.
 */

import { createWalletClient, http, parseUnits, type Address } from "viem";
import { privateKeyToAccount } from "viem/accounts";
import { encodeFunctionData } from "viem";
import { ANVIL_FUNDER, ERC20_ABI, LOCAL_CHAIN_IDS, type ChainConfig } from "./chains.js";
import type { WalletPool } from "./wallets.js";

export interface FundingPlan {
  chainId: string;
  nativePerWallet: bigint;
  tokenUnitsPerWallet: bigint;
  tokenAddress: string;
}

export function assertFundingIsLocal(chain: ChainConfig, tokenAddress: string): void {
  if (!LOCAL_CHAIN_IDS.has(chain.id)) {
    throw new Error(`funding.refuse: chain ${chain.id} is not a local Anvil chain`);
  }
  const allowed = Object.values(chain.tokens).map((address) => address.toLowerCase());
  if (!allowed.includes(tokenAddress.toLowerCase())) {
    throw new Error(`funding.refuse: ${tokenAddress} is not a mock token configured for local chain ${chain.id}`);
  }
}

/**
 * Funds every payer wallet from Anvil account #1: native gas first, then a
 * `mint` per token. Funder nonces are serialized by awaiting each receipt;
 * balances are re-read and verified before the measured phase.
 */
export async function fundLocalPayers(options: {
  pool: WalletPool;
  chain: ChainConfig;
  plan: FundingPlan;
  currency: string;
}): Promise<void> {
  assertFundingIsLocal(options.chain, options.plan.tokenAddress);
  const funder = privateKeyToAccount(ANVIL_FUNDER.key as `0x${string}`);
  const walletClient = createWalletClient({ account: funder, chain: viemChainFor(options.chain), transport: http(options.chain.rpcUrl) });
  const wallets = options.pool.all();
  const txHashes: string[] = [];

  for (const wallet of wallets) {
    const hash = await walletClient.sendTransaction({
      to: wallet.address as Address,
      value: options.plan.nativePerWallet,
      gas: 21_000n,
    });
    txHashes.push(hash);
  }
  for (const wallet of wallets) {
    const data = encodeFunctionData({
      abi: ERC20_ABI,
      functionName: "mint",
      args: [wallet.address as Address, options.plan.tokenUnitsPerWallet],
    });
    const hash = await walletClient.sendTransaction({
      to: options.plan.tokenAddress as Address,
      data,
      gas: 120_000n,
    });
    txHashes.push(hash);
  }

  for (const hash of txHashes) {
    const receipt = await waitForReceipt(options.chain.rpcUrl, hash);
    if (receipt.status !== "success") throw new Error(`funding transaction ${hash} reverted; refusing to continue`);
  }

  // Verify balances instead of trusting receipts.
  const { createPublicClient } = await import("viem");
  const publicClient = createPublicClient({ chain: viemChainFor(options.chain), transport: http(options.chain.rpcUrl) });
  for (const wallet of wallets) {
    const [native, token] = await Promise.all([
      publicClient.getBalance({ address: wallet.address as Address }),
      publicClient.readContract({
        address: options.plan.tokenAddress as Address,
        abi: ERC20_ABI,
        functionName: "balanceOf",
        args: [wallet.address as Address],
      }) as Promise<bigint>,
    ]);
    if (native < options.plan.nativePerWallet) {
      throw new Error(`wallet ${wallet.address} funded with less native than planned on chain ${options.chain.id}`);
    }
    if (token < options.plan.tokenUnitsPerWallet) {
      throw new Error(`wallet ${wallet.address} minted less ${options.currency} than planned on chain ${options.chain.id}`);
    }
  }
  return void options;
}

export function defaultFundingPlan(chain: ChainConfig, currency: string, wallets: number, amountUnits: bigint): FundingPlan {
  const headroom = 4n; // sweeps recycle payouts; payers also pay gas
  return {
    chainId: chain.id,
    nativePerWallet: parseUnits("0.05", 18),
    tokenUnitsPerWallet: amountUnits * BigInt(wallets) * headroom + amountUnits * 10n,
    tokenAddress: chain.tokens[currency] ?? Object.values(chain.tokens)[0]!,
  };
}

function viemChainFor(config: ChainConfig) {
  return {
    id: Number(config.id),
    name: config.name,
    nativeCurrency: { name: config.nativeSymbol, symbol: config.nativeSymbol, decimals: 18 },
    rpcUrls: { default: { http: [config.rpcUrl] } },
    network: `gum-fund-${config.id}`,
  };
}

async function waitForReceipt(rpcUrl: string, hash: string): Promise<{ status: string }> {
  const { createPublicClient, http } = await import("viem");
  // Anvil mines with the runner's cadence; a plain bounded poll suffices.
  for (let attempt = 0; attempt < 120; attempt++) {
    const client = createPublicClient({ transport: http(rpcUrl) });
    const receipt = await client.getTransactionReceipt({ hash: hash as `0x${string}` }).catch(() => null);
    if (receipt) return { status: receipt.status };
    await sleep(500);
  }
  throw new Error(`funding transaction ${hash} not mined in time`);
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}
