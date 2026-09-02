const assert = require("node:assert/strict");
const test = require("node:test");

const { onExecutePostLogin } = require("./payday-payer-email-otp");

const PAYER_AUDIENCE = "https://api.payday.sh/payer";
const MERCHANT_AUDIENCE = "https://api.payday.sh";

const EXPECTED_CLAIMS = [
  "https://api.payday.sh/auth/authenticated_at",
  "https://api.payday.sh/auth/client_id",
  "https://api.payday.sh/auth/email",
  "https://api.payday.sh/auth/event_id",
  "https://api.payday.sh/auth/method",
];

function actionApi() {
  const claims = new Map();
  let denial;
  return {
    api: {
      access: { deny: (reason) => (denial = reason) },
      accessToken: { setCustomClaim: (name, value) => claims.set(name, value) },
    },
    claims,
    denial: () => denial,
  };
}

function event(overrides = {}) {
  return {
    resource_server: { identifier: PAYER_AUDIENCE },
    secrets: {
      PAYDAY_PAYER_AUDIENCE: PAYER_AUDIENCE,
      PAYDAY_PAYER_CLIENT_ID: "payday-payer",
    },
    client: { client_id: "payday-payer" },
    connection: { strategy: "email" },
    authentication: {
      methods: [{ name: "email", timestamp: new Date().toISOString() }],
    },
    user: { email: "payer@example.com", email_verified: true },
    ...overrides,
  };
}

test("accepts the payer client on the payer audience and sets exactly the five claims", async () => {
  const result = actionApi();
  await onExecutePostLogin(event(), result.api);

  assert.equal(result.denial(), undefined);
  assert.deepEqual([...result.claims.keys()].sort(), EXPECTED_CLAIMS);
  assert.equal(
    result.claims.get("https://api.payday.sh/auth/method"),
    "email_otp",
  );
  assert.equal(
    result.claims.get("https://api.payday.sh/auth/client_id"),
    "payday-payer",
  );
  assert.equal(
    result.claims.get("https://api.payday.sh/auth/email"),
    "payer@example.com",
  );
  assert.match(
    result.claims.get("https://api.payday.sh/auth/event_id"),
    /^[0-9a-f-]{36}$/,
  );
  const authenticatedAt = result.claims.get(
    "https://api.payday.sh/auth/authenticated_at",
  );
  assert.ok(Math.abs(Date.now() / 1000 - authenticatedAt) < 5);
});

test("denies the merchant CLI and dashboard clients on the payer audience", async () => {
  for (const clientId of ["payday-cli", "payday-dashboard"]) {
    const result = actionApi();
    await onExecutePostLogin(
      event({ client: { client_id: clientId } }),
      result.api,
    );
    assert.match(result.denial(), /fresh email OTP/);
    assert.equal(result.claims.size, 0);
  }
});

test("denies a stale email authentication, another connection, or a missing client", async () => {
  const sixMinutesAgo = new Date(Date.now() - 6 * 60 * 1000).toISOString();
  for (const invalid of [
    event({
      authentication: { methods: [{ name: "email", timestamp: sixMinutesAgo }] },
    }),
    event({ authentication: { methods: [{ name: "federated" }] } }),
    event({ connection: { strategy: "google-oauth2" } }),
    event({ user: { email: "payer@example.com", email_verified: false } }),
    event({ client: {} }),
    // The payer client secret is not configured yet: nobody is admitted.
    event({ secrets: { PAYDAY_PAYER_AUDIENCE: PAYER_AUDIENCE } }),
  ]) {
    const result = actionApi();
    await onExecutePostLogin(invalid, result.api);
    assert.match(result.denial(), /fresh email OTP/);
    assert.equal(result.claims.size, 0);
  }
});

test("sets the email trimmed and lowercased so gatewayd can compare it", async () => {
  const result = actionApi();
  await onExecutePostLogin(
    event({ user: { email: "  Alice.Payer@Example.COM ", email_verified: true } }),
    result.api,
  );

  assert.equal(result.denial(), undefined);
  assert.equal(
    result.claims.get("https://api.payday.sh/auth/email"),
    "alice.payer@example.com",
  );
});

test("ignores the merchant API and any other audience, whatever the client", async () => {
  for (const identifier of [MERCHANT_AUDIENCE, "https://unrelated.example"]) {
    for (const clientId of ["payday-payer", "payday-cli", "other-client"]) {
      const result = actionApi();
      await onExecutePostLogin(
        event({
          resource_server: { identifier },
          client: { client_id: clientId },
        }),
        result.api,
      );
      assert.equal(result.denial(), undefined);
      assert.equal(result.claims.size, 0);
    }
  }
});

test("does nothing at all while the payer audience secret is unset", async () => {
  for (const resourceServer of [
    { identifier: PAYER_AUDIENCE },
    undefined,
  ]) {
    const result = actionApi();
    await onExecutePostLogin(
      event({
        secrets: { PAYDAY_PAYER_CLIENT_ID: "payday-payer" },
        resource_server: resourceServer,
      }),
      result.api,
    );
    assert.equal(result.denial(), undefined);
    assert.equal(result.claims.size, 0);
  }
});
