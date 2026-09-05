import assert from "node:assert/strict";
import test from "node:test";
import { PaydayClient, PaydayError, checkoutUrl } from "../dist/index.js";

function mockFetch(handler) {
  const calls = [];
  const fetch = async (url, init) => {
    calls.push({ url, init });
    return handler(url, init, calls.length);
  };
  return { fetch, calls };
}

const json = (body, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const apiError = (code, status, message = code) =>
  json({ error: { code, message }, request_id: `req-${code}` }, status);

const invoice = {
  amount: "10.00",
  payout_address: "0x1111111111111111111111111111111111111111",
  issuer: { name: "Acme LLC", email: "billing@acme.example" },
  bill_to: { name: "Customer Inc", details: "12 Main St" },
  payer_policy: { mode: "verified_email", expected_email: "alice@example.com" },
  heading: "March retainer",
  expires_in: 3600,
};

test("create sends bearer auth, JSON, and the caller's idempotency key", async () => {
  const mock = mockFetch(() => json({ id: "pay_1", status: "awaiting_payment" }, 201));
  const client = new PaydayClient({ apiKey: "secret", baseUrl: "https://example.test/", fetch: mock.fetch });

  const payment = await client.payments.create(invoice, "order-123");

  assert.equal(payment.id, "pay_1");
  assert.equal(mock.calls[0].url, "https://example.test/v1/payments");
  assert.equal(mock.calls[0].init.method, "POST");
  assert.equal(mock.calls[0].init.headers.Authorization, "Bearer secret");
  assert.equal(mock.calls[0].init.headers["Idempotency-Key"], "order-123");
  assert.deepEqual(JSON.parse(mock.calls[0].init.body), invoice);
});

test("create sends exactly the invoice fields, never a refund address or memo", async () => {
  const mock = mockFetch(() => json({ id: "pay_1", status: "awaiting_payment" }, 201));
  const client = new PaydayClient({ apiKey: "secret", baseUrl: "https://example.test", fetch: mock.fetch });

  await client.payments.create(invoice, "order-123");

  // The API rejects unknown fields outright, so the exact payload on the wire
  // is what matters — not the type the caller was offered.
  const sent = JSON.parse(mock.calls[0].init.body);
  assert.equal("refund_address" in sent, false);
  assert.equal("memo" in sent, false);
  assert.deepEqual(Object.keys(sent).sort(), [
    "amount", "bill_to", "expires_in", "heading", "issuer", "payer_policy", "payout_address",
  ]);
});

test("create rejects a missing idempotency key before fetch", () => {
  const mock = mockFetch(() => { throw new Error("must not fetch"); });
  const client = new PaydayClient({ apiKey: "secret", fetch: mock.fetch });
  assert.throws(() => client.payments.create(invoice, ""), /idempotencyKey is required/);
  assert.equal(mock.calls.length, 0);
});

test("the client takes exactly one credential and sends either as the bearer", async () => {
  assert.throws(() => new PaydayClient({}), /exactly one of apiKey or accessToken/);
  assert.throws(
    () => new PaydayClient({ apiKey: "key", accessToken: "token" }),
    /exactly one of apiKey or accessToken/,
  );

  const mock = mockFetch(() => json({ payments: [], next_cursor: null }));
  const dashboard = new PaydayClient({ accessToken: "eyJ.access.token", baseUrl: "https://example.test", fetch: mock.fetch });
  await dashboard.payments.list();
  assert.equal(mock.calls[0].init.headers.Authorization, "Bearer eyJ.access.token");
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

test("attachment, invoice PDF, and proof reads use the payment sub-routes", async () => {
  const pdf = new Uint8Array([0x25, 0x50, 0x44, 0x46, 0x2d]);
  const mock = mockFetch((url) => {
    if (url.endsWith("/invoice.pdf")) return new Response(pdf, { headers: { "content-type": "application/pdf" } });
    if (url.endsWith("/proof")) return json({ version: "1", payment_id: "pay_a/b", transfers: [] });
    return json({ id: "att_1", filename: "invoice.pdf", mime_type: "application/pdf", download_url: "https://signed.example" });
  });
  const client = new PaydayClient({ apiKey: "secret", baseUrl: "https://example.test", fetch: mock.fetch });

  const attachment = await client.payments.attachment("pay_a/b");
  const blob = await client.payments.invoicePdf("pay_a/b");
  const proof = await client.payments.proof("pay_a/b");

  assert.equal(attachment.download_url, "https://signed.example");
  assert.deepEqual(new Uint8Array(await blob.arrayBuffer()), pdf);
  assert.equal(proof.payment_id, "pay_a/b");
  assert.deepEqual(mock.calls.map((call) => [call.init.method, call.url, call.init.headers.Accept]), [
    ["GET", "https://example.test/v1/payments/pay_a%2Fb/attachment", "application/json"],
    ["GET", "https://example.test/v1/payments/pay_a%2Fb/invoice.pdf", "application/pdf"],
    ["GET", "https://example.test/v1/payments/pay_a%2Fb/proof", "application/json"],
  ]);
  for (const call of mock.calls) assert.equal(call.init.headers.Authorization, "Bearer secret");
});

test("proof before settlement surfaces payment_not_settled", async () => {
  const mock = mockFetch(() => apiError("payment_not_settled", 409, "Payment has not settled"));
  const client = new PaydayClient({ apiKey: "secret", baseUrl: "https://example.test", fetch: mock.fetch });

  await assert.rejects(client.payments.proof("pay_1"), (error) => {
    assert.ok(error instanceof PaydayError);
    assert.equal(error.code, "payment_not_settled");
    assert.equal(error.status, 409);
    return true;
  });
});

test("customer methods use canonical routes, PATCH for updates, and encoded cursors", async () => {
  const customer = { id: "cus_1", name: "Customer Inc", email: null, details: null };
  const mock = mockFetch((url) => (url.includes("?") ? json({ customers: [customer], next_cursor: null }) : json(customer)));
  const client = new PaydayClient({ apiKey: "secret", baseUrl: "https://example.test", fetch: mock.fetch });

  await client.customers.create({ name: "Customer Inc", email: "ap@customer.example" });
  await client.customers.get("cus/1");
  await client.customers.list({ limit: 50, starting_after: "cus/cursor" });
  await client.customers.update("cus/1", { name: "Customer Inc", email: null, details: "Net 30" });

  assert.deepEqual(mock.calls.map((call) => [call.init.method, call.url]), [
    ["POST", "https://example.test/v1/customers"],
    ["GET", "https://example.test/v1/customers/cus%2F1"],
    ["GET", "https://example.test/v1/customers?limit=50&starting_after=cus%2Fcursor"],
    ["PATCH", "https://example.test/v1/customers/cus%2F1"],
  ]);
  assert.deepEqual(JSON.parse(mock.calls[0].init.body), { name: "Customer Inc", email: "ap@customer.example" });
  assert.deepEqual(JSON.parse(mock.calls[3].init.body), { name: "Customer Inc", email: null, details: "Net 30" });
  assert.equal(mock.calls[3].init.headers["Content-Type"], "application/json");
});

const slot = {
  id: "att_1",
  upload_url: "https://bucket.example/uploads/acc/att_1?X-Amz-Signature=abc",
  headers: { "Content-Type": "application/pdf", "x-amz-server-side-encryption": "aws:kms" },
  expires_at: "2026-09-01T00:05:00Z",
};
const ready = { id: "att_1", filename: "invoice.pdf", mime_type: "application/pdf", byte_length: "5", sha256: "0xabc" };

test("upload reserves a slot, PUTs the bytes with the presigned headers verbatim, then finalizes", async () => {
  const mock = mockFetch((url, init) => {
    if (url === "https://example.test/v1/attachments") return json(slot, 201);
    if (url === slot.upload_url) return new Response(null, { status: 200 });
    if (url === "https://example.test/v1/attachments/att_1/finalize") return json(ready);
    throw new Error(`unexpected ${init.method} ${url}`);
  });
  const client = new PaydayClient({ apiKey: "secret", baseUrl: "https://example.test", fetch: mock.fetch });
  const bytes = new Uint8Array([0x25, 0x50, 0x44, 0x46, 0x2d]);

  const descriptor = await client.attachments.upload(bytes, "invoice.pdf");

  assert.deepEqual(descriptor, ready);
  assert.equal(mock.calls.length, 3);
  const [create, put, finalize] = mock.calls;
  assert.equal(create.init.method, "POST");
  assert.deepEqual(JSON.parse(create.init.body), { filename: "invoice.pdf" });
  assert.equal(put.init.method, "PUT");
  // The presigned URL only admits a request whose headers match what was signed,
  // and it is a bucket URL: no Payday bearer may leak to it.
  assert.deepEqual(put.init.headers, slot.headers);
  assert.equal(put.init.headers.Authorization, undefined);
  assert.deepEqual(new Uint8Array(await put.init.body.arrayBuffer()), bytes);
  assert.equal(finalize.init.method, "POST");
  assert.equal(finalize.init.headers.Authorization, "Bearer secret");
});

test("upload retries finalize while the scan is pending and returns once admitted", async () => {
  let finalizeCalls = 0;
  const mock = mockFetch((url) => {
    if (url === "https://example.test/v1/attachments") return json(slot, 201);
    if (url === slot.upload_url) return new Response(null, { status: 200 });
    finalizeCalls += 1;
    return finalizeCalls === 1 ? apiError("attachment_scan_pending", 409) : json(ready);
  });
  const client = new PaydayClient({ apiKey: "secret", baseUrl: "https://example.test", fetch: mock.fetch });

  const descriptor = await client.attachments.upload(new Blob(["%PDF-"]), "invoice.pdf");

  assert.deepEqual(descriptor, ready);
  assert.equal(finalizeCalls, 2);
});

test("upload gives up with attachment_scan_timeout when the scan outlasts the bound", async () => {
  let finalizeCalls = 0;
  const mock = mockFetch((url) => {
    if (url === "https://example.test/v1/attachments") return json(slot, 201);
    if (url === slot.upload_url) return new Response(null, { status: 200 });
    finalizeCalls += 1;
    return apiError("attachment_scan_pending", 409);
  });
  const client = new PaydayClient({ apiKey: "secret", baseUrl: "https://example.test", fetch: mock.fetch });

  await assert.rejects(client.attachments.upload(new Blob(["%PDF-"]), "invoice.pdf", { scanTimeout: 0 }), (error) => {
    assert.ok(error instanceof PaydayError);
    assert.equal(error.code, "attachment_scan_timeout");
    assert.equal(error.requestId, "req-attachment_scan_pending");
    return true;
  });
  assert.equal(finalizeCalls, 1);
});

test("upload surfaces a rejection or a failed PUT as the API error, without retrying", async () => {
  let finalizeCalls = 0;
  const rejecting = mockFetch((url) => {
    if (url === "https://example.test/v1/attachments") return json(slot, 201);
    if (url === slot.upload_url) return new Response(null, { status: 200 });
    finalizeCalls += 1;
    return apiError("attachment_rejected", 422, "Not a PDF");
  });
  const client = new PaydayClient({ apiKey: "secret", baseUrl: "https://example.test", fetch: rejecting.fetch });
  await assert.rejects(client.attachments.upload(new Blob(["nope"]), "invoice.pdf"), (error) => {
    assert.ok(error instanceof PaydayError);
    assert.equal(error.code, "attachment_rejected");
    assert.equal(error.status, 422);
    assert.equal(error.message, "Not a PDF");
    return true;
  });
  assert.equal(finalizeCalls, 1);

  const failingPut = mockFetch((url) => {
    if (url === "https://example.test/v1/attachments") return json(slot, 201);
    if (url === slot.upload_url) return new Response("<Error/>", { status: 403 });
    throw new Error("finalize must not run after a failed PUT");
  });
  const other = new PaydayClient({ apiKey: "secret", baseUrl: "https://example.test", fetch: failingPut.fetch });
  await assert.rejects(other.attachments.upload(new Blob(["%PDF-"]), "invoice.pdf"), (error) => {
    assert.ok(error instanceof PaydayError);
    assert.equal(error.code, "attachment_upload_failed");
    assert.equal(error.status, 403);
    return true;
  });
  assert.equal(failingPut.calls.length, 2);
});

test("upload forwards an abort signal to every request and to the scan wait", async () => {
  const controller = new AbortController();
  const mock = mockFetch((url) => {
    if (url === "https://example.test/v1/attachments") return json(slot, 201);
    if (url === slot.upload_url) return new Response(null, { status: 200 });
    controller.abort(new Error("cancelled by the caller"));
    return apiError("attachment_scan_pending", 409);
  });
  const client = new PaydayClient({ apiKey: "secret", baseUrl: "https://example.test", fetch: mock.fetch });

  await assert.rejects(
    client.attachments.upload(new Blob(["%PDF-"]), "invoice.pdf", { signal: controller.signal }),
    /cancelled by the caller/,
  );
  for (const call of mock.calls) assert.equal(call.init.signal, controller.signal);
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

test("verification detail uses the payment sub-route", async () => {
  const detail = {
    payer_policy_mode: "verified_email",
    verification_completed_at: "2026-09-01T00:01:00Z",
    likely_unsolicited_at: null,
    facts: { email: "approved", merchant_session: "not_required", complete: true },
    attempts: [{
      id: "v1", kind: "email", status: "approved",
      verified_at: "2026-09-01T00:01:00Z", created_at: "2026-09-01T00:00:00Z",
    }],
  };
  const mock = mockFetch(() => json(detail));
  const client = new PaydayClient({ apiKey: "k", baseUrl: "https://example.test", fetch: mock.fetch });

  const read = await client.payments.verification("pay_a/b");
  assert.equal(read.attempts[0].status, "approved");
  assert.equal(read.facts.email, "approved");
  assert.deepEqual(mock.calls.map((call) => [call.init.method ?? "GET", call.url]), [
    ["GET", "https://example.test/v1/payments/pay_a%2Fb/verification"],
  ]);
  for (const call of mock.calls) assert.equal(call.init.headers.Authorization, "Bearer k");
});

test("a merchant-session payment returns its client secret once and mints more on request", async () => {
  const issued = {
    id: "pay_1",
    payment_url: "https://payday.sh/pay/pay_1",
    payer_policy: { mode: "merchant_session", payer_reference: "user_123" },
    client_secret: "cs_first",
    client_secret_expires_at: "2026-09-01T00:15:00Z",
  };
  const mock = mockFetch((url, init) => {
    if (url.endsWith("/client-secret")) return json({ client_secret: "cs_second", expires_at: "2026-09-01T01:15:00Z" }, 201);
    // The replay carries no secret; only the first response does.
    return json(init.headers["Idempotency-Key"] === "replay" ? { ...issued, client_secret: undefined, client_secret_expires_at: undefined } : issued, 201);
  });
  const client = new PaydayClient({ apiKey: "k", baseUrl: "https://example.test", fetch: mock.fetch });

  const created = await client.payments.create(
    { ...invoice, payer_policy: { mode: "merchant_session", payer_reference: "user_123" } },
    "first",
  );
  assert.equal(created.client_secret, "cs_first");
  assert.equal(checkoutUrl(created, created.client_secret), "https://payday.sh/pay/pay_1#cs=cs_first");
  assert.deepEqual(JSON.parse(mock.calls[0].init.body).payer_policy, { mode: "merchant_session", payer_reference: "user_123" });

  const replayed = await client.payments.create(
    { ...invoice, payer_policy: { mode: "merchant_session", payer_reference: "user_123" } },
    "replay",
  );
  assert.equal(replayed.client_secret, undefined);

  const minted = await client.payments.createClientSecret("pay_a/b");
  assert.equal(minted.client_secret, "cs_second");
  assert.deepEqual(mock.calls.slice(2).map((call) => [call.init.method, call.url]), [
    ["POST", "https://example.test/v1/payments/pay_a%2Fb/client-secret"],
  ]);
  assert.equal(mock.calls[2].init.body, undefined);
  assert.equal(mock.calls[2].init.headers.Authorization, "Bearer k");
  // The secret rides in the fragment, encoded, and never without a secret.
  assert.equal(checkoutUrl({ payment_url: "https://payday.sh/pay/pay_1" }, "cs_a+b"), "https://payday.sh/pay/pay_1#cs=cs_a%2Bb");
  assert.throws(() => checkoutUrl(created, ""), TypeError);
});

test("minting a client secret for the wrong mode surfaces the API's code", async () => {
  const mock = mockFetch(() => apiError("verification_method_not_applicable", 409));
  const client = new PaydayClient({ apiKey: "k", baseUrl: "https://example.test", fetch: mock.fetch });
  await assert.rejects(client.payments.createClientSecret("pay_1"), (error) => {
    assert.ok(error instanceof PaydayError);
    assert.equal(error.code, "verification_method_not_applicable");
    assert.equal(error.status, 409);
    return true;
  });
});

test("account.get reads with the client's own credential", async () => {
  const metadata = {
    account_id: "acc_1", key_hint: "…aB3f9Q", generation: 2, created_at: "2026-09-01T00:00:00Z",
    rotated_at: "2026-09-02T00:00:00Z", previous_key_expires_at: "2026-09-03T00:00:00Z", revoked_at: null,
  };
  const mock = mockFetch(() => json(metadata));
  const client = new PaydayClient({ accessToken: "session-token", baseUrl: "https://example.test", fetch: mock.fetch });

  const account = await client.account.get();

  assert.deepEqual(account, metadata);
  assert.equal(mock.calls[0].url, "https://example.test/v1/account");
  assert.equal(mock.calls[0].init.headers.Authorization, "Bearer session-token");
});

test("account.issueApiKey sends the given token, not the client's own credential", async () => {
  const issued = { api_key: "payday_live_abc", generation: 1, replaced_previous_key: false };
  const mock = mockFetch(() => json(issued, 201));
  // The client's own credential is a plain session; issuing needs a separate,
  // freshly completed sign-in, which is why the method takes its own token.
  const client = new PaydayClient({ accessToken: "session-token", baseUrl: "https://example.test", fetch: mock.fetch });

  const result = await client.account.issueApiKey("fresh-otp-token", 3);

  assert.deepEqual(result, issued);
  assert.equal(mock.calls[0].url, "https://example.test/v1/account/api-key");
  assert.equal(mock.calls[0].init.method, "POST");
  assert.equal(mock.calls[0].init.headers.Authorization, "Bearer fresh-otp-token");
  assert.deepEqual(JSON.parse(mock.calls[0].init.body), { expected_generation: 3 });
});

test("account.issueApiKey omits a generation as null, for a first key", async () => {
  const issued = { api_key: "payday_live_abc", generation: 1, replaced_previous_key: false };
  const mock = mockFetch(() => json(issued, 201));
  const client = new PaydayClient({ apiKey: "k", baseUrl: "https://example.test", fetch: mock.fetch });

  await client.account.issueApiKey("fresh-otp-token");

  assert.deepEqual(JSON.parse(mock.calls[0].init.body), { expected_generation: null });
});

test("account.revokeApiKey sends the given token and resolves on 204 No Content", async () => {
  const mock = mockFetch(() => new Response(null, { status: 204 }));
  const client = new PaydayClient({ apiKey: "k", baseUrl: "https://example.test", fetch: mock.fetch });

  const result = await client.account.revokeApiKey("fresh-otp-token", 2);

  assert.equal(result, undefined);
  assert.equal(mock.calls[0].url, "https://example.test/v1/account/api-key");
  assert.equal(mock.calls[0].init.method, "DELETE");
  assert.equal(mock.calls[0].init.headers.Authorization, "Bearer fresh-otp-token");
  assert.deepEqual(JSON.parse(mock.calls[0].init.body), { expected_generation: 2 });
});

test("account.issueApiKey surfaces a generation conflict as a typed PaydayError", async () => {
  const mock = mockFetch(() => apiError("api_key_generation_conflict", 409));
  const client = new PaydayClient({ apiKey: "k", baseUrl: "https://example.test", fetch: mock.fetch });

  await assert.rejects(client.account.issueApiKey("fresh-otp-token", 1), (error) => {
    assert.ok(error instanceof PaydayError);
    assert.equal(error.code, "api_key_generation_conflict");
    assert.equal(error.status, 409);
    return true;
  });
});
