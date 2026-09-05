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

const requirementsNone = { email: "not_required", merchant_session: "not_required", complete: true };

const payerPayment = {
  id: "dr_0198f80c-8d2f-7dc1-a369-90556a64f700",
  issuer_name: "Acme LLC",
  heading: "March retainer",
  payer_policy: { mode: "permissionless", expected_email_hint: null },
  requirements: requirementsNone,
  status: "awaiting_deposit",
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
  deposit_uri: "ethereum:0x754704Bc059F8C67012fEd69BC8A327a5aafb603@143/transfer?address=0x1111111111111111111111111111111111111111&uint256=25000000",
  details: {
    amount: "25.00", amount_base_units: "25000000",
    payer: { name: "Customer Inc" }, notes: null, reference: "INV-1042",
    attachment: { id: "att_1", filename: "request.pdf", mime_type: "application/pdf", byte_length: "5", sha256: "0xabc" },
  },
};

/** What a gated deposit request looks like before its payer has verified anything. */
const lockedPayment = {
  id: payerPayment.id,
  issuer_name: "Acme LLC",
  heading: "March retainer",
  payer_policy: { mode: "verified_email", expected_email_hint: "a****@e***.com" },
  requirements: { ...requirementsNone, email: "pending", complete: false },
  status: "awaiting_deposit",
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
  address: null, address_explorer_url: null, deposit_uri: null,
  details: null,
};

test("payer reads use the public route and send no authorization", async () => {
  const mock = mockFetch(() => new Response(JSON.stringify(payerPayment), {
    headers: { "content-type": "application/json" },
  }));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test/", fetch: mock.fetch });

  const payment = await client.depositRequests.get(payerPayment.id);

  assert.equal(payment.remaining_base_units, "25000000");
  assert.equal(payment.payable, true);
  assert.equal(payment.content_unlocked, true);
  assert.equal(payment.details.reference, "INV-1042");
  assert.equal(mock.calls[0].url, `https://example.test/v1/payer/deposit-requests/${payerPayment.id}`);
  assert.equal(mock.calls[0].init.method, "GET");
  assert.equal(mock.calls[0].init.headers.Authorization, undefined);
  assert.equal(mock.calls[0].init.headers["Payday-Payer-Session"], undefined);
  assert.equal(mock.calls[0].init.cache, "no-store");
});

test("a locked gated deposit request carries null mechanics and no invoice content", async () => {
  const mock = mockFetch(() => new Response(JSON.stringify(lockedPayment)));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test", fetch: mock.fetch });

  const payment = await client.depositRequests.get(lockedPayment.id);

  assert.equal(payment.content_unlocked, false);
  assert.equal(payment.issuer_name, "Acme LLC");
  assert.equal(payment.payer_policy.expected_email_hint, "a****@e***.com");
  assert.equal(payment.requirements.complete, false);
  for (const field of [
    "chain", "token", "amount", "amount_base_units", "received", "received_base_units",
    "remaining", "remaining_base_units", "address", "address_explorer_url", "deposit_uri", "details",
  ]) {
    assert.equal(payment[field], null, `${field} must be withheld while locked`);
  }
});

test("payer reads forward a session token and an abort signal, and encode the id", async () => {
  const mock = mockFetch(() => new Response(JSON.stringify(payerPayment)));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test", fetch: mock.fetch });
  const controller = new AbortController();

  await client.depositRequests.get("dr_a/b", { signal: controller.signal, payerSession: "pps_token" });

  assert.equal(mock.calls[0].url, "https://example.test/v1/payer/deposit-requests/dr_a%2Fb");
  assert.equal(mock.calls[0].init.signal, controller.signal);
  assert.equal(mock.calls[0].init.headers["Payday-Payer-Session"], "pps_token");
  assert.equal(mock.calls[0].init.headers.Authorization, undefined);
});

test("payer attachment reads use the public sub-route with an optional session", async () => {
  const mock = mockFetch(() => new Response(JSON.stringify(payerPayment.details.attachment)));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test", fetch: mock.fetch });

  const open = await client.depositRequests.attachment("dr_a/b");
  await client.depositRequests.attachment("dr_a/b", "pps_token");

  assert.equal(open.filename, "request.pdf");
  assert.equal(mock.calls[0].url, "https://example.test/v1/payer/deposit-requests/dr_a%2Fb/attachment");
  assert.equal(mock.calls[0].init.method, "GET");
  assert.equal(mock.calls[0].init.headers["Payday-Payer-Session"], undefined);
  assert.equal(mock.calls[1].init.headers["Payday-Payer-Session"], "pps_token");
});

test("a locked attachment surfaces verification_required", async () => {
  const mock = mockFetch(() => new Response(JSON.stringify({
    error: { code: "verification_required", message: "Complete verification first" }, request_id: "req-7",
  }), { status: 401 }));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test", fetch: mock.fetch });

  await assert.rejects(client.depositRequests.attachment("dr_1"), (error) => {
    assert.ok(error instanceof PaydayError);
    assert.equal(error.code, "verification_required");
    assert.equal(error.status, 401);
    return true;
  });
});

