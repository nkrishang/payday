/**
 * Chain configuration: local Anvil defaults, real-chain defaults with
 * operator-provided RPC, finality policy, and the minimal ABIs the observer
 * needs. Token/factory addresses are allowlists: API responses must agree
 * with them before money moves.
 */

export interface FinalityPolicy {
  kind: "finalized" | "confirmations";
  /** For `confirmations`: Gum accepts a receipt at block B once head ≥ B + margin. */
  margin: number;
}

export interface ChainConfig {
  id: string;
  name: string;
  rpcUrl: string;
  finality: FinalityPolicy;
  /** Allowed token contracts per currency, lowercase hex. */
  tokens: Record<string, string>;
  /** Allowed factory contracts (challenge `verifyingContract`), lowercase hex; optional. */
  factories?: string[];
  nativeSymbol: string;
  /** Gas ceiling for one payer transfer, wei. */
  maxFeePerGasWei?: string;
  explorerUrl?: string;
  mock?: boolean;
}

export const ERC20_ABI = [
  {
    type: "event",
    name: "Transfer",
    inputs: [
      { name: "from", type: "address", indexed: true },
      { name: "to", type: "address", indexed: true },
      { name: "value", type: "uint256", indexed: false },
    ],
  },
  { type: "function", name: "mint", stateMutability: "nonpayable", inputs: [{ name: "to", type: "address" }, { name: "value", type: "uint256" }], outputs: [] },
  { type: "function", name: "transfer", stateMutability: "nonpayable", inputs: [{ name: "to", type: "address" }, { name: "value", type: "uint256" }], outputs: [{ name: "", type: "bool" }] },
  { type: "function", name: "decimals", stateMutability: "view", inputs: [], outputs: [{ name: "", type: "uint8" }] },
  { type: "function", name: "balanceOf", stateMutability: "view", inputs: [{ name: "who", type: "address" }], outputs: [{ name: "", type: "uint256" }] },
] as const;

export const LOCAL_CHAINS: Record<string, ChainConfig> = {
  "31337": {
    id: "31337", name: "Anvil A", rpcUrl: "http://127.0.0.1:8545",
    finality: { kind: "finalized", margin: 0 },
    tokens: { USDC: "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512", USDT: "0xCf7Ed3AccA5a467e9e704C703E8D87F634fB0Fc9" },
    factories: ["0x5FbDB2315678afecb367f032d93F642f64180aa3"],
    nativeSymbol: "ETH", mock: true,
  },
  "31338": {
    id: "31338", name: "Anvil B", rpcUrl: "http://127.0.0.1:8546",
    finality: { kind: "confirmations", margin: 2 },
    tokens: { USDC: "0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512", USDT: "0xCf7Ed3AccA5a467e9e704C703E8D87F634fB0Fc9" },
    factories: ["0x5FbDB2315678afecb367f032d93F642f64180aa3"],
    nativeSymbol: "ETH", mock: true,
  },
};

export const REAL_CHAIN_DEFAULTS: Record<string, Omit<ChainConfig, "rpcUrl">> = {
  "143": {
    id: "143", name: "Monad", finality: { kind: "finalized", margin: 0 },
    tokens: { USDC: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603", USDT: "0xe7cd86e13AC4309349F30B3435a9d337750fC82D" },
    nativeSymbol: "MON",
  },
  "8453": {
    id: "8453", name: "Base", finality: { kind: "confirmations", margin: 10 },
    tokens: { USDC: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913" },
    nativeSymbol: "ETH",
  },
  "42161": {
    id: "42161", name: "Arbitrum One", finality: { kind: "confirmations", margin: 40 },
    tokens: { USDC: "0xaf88d065e77c8cC2239327C5EDb3A432268e5831", USDT: "0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9" },
    nativeSymbol: "ETH",
  },
};

export const EXPLORERS: Record<string, string> = {
  "143": "https://monadexplorer.com",
  "8453": "https://basescan.org",
  "42161": "https://arbiscan.io",
  "31337": "",
  "31338": "",
};

export const ANVIL_FUNDER = {
  address: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
  key: "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
};

export const LOCAL_CHAIN_IDS = new Set(Object.keys(LOCAL_CHAINS));

/** Real Anvil development keys must never fund real-mode transfers. */
const ANVIL_DEV_KEYS = new Set([
  "0xac0974bec39a17e36ba4a6b4d238ff944bacb478cbed5efcae784d7bf4f2ff80",
  "0x59c6995e998f97a5a0044966f0945389dc9e86dae88c7a8412f4603b6b78690d",
]);

export function isAnvilDevKey(privateKey: string): boolean {
  return ANVIL_DEV_KEYS.has(privateKey.toLowerCase());
}
