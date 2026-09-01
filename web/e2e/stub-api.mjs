/**
 * A stand-in for the Payday payer API, so the checkout's states can be driven
 * deterministically in a browser.
 *
 * The scenario is encoded in the payment id, which keeps every test independent
 * — no shared mutable state, no ordering between specs. Shapes here mirror
 * `PayerPaymentResponse` in crates/gateway-core/src/dto.rs exactly.
 */
import { createServer } from "node:http";

const PORT = Number(process.env.STUB_PORT ?? 4010);
const TOKEN = "0x754704Bc059F8C67012fEd69BC8A327a5aafb603";
const ADDRESS = "0x9a3f0000000000000000000000000000000000c2";

// A real QR is unnecessary here; only load success or a 410 matters.
const QR_SVG =
  '<svg xmlns="http://www.w3.org/2000/svg" width="224" height="224" viewBox="0 0 2 2">' +
  '<rect width="2" height="2" fill="#fff"/><rect width="1" height="1" fill="#090811"/></svg>';

function base(overrides = {}) {
  const now = Math.floor(Date.now() / 1000);
  return {
    id: "pay_0198f80c-8d2f-7dc1-a369-90556a64f700",
    chain: { id: "143", name: "Monad" },
    token: { symbol: "USDC", address: TOKEN, decimals: 6 },
    amount: "25.000000",
    amount_base_units: "25000000",
    received: "0",
    received_base_units: "0",
    remaining: "25.000000",
    remaining_base_units: "25000000",
    address: ADDRESS,
    expires_at: new Date((now + 3600) * 1000).toISOString(),
    server_timestamp: String(now),
    status: "awaiting_payment",
    payable: true,
    payment_uri: `ethereum:${TOKEN}@143/transfer?address=${ADDRESS}&uint256=25000000`,
    address_explorer_url: null,
    settlement_tx_hash: null,
    settlement_explorer_url: null,
    payer_message: null,
    ...overrides,
  };
}

const PARTIAL = {
  status: "partially_paid",
  received: "10.000000",
  received_base_units: "10000000",
  remaining: "15.000000",
  remaining_base_units: "15000000",
  payment_uri: `ethereum:${TOKEN}@143/transfer?address=${ADDRESS}&uint256=15000000`,
};

const FULL = {
  received: "25.000000",
  received_base_units: "25000000",
  remaining: "0",
  remaining_base_units: "0",
};

const CLOSED = { payable: false, payment_uri: null };

const SETTLED = {
  status: "settled",
  settlement_tx_hash: "0x00210b337281f97a1d0747a1535795822998906f5f7e89917a8cd4ee83aa0190",
  settlement_explorer_url:
    "https://monadvision.com/tx/0x00210b337281f97a1d0747a1535795822998906f5f7e89917a8cd4ee83aa0190",
};

/** Reads counted per id, so one scenario can change between polls. */
const reads = new Map();

const scenarios = {
  awaiting: () => base(),
  partial: () => base(PARTIAL),
  paid: () => base({ ...FULL, ...CLOSED, status: "paid" }),
  settled: () => base({ ...FULL, ...CLOSED, ...SETTLED }),
  // More arrived than was asked for. Settlement is exact, so the merchant got
  // 25 and the remainder went to the recovery wallet; the receipt says so.
  "settled-overpaid": () =>
    base({ ...FULL, ...CLOSED, ...SETTLED, received: "30.000000", received_base_units: "30000000" }),
  expired: () => base({ ...CLOSED, status: "expired" }),
  "expired-funded": () => base({ ...PARTIAL, ...CLOSED, status: "expired" }),
  returned: () => base({ ...PARTIAL, ...CLOSED, status: "returned" }),
  attention: () =>
    base({
      ...CLOSED,
      status: "needs_attention",
      payer_message:
        "Payout is paused, but your funds remain safe. The merchant and Payday support are resolving settlement; do not send a second payment.",
    }),
  // Payable false while the status has not yet flipped: the deadline passed but
  // the indexer has not committed the expiry.
  closing: () => base(CLOSED),
  // Ends moments from now, to prove the page waits for the server rather than
  // declaring the payment expired off its own countdown.
  ending: () => {
    const now = Math.floor(Date.now() / 1000);
    return base({
      server_timestamp: String(now),
      expires_at: new Date((now + 3) * 1000).toISOString(),
    });
  },
  // First read is awaiting; every later read is partially paid.
  transition: (id) => {
    const seen = (reads.get(id) ?? 0) + 1;
    reads.set(id, seen);
    return seen <= 1 ? base() : base(PARTIAL);
  },
  // A different chain, to prove the wallet button refuses to sign for it.
  "other-chain": () => base({ chain: { id: "1", name: "Ethereum" } }),
};

function scenarioFor(id) {
  return scenarios[id.replace(/^pay_/, "")] ?? null;
}

function send(res, status, body, headers = {}) {
  res.writeHead(status, {
    "content-type": "application/json",
    "cache-control": "no-store",
    "access-control-allow-origin": "*",
    ...headers,
  });
  res.end(typeof body === "string" ? body : JSON.stringify(body));
}

createServer((req, res) => {
  const url = new URL(req.url ?? "/", "http://localhost");

  if (url.pathname === "/health") return send(res, 200, { ok: true });
  if (url.pathname === "/__reset") {
    reads.clear();
    return send(res, 200, { ok: true });
  }

  const match = url.pathname.match(/^\/v1\/payer\/payments\/([^/]+)(\/qr)?$/);
  if (!match) {
    return send(res, 404, { error: { code: "not_found", message: "no route" }, request_id: "stub" });
  }

  const id = decodeURIComponent(match[1]);
  const scenario = scenarioFor(id);
  if (!scenario) {
    return send(res, 401, {
      error: { code: "invalid_payment_link", message: "Payment link is not valid" },
      request_id: "stub",
    });
  }

  const payment = { ...scenario(id), id };

  if (match[2]) {
    if (!payment.payable) {
      return send(res, 410, {
        error: { code: "payment_not_payable", message: "No longer payable" },
        request_id: "stub",
      });
    }
    res.writeHead(200, {
      "content-type": "image/svg+xml; charset=utf-8",
      "cache-control": "no-store",
      "access-control-allow-origin": "*",
    });
    return res.end(QR_SVG);
  }

  return send(res, 200, payment);
}).listen(PORT, "127.0.0.1", () => {
  console.log(`stub payer API on http://127.0.0.1:${PORT}`);
});