test("the QR is fetched as a blob with the session in a header, never the URL", async () => {
  const mock = mockFetch(() => new Response("<svg/>", { headers: { "content-type": "image/svg+xml" } }));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test/", fetch: mock.fetch });

  const svg = await client.depositRequests.qr("dr_a/b", "pps_token");

  assert.equal(await svg.text(), "<svg/>");
  assert.equal(mock.calls[0].url, "https://example.test/v1/payer/deposit-requests/dr_a%2Fb/qr");
  assert.equal(mock.calls[0].init.headers["Payday-Payer-Session"], "pps_token");
  assert.equal(mock.calls[0].init.headers.Accept, "image/svg+xml");
  assert.ok(!mock.calls[0].url.includes("pps_token"));
});

test("payer client defaults to the production API origin", async () => {
  const mock = mockFetch(() => new Response(JSON.stringify(payerPayment)));
  const client = new PaydayPayerClient({ fetch: mock.fetch });
  await client.depositRequests.get("dr_1");
  assert.equal(mock.calls[0].url, "https://api.payday.sh/v1/payer/deposit-requests/dr_1");
});

test("email verification starts without naming a mailbox and confirms with the session", async () => {
  const status = { requirements: { ...requirementsNone, email: "approved" } };
  const mock = mockFetch((url) => new Response(JSON.stringify(
    url.endsWith("/start") ? { payer_session: "pps_new", expires_at: "2026-09-02T00:00:00Z" } : status,
  )));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test", fetch: mock.fetch });

  const started = await client.verification.startEmail("dr_a/b");
  const resent = await client.verification.startEmail("dr_a/b", { payerSession: started.payer_session });
  const confirmed = await client.verification.confirmEmail("dr_a/b", "123456", started.payer_session);
  const current = await client.verification.status("dr_a/b", { payerSession: started.payer_session });

  assert.equal(started.payer_session, "pps_new");
  assert.equal(resent.payer_session, "pps_new");
  assert.equal(confirmed.requirements.email, "approved");
  assert.equal(current.requirements.complete, true);
  assert.equal(mock.calls[0].url, "https://example.test/v1/payer/deposit-requests/dr_a%2Fb/verify/email/start");
  assert.equal(mock.calls[0].init.method, "POST");
  assert.equal(mock.calls[0].init.body, undefined);
  assert.equal(mock.calls[0].init.headers["Payday-Payer-Session"], undefined);
  assert.equal(mock.calls[1].init.headers["Payday-Payer-Session"], "pps_new");
  assert.equal(mock.calls[2].url, "https://example.test/v1/payer/deposit-requests/dr_a%2Fb/verify/email/confirm");
  assert.deepEqual(JSON.parse(mock.calls[2].init.body), { otp: "123456" });
  assert.equal(mock.calls[2].init.headers["Payday-Payer-Session"], "pps_new");
  assert.equal(mock.calls[3].url, "https://example.test/v1/payer/deposit-requests/dr_a%2Fb/verify");
  assert.equal(mock.calls[3].init.method, "GET");
  for (const call of mock.calls) assert.equal(call.init.headers.Authorization, undefined);
});

test("email persistence continuation is exposed and can be retried without an OTP", async () => {
  const calls = [];
  const client = new PaydayPayerClient({
    baseUrl: "https://example.test",
    fetch: async (_url, init) => {
      calls.push(JSON.parse(init.body));
      if (calls.length === 1) return new Response(JSON.stringify({
        error: { code: "verification_persistence_unavailable", message: "retry" },
        continuation: "signed-proof",
      }), { status: 503 });
      return new Response(JSON.stringify({ requirements: { email: "approved", complete: true } }));
    },
  });
  let proof;
  await assert.rejects(client.verification.confirmEmail("dr_1", "123456", "pps"), (error) => {
    proof = error.continuation;
    return error.code === "verification_persistence_unavailable" && proof === "signed-proof";
  });
  await client.verification.continueEmail("dr_1", proof, "pps");
  assert.deepEqual(calls, [{ otp: "123456" }, { continuation: "signed-proof" }]);
});

test("a wrong code and a cooled-down resend surface their codes", async () => {
  const mock = mockFetch((url) => new Response(JSON.stringify({
    error: url.endsWith("/confirm")
      ? { code: "otp_invalid", message: "The code was not accepted" }
      : { code: "otp_resend_cooldown", message: "Request another in 42 seconds" },
  }), { status: url.endsWith("/confirm") ? 401 : 429, headers: { "retry-after": "42" } }));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test", fetch: mock.fetch });

  await assert.rejects(client.verification.confirmEmail("dr_1", "000000", "pps"), (error) => {
    assert.ok(error instanceof PaydayError);
    assert.equal(error.code, "otp_invalid");
    return true;
  });
  await assert.rejects(client.verification.startEmail("dr_1", { payerSession: "pps" }), (error) => {
    assert.equal(error.code, "otp_resend_cooldown");
    assert.equal(error.status, 429);
    return true;
  });
});

