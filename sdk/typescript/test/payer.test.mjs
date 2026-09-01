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

const requirementsNone = { email: "not_required", document: "not_required", liveness: "not_required", identity_match: "not_required", complete: true };

const payerPayment = {
  id: "pay_0198f80c-8d2f-7dc1-a369-90556a64f700",
  issuer_name: "Acme LLC",
  heading: "March retainer",
  payer_policy: { mode: "permissionless", expected_email_hint: null },
  requirements: requirementsNone,
  status: "awaiting_payment",
  payable: true,
  expires_at: "2026-09-01T00:00:00Z",
  server_timestamp: "1788000000",
  settlement_tx_hash: null,
  settlement_explorer_url: null,
  payer_message: null,
  content_unlocked: true,
  chain: { id: "143", name: "Monad" },
  token: { symbol: "USDC", address: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603", decimals: 6 },
  amount: "25.00", amount_base_units: "25000000",
  received: "0", received_base_units: "0",
  remaining: "25.00", remaining_base_units: "25000000",
  address: "0x1111111111111111111111111111111111111111",
  address_explorer_url: null,
  payment_uri: "ethereum:0x754704Bc059F8C67012fEd69BC8A327a5aafb603@143/transfer?address=0x1111111111111111111111111111111111111111&uint256=25000000",
  invoice: {
    amount: "25.00", amount_base_units: "25000000",
    bill_to: { name: "Customer Inc" }, notes: null, reference: "INV-1042",
    attachment: { id: "att_1", filename: "invoice.pdf", mime_type: "application/pdf", byte_length: "5", sha256: "0xabc" },
  },
};

/** What a gated invoice looks like before its payer has verified anything. */
const lockedPayment = {
  id: payerPayment.id,
  issuer_name: "Acme LLC",
  heading: "March retainer",
  payer_policy: { mode: "verified_email", expected_email_hint: "a****@e***.com" },
  requirements: { ...requirementsNone, email: "pending", complete: false },
  status: "awaiting_payment",
  payable: true,
  expires_at: "2026-09-01T00:00:00Z",
  server_timestamp: "1788000000",
  settlement_tx_hash: null,
  settlement_explorer_url: null,
  payer_message: null,
  content_unlocked: false,
  chain: null, token: null,
  amount: null, amount_base_units: null,
  received: null, received_base_units: null,
  remaining: null, remaining_base_units: null,
  address: null, address_explorer_url: null, payment_uri: null,
  invoice: null,
};

test("payer reads use the public route and send no authorization", async () => {
  const mock = mockFetch(() => new Response(JSON.stringify(payerPayment), {
    headers: { "content-type": "application/json" },
  }));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test/", fetch: mock.fetch });

  const payment = await client.payments.get(payerPayment.id);

  assert.equal(payment.remaining_base_units, "25000000");
  assert.equal(payment.payable, true);
  assert.equal(payment.content_unlocked, true);
  assert.equal(payment.invoice.reference, "INV-1042");
  assert.equal(mock.calls[0].url, `https://example.test/v1/payer/payments/${payerPayment.id}`);
  assert.equal(mock.calls[0].init.method, "GET");
  assert.equal(mock.calls[0].init.headers.Authorization, undefined);
  assert.equal(mock.calls[0].init.headers["Payday-Payer-Session"], undefined);
  assert.equal(mock.calls[0].init.cache, "no-store");
});

test("a locked gated invoice carries null mechanics and no invoice content", async () => {
  const mock = mockFetch(() => new Response(JSON.stringify(lockedPayment)));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test", fetch: mock.fetch });

  const payment = await client.payments.get(lockedPayment.id);

  assert.equal(payment.content_unlocked, false);
  assert.equal(payment.issuer_name, "Acme LLC");
  assert.equal(payment.payer_policy.expected_email_hint, "a****@e***.com");
  assert.equal(payment.requirements.complete, false);
  for (const field of [
    "chain", "token", "amount", "amount_base_units", "received", "received_base_units",
    "remaining", "remaining_base_units", "address", "address_explorer_url", "payment_uri", "invoice",
  ]) {
    assert.equal(payment[field], null, `${field} must be withheld while locked`);
  }
});

test("payer reads forward a session token and an abort signal, and encode the id", async () => {
  const mock = mockFetch(() => new Response(JSON.stringify(payerPayment)));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test", fetch: mock.fetch });
  const controller = new AbortController();

  await client.payments.get("pay_a/b", { signal: controller.signal, payerSession: "pps_token" });

  assert.equal(mock.calls[0].url, "https://example.test/v1/payer/payments/pay_a%2Fb");
  assert.equal(mock.calls[0].init.signal, controller.signal);
  assert.equal(mock.calls[0].init.headers["Payday-Payer-Session"], "pps_token");
  assert.equal(mock.calls[0].init.headers.Authorization, undefined);
});

test("payer attachment reads use the public sub-route with an optional session", async () => {
  const mock = mockFetch(() => new Response(JSON.stringify(payerPayment.invoice.attachment)));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test", fetch: mock.fetch });

  const open = await client.payments.attachment("pay_a/b");
  await client.payments.attachment("pay_a/b", "pps_token");

  assert.equal(open.filename, "invoice.pdf");
  assert.equal(mock.calls[0].url, "https://example.test/v1/payer/payments/pay_a%2Fb/attachment");
  assert.equal(mock.calls[0].init.method, "GET");
  assert.equal(mock.calls[0].init.headers["Payday-Payer-Session"], undefined);
  assert.equal(mock.calls[1].init.headers["Payday-Payer-Session"], "pps_token");
});

test("a locked attachment surfaces verification_required", async () => {
  const mock = mockFetch(() => new Response(JSON.stringify({
    error: { code: "verification_required", message: "Complete verification first" }, request_id: "req-7",
  }), { status: 401 }));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test", fetch: mock.fetch });

  await assert.rejects(client.payments.attachment("pay_1"), (error) => {
    assert.ok(error instanceof PaydayError);
    assert.equal(error.code, "verification_required");
    assert.equal(error.status, 401);
    return true;
  });
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
