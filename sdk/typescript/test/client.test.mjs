import assert from "node:assert/strict";
import test from "node:test";
import { PaydayClient, PaydayError } from "../dist/index.js";

function mockFetch(handler) {
  const calls = [];
  const fetch = async (url, init) => {
    calls.push({ url, init });
    return handler(url, init);
  };
  return { fetch, calls };
}

test("create sends bearer auth, JSON, and the caller's idempotency key", async () => {
  const mock = mockFetch(() => new Response(JSON.stringify({ id: "pay_1", status: "awaiting_payment" }), {
    status: 201, headers: { "content-type": "application/json" },
  }));
  const client = new PaydayClient({ apiKey: "secret", baseUrl: "https://example.test/", fetch: mock.fetch });
  const input = {
    amount: "10.00", payout_address: "0x1", refund_address: "0x2", expires_in: 3600,
  };

  const payment = await client.payments.create(input, "order-123");

  assert.equal(payment.id, "pay_1");
  assert.equal(mock.calls[0].url, "https://example.test/v1/payments");
  assert.equal(mock.calls[0].init.method, "POST");
  assert.equal(mock.calls[0].init.headers.Authorization, "Bearer secret");
  assert.equal(mock.calls[0].init.headers["Idempotency-Key"], "order-123");
  assert.deepEqual(JSON.parse(mock.calls[0].init.body), input);
});

test("create rejects a missing idempotency key before fetch", () => {
  const mock = mockFetch(() => { throw new Error("must not fetch"); });
  const client = new PaydayClient({ apiKey: "secret", fetch: mock.fetch });
  assert.throws(() => client.payments.create({
    amount: "1", payout_address: "0x1", refund_address: "0x2", expires_in: 60,
  }, ""), /idempotencyKey is required/);
  assert.equal(mock.calls.length, 0);
});

test("list and long polling encode payment query parameters", async () => {
  const mock = mockFetch(() => new Response('{"payments":[],"next_cursor":null}'));
  const client = new PaydayClient({ apiKey: "secret", baseUrl: "https://example.test", fetch: mock.fetch });
  const page = await client.payments.list({ starting_after: "pay_cursor/id", status: "paid", reference: "a & b", limit: 25 });
  assert.deepEqual(page, { payments: [], next_cursor: null });
  assert.equal(mock.calls[0].url, "https://example.test/v1/payments?starting_after=pay_cursor%2Fid&status=paid&reference=a+%26+b&limit=25");
  await client.payments.get("payment/id", { waitForChange: true, timeout: 30 });
  assert.equal(mock.calls[1].url, "https://example.test/v1/payments/payment%2Fid?wait_for=change&timeout=30");
});

test("webhook and status methods use canonical routes", async () => {
  const mock = mockFetch(() => new Response("[]"));
  const client = new PaydayClient({ apiKey: "secret", baseUrl: "https://example.test", fetch: mock.fetch });
  await client.webhooks.add("https://hooks.example.test/payday");
  await client.webhooks.remove("endpoint/id");
  await client.webhooks.test("endpoint/id");
  await client.webhooks.deliveries();
  await client.status();
  assert.deepEqual(mock.calls.map((call) => [call.init.method, call.url]), [
    ["POST", "https://example.test/v1/webhooks"],
    ["DELETE", "https://example.test/v1/webhooks/endpoint%2Fid"],
    ["POST", "https://example.test/v1/webhooks/endpoint%2Fid/test"],
    ["GET", "https://example.test/v1/webhook-deliveries"],
    ["GET", "https://example.test/v1/status"],
  ]);
});

test("API errors expose stable code, requestId, and HTTP status", async () => {
  const mock = mockFetch(() => new Response(JSON.stringify({
    error: { code: "invoice_not_found", message: "Invoice not found" }, request_id: "req-123",
  }), { status: 404 }));
  const client = new PaydayClient({ apiKey: "secret", fetch: mock.fetch });

  await assert.rejects(client.payments.get("missing"), (error) => {
    assert.ok(error instanceof PaydayError);
    assert.equal(error.message, "Invoice not found");
    assert.equal(error.code, "invoice_not_found");
    assert.equal(error.requestId, "req-123");
    assert.equal(error.status, 404);
    return true;
  });
});