test("a client secret is exchanged once for a session, and the failures are told apart", async () => {
  const opened = {
    payer_session: "pps_opened",
    expires_at: "2026-09-02T00:00:00Z",
    requirements: { ...requirementsNone, merchant_session: "approved" },
  };
  let exchanges = 0;
  const mock = mockFetch(() => {
    exchanges += 1;
    if (exchanges === 1) return new Response(JSON.stringify(opened), { status: 200 });
    if (exchanges === 2) return new Response(JSON.stringify({
      error: { code: "client_secret_used", message: "This link was already opened" }, request_id: "req-2",
    }), { status: 409 });
    return new Response(JSON.stringify({
      error: { code: "client_secret_invalid", message: "not valid" }, request_id: "req-3",
    }), { status: 401 });
  });
  const client = new PaydayPayerClient({ baseUrl: "https://example.test", fetch: mock.fetch });

  const session = await client.verification.exchangeClientSecret("dr_a/b", "cs_secret");
  assert.equal(session.payer_session, "pps_opened");
  assert.equal(session.requirements.merchant_session, "approved");
  assert.equal(session.requirements.complete, true);
  assert.equal(mock.calls[0].url, "https://example.test/v1/payer/deposit-requests/dr_a%2Fb/session");
  assert.equal(mock.calls[0].init.method, "POST");
  assert.deepEqual(JSON.parse(mock.calls[0].init.body), { client_secret: "cs_secret" });
  assert.equal(mock.calls[0].init.headers.Authorization, undefined);
  assert.equal(mock.calls[0].init.headers["Payday-Payer-Session"], undefined);

  await assert.rejects(client.verification.exchangeClientSecret("dr_a/b", "cs_secret"), (error) => {
    assert.ok(error instanceof PaydayError);
    assert.equal(error.code, "client_secret_used");
    assert.equal(error.status, 409);
    return true;
  });
  await assert.rejects(client.verification.exchangeClientSecret("dr_a/b", "cs_other"), (error) => {
    assert.equal(error.code, "client_secret_invalid");
    assert.equal(error.status, 401);
    return true;
  });
});

test("an unknown or malformed link surfaces invalid_deposit_link", async () => {
  const mock = mockFetch(() => new Response(JSON.stringify({
    error: { code: "invalid_deposit_link", message: "Deposit link is not valid" }, request_id: "req-9",
  }), { status: 401 }));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test", fetch: mock.fetch });

  await assert.rejects(client.depositRequests.get("dr_missing"), (error) => {
    assert.ok(error instanceof PaydayError);
    assert.equal(error.code, "invalid_deposit_link");
    assert.equal(error.status, 401);
    assert.equal(error.requestId, "req-9");
    return true;
  });
});


test("the wallet step mints a challenge for the wallet and submits its signature with the session", async () => {
  const challenge = {
    payer_session: "pps_wallet",
    expires_at: "2026-09-02T00:10:00Z",
    typed_data: { primaryType: "PayerAttestation", domain: {}, types: {}, message: {} },
  };
  const bound = { ...payerPayment, payer_wallet: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8" };
  const mock = mockFetch((url) => new Response(JSON.stringify(url.endsWith("/challenge") ? challenge : bound)));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test", fetch: mock.fetch });

  const minted = await client.wallet.challenge("dr_a/b", "0x70997970C51812dc3A010C7d01b50e0d17dc79C8");
  const attested = await client.wallet.attest(
    "dr_a/b",
    "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
    `0x${"ab".repeat(65)}`,
    minted.payer_session,
  );

  assert.equal(minted.typed_data.primaryType, "PayerAttestation");
  assert.equal(attested.payer_wallet, "0x70997970C51812dc3A010C7d01b50e0d17dc79C8");
  assert.equal(mock.calls[0].url, "https://example.test/v1/payer/deposit-requests/dr_a%2Fb/wallet/challenge");
  assert.equal(mock.calls[0].init.method, "POST");
  assert.deepEqual(JSON.parse(mock.calls[0].init.body), { wallet: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8" });
  assert.equal(mock.calls[0].init.headers["Payday-Payer-Session"], undefined);
  assert.equal(mock.calls[1].url, "https://example.test/v1/payer/deposit-requests/dr_a%2Fb/wallet/attest");
  assert.deepEqual(JSON.parse(mock.calls[1].init.body), {
    wallet: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
    signature: `0x${"ab".repeat(65)}`,
  });
  assert.equal(mock.calls[1].init.headers["Payday-Payer-Session"], "pps_wallet");
  for (const call of mock.calls) assert.equal(call.init.headers.Authorization, undefined);
});

test("a wallet that is already bound surfaces wallet_already_bound", async () => {
  const mock = mockFetch(() => new Response(
    JSON.stringify({ error: { code: "wallet_already_bound", message: "bound" } }),
    { status: 409 },
  ));
  const client = new PaydayPayerClient({ baseUrl: "https://example.test", fetch: mock.fetch });
  await assert.rejects(client.wallet.challenge("dr_1", "0x70997970C51812dc3A010C7d01b50e0d17dc79C8"), (error) => {
    assert.ok(error instanceof PaydayError);
    assert.equal(error.code, "wallet_already_bound");
    assert.equal(error.status, 409);
    return true;
  });
});
