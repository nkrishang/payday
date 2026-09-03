import type { PayerPayment } from "@payday/sdk";
import type { UnlockedPayerPayment } from "@/lib/checkout-state";

const ADDRESS = "0x9a3f0000000000000000000000000000000000c2";
const TOKEN = "0x754704Bc059F8C67012fEd69BC8A327a5aafb603";

/** A permissionless invoice: everything the payer route can disclose is present. */
const UNLOCKED: UnlockedPayerPayment = {
  id: "pay_0198f80c-8d2f-7dc1-a369-90556a64f700",
  issuer_name: "Acme Corp",
  heading: null,
  payer_policy: { mode: "permissionless", expected_email_hint: null },
  requirements: {
    email: "not_required",
    document: "not_required",
    liveness: "not_required",
    identity_match: "not_required",
    complete: true,
  },
  status: "awaiting_payment",
  payable: true,
  expires_at: "2026-09-01T00:00:00Z",
  server_timestamp: "1788000000",
  settlement_tx_hash: null,
  settlement_explorer_url: null,
  payer_message: null,
  content_unlocked: true,
  chain: { id: "143", name: "Monad" },
  token: { symbol: "USDC", address: TOKEN, decimals: 6 },
  amount: "25.00",
  amount_base_units: "25000000",
  received: "0",
  received_base_units: "0",
  remaining: "25.00",
  remaining_base_units: "25000000",
  address: ADDRESS,
  address_explorer_url: null,
  payment_uri: `ethereum:${TOKEN}@143/transfer?address=${ADDRESS}&uint256=25000000`,
  invoice: {
    amount: "25.00",
    amount_base_units: "25000000",
    bill_to: { name: "Globex Corporation" },
    notes: null,
    reference: null,
    attachment: null,
  },
};

export function payment(overrides: Partial<UnlockedPayerPayment> = {}): UnlockedPayerPayment {
  return { ...UNLOCKED, ...overrides };
}

/**
 * A gated invoice before verification: exactly what the payer route sends,
 * with every mechanic and document field null.
 */
export function lockedPayment(overrides: Partial<PayerPayment> = {}): PayerPayment {
  return {
    ...UNLOCKED,
    payer_policy: { mode: "verified_email", expected_email_hint: "a****@e***.com" },
    requirements: {
      email: "pending",
      document: "not_required",
      liveness: "not_required",
      identity_match: "not_required",
      complete: false,
    },
    content_unlocked: false,
    chain: null,
    token: null,
    amount: null,
    amount_base_units: null,
    received: null,
    received_base_units: null,
    remaining: null,
    remaining_base_units: null,
    address: null,
    address_explorer_url: null,
    payment_uri: null,
    invoice: null,
    ...overrides,
  };
}
