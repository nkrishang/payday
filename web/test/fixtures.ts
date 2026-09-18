import type { PayerDepositRequest } from "@payday/sdk";
import type { ReadyPayerDepositRequest, UnlockedPayerDepositRequest } from "@/lib/checkout-state";

const ADDRESS = "0x9a3f0000000000000000000000000000000000c2";
const TOKEN = "0x754704Bc059F8C67012fEd69BC8A327a5aafb603";
const BASE_TOKEN = "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913";
/** The networks a fixture request can be paid on: Monad first, Base second. */
export const NETWORKS = [
  { chain: { id: "143", name: "Monad", native_symbol: "MON" }, token: { symbol: "USDC", address: TOKEN, decimals: 6 } },
  { chain: { id: "8453", name: "Base", native_symbol: "ETH" }, token: { symbol: "USDC", address: BASE_TOKEN, decimals: 6 } },
];
/** The wallet the fixture payer attested; the address above commits to it. */
export const PAYER_WALLET = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";

/**
 * A deposit request with the wallet attestation add-on, completed: everything the
 * payer route can disclose is present, the address included, and only transfers
 * from the attested wallet count.
 */
const READY: ReadyPayerDepositRequest = {
  id: "dr_0198f80c-8d2f-7dc1-a369-90556a64f700",
  issuer_name: "Acme Corp",
  heading: null,
  expected_email_hint: null,
  requirements: { email: "not_required", wallet: "approved", merchant_session: "not_required", complete: true },
  status: "awaiting_deposit",
  payable: true,
  expires_at: "2026-09-01T00:00:00Z",
  server_timestamp: "1788000000",
  settlement_tx_hash: null,
  settlement_explorer_url: null,
  payer_message: null,
  content_unlocked: true,
  currency: "USDC",
  networks: NETWORKS,
  chain: { id: "143", name: "Monad", native_symbol: "MON" },
  token: { symbol: "USDC", address: TOKEN, decimals: 6 },
  amount: "25.00",
  amount_base_units: "25000000",
  received: "0",
  received_base_units: "0",
  remaining: "25.00",
  remaining_base_units: "25000000",
  payer_wallet: PAYER_WALLET,
  address: ADDRESS,
  address_explorer_url: null,
  deposit_uri: `ethereum:${TOKEN}@143/transfer?address=${ADDRESS}&uint256=25000000`,
  details: {
    amount: "25.00",
    amount_base_units: "25000000",
    payer: { name: "Globex Corporation" },
    notes: null,
    reference: null,
    attachment: null,
  },
  // Cross-chain Relay cannot honour a sender attestation, so it is gone.
  relay_available: false,
  relay: null,
};

export function payment(overrides: Partial<ReadyPayerDepositRequest> = {}): ReadyPayerDepositRequest {
  return { ...READY, ...overrides };
}

/**
 * A request with no add-ons at all: the address already exists, and the payer
 * may pay from any wallet — or from the QR or copy-address flow without
 * connecting one.
 */
export function unattestedPayment(overrides: Partial<ReadyPayerDepositRequest> = {}): ReadyPayerDepositRequest {
  return payment({
    requirements: { email: "not_required", wallet: "not_required", merchant_session: "not_required", complete: true },
    payer_wallet: null,
    relay_available: true,
    ...overrides,
  });
}

/** USDT is served on Monad alone, as Tether's USDT0. */
export const USDT_TOKEN = "0xe7cd86e13AC4309349F30B3435a9d337750fC82D";
export const USDT_NETWORK = {
  chain: { id: "143", name: "Monad", native_symbol: "MON" },
  token: { symbol: "USDT0", address: USDT_TOKEN, decimals: 6 },
};

/** A USDT request pinned to Monad, bound and ready to pay in USDT0. */
export function usdtPayment(overrides: Partial<ReadyPayerDepositRequest> = {}): ReadyPayerDepositRequest {
  return payment({
    currency: "USDT",
    networks: [USDT_NETWORK],
    token: USDT_NETWORK.token,
    deposit_uri: `ethereum:@143/transfer?address=&uint256=25000000`,
    ...overrides,
  });
}

/**
 * A request with the wallet attestation add-on still pending: the content is
 * present, but the network, address, and attested wallet are all null — the
 * address is derived from both the chosen network and the signed wallet.
 */
export function unboundDepositRequest(
  overrides: Partial<UnlockedPayerDepositRequest> = {},
): UnlockedPayerDepositRequest {
  return {
    ...READY,
    requirements: {
      email: "not_required",
      wallet: "pending",
      merchant_session: "not_required",
      complete: true,
    },
    chain: null,
    token: null,
    payer_wallet: null,
    address: null,
    address_explorer_url: null,
    deposit_uri: null,
    relay_available: false,
    ...overrides,
  };
}

/**
 * A request with no add-ons and several networks on offer, whose address does
 * not exist yet: choosing one is the payer's step, with no signature involved.
 */
export function networkChoiceDepositRequest(
  overrides: Partial<UnlockedPayerDepositRequest> = {},
): UnlockedPayerDepositRequest {
  return unboundDepositRequest({
    requirements: {
      email: "not_required",
      wallet: "not_required",
      merchant_session: "not_required",
      complete: true,
    },
    payer_wallet: null,
    ...overrides,
  });
}

/**
 * An email-gated deposit request before verification: exactly what the payer route sends,
 * with every mechanic and document field null.
 */
export function lockedDepositRequest(overrides: Partial<PayerDepositRequest> = {}): PayerDepositRequest {
  return {
    ...READY,
    expected_email_hint: "a****@e***.com",
    requirements: { email: "pending", wallet: "pending", merchant_session: "not_required", complete: false },
    content_unlocked: false,
    currency: null,
    networks: null,
    chain: null,
    token: null,
    amount: null,
    amount_base_units: null,
    received: null,
    received_base_units: null,
    remaining: null,
    remaining_base_units: null,
    payer_wallet: null,
    address: null,
    address_explorer_url: null,
    deposit_uri: null,
    details: null,
    ...overrides,
  };
}

/**
 * A merchant-auth deposit request before its app has opened it: locked like an
 * email-gated one, with no mailbox hint and no step the payer can take here.
 */
export function merchantSessionDepositRequest(overrides: Partial<PayerDepositRequest> = {}): PayerDepositRequest {
  return lockedDepositRequest({
    expected_email_hint: null,
    requirements: {
      email: "not_required",
      wallet: "pending",
      merchant_session: "pending",
      complete: false,
    },
    ...overrides,
  });
}
