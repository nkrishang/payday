import assert from "node:assert/strict";
import test from "node:test";
import { PaydayError, PaydayPayerClient } from "../dist/index.js";

function mockFetch(handler) {
  const calls = [];
  const fetch = async (url, init) => {
    calls.push({ url, init });
    return handler(url, init);
  };
  return { fetch, calls };
}

const payerPayment = {
  id: "pay_0198f80c-8d2f-7dc1-a369-90556a64f700",
  chain: { id: "143", name: "Monad" },
  token: { symbol: "USDC", address: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603", decimals: 6 },
  amount: "25.00", amount_base_units: "25000000",
  received: "0", received_base_units: "0",
  remaining: "25.00", remaining_base_units: "25000000",
  address: "0x1111111111111111111111111111111111111111",
  expires_at: "2026-09-01T00:00:00Z",
  server_timestamp: "1788000000",
  status: "awaiting_payment",
  payable: true,
  payment_uri: "ethereum:0x754704Bc059F8C67012fEd69BC8A327a5aafb603@143/transfer?address=0x1111111111111111111111111111111111111111&uint256=25000000",
  address_explorer_url: null,
  settlement_tx_hash: null,
  settlement_explorer_url: null,
  payer_message: null,
};

test("payer reads use the public route and send no authorization", async () => {
  const mock = mockFetch(() => new Response(JSON.stringify(payerPayment), {
    headers: { "content-type": "application/json" },
  }));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test/", fetch: mock.fetch });

  const payment = await client.payments.get(payerPayment.id);

  assert.equal(payment.remaining_base_units, "25000000");
  assert.equal(payment.payable, true);
  assert.equal(mock.calls[0].url, `https://example.test/v1/payer/payments/${payerPayment.id}`);
  assert.equal(mock.calls[0].init.method, "GET");
  assert.equal(mock.calls[0].init.headers.Authorization, undefined);
  assert.equal(mock.calls[0].init.cache, "no-store");
});

test("payer reads forward an abort signal and encode the id", async () => {
  const mock = mockFetch(() => new Response(JSON.stringify(payerPayment)));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test", fetch: mock.fetch });
  const controller = new AbortController();

  await client.payments.get("pay_a/b", { signal: controller.signal });

  assert.equal(mock.calls[0].url, "https://example.test/v1/payer/payments/pay_a%2Fb");
  assert.equal(mock.calls[0].init.signal, controller.signal);
});

test("qrUrl is absolute and encoded", () => {
  const client = new PaydayPayerClient({ baseUrl: "https://example.test/", fetch: async () => new Response("") });
  assert.equal(
    client.payments.qrUrl("pay_a/b"),
    "https://example.test/v1/payer/payments/pay_a%2Fb/qr",
  );
});

test("payer client defaults to the production API origin", () => {
  const client = new PaydayPayerClient({ fetch: async () => new Response("") });
  assert.equal(client.payments.qrUrl("pay_1"), "https://api.payday.sh/v1/payer/payments/pay_1/qr");
});

test("an unknown or malformed link surfaces invalid_payment_link", async () => {
  const mock = mockFetch(() => new Response(JSON.stringify({
    error: { code: "invalid_payment_link", message: "Payment link is not valid" }, request_id: "req-9",
  }), { status: 401 }));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test", fetch: mock.fetch });

  await assert.rejects(client.payments.get("pay_missing"), (error) => {
    assert.ok(error instanceof PaydayError);
    assert.equal(error.code, "invalid_payment_link");
    assert.equal(error.status, 401);
    assert.equal(error.requestId, "req-9");
    return true;
  });
});
