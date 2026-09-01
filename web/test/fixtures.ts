import type { PayerPayment } from "@payday/sdk";

const BASE: PayerPayment = {
  id: "pay_0198f80c-8d2f-7dc1-a369-90556a64f700",
  chain: { id: "143", name: "Monad" },
  token: {
    symbol: "USDC",
    address: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
    decimals: 6,
  },
  amount: "25.00",
  amount_base_units: "25000000",
  received: "0",
  received_base_units: "0",
  remaining: "25.00",
  remaining_base_units: "25000000",
  address: "0x9a3f0000000000000000000000000000000000c2",
  expires_at: "2026-09-01T00:00:00Z",
  server_timestamp: "1788000000",
  status: "awaiting_payment",
  payable: true,
  payment_uri:
    "ethereum:0x754704Bc059F8C67012fEd69BC8A327a5aafb603@143/transfer?address=0x9a3f0000000000000000000000000000000000c2&uint256=25000000",
  address_explorer_url: null,
  settlement_tx_hash: null,
  settlement_explorer_url: null,
  payer_message: null,
};

export function payment(overrides: Partial<PayerPayment> = {}): PayerPayment {
  return { ...BASE, ...overrides };
}
