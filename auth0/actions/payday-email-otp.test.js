const assert = require("node:assert/strict");
const test = require("node:test");

const { onExecutePostLogin } = require("./payday-email-otp");

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
    resource_server: { identifier: "https://api.payday.sh" },
    secrets: {
      PAYDAY_API_AUDIENCE: "https://api.payday.sh",
      PAYDAY_CLIENT_ID: "payday-dashboard",
    },
    client: { client_id: "payday-dashboard" },
    connection: { strategy: "email" },
    authentication: {
      methods: [{ name: "email", timestamp: new Date().toISOString() }],
    },
    user: { email: "merchant@example.com", email_verified: true },
    ...overrides,
  };
}

test("adds fresh email OTP claims for the dashboard client", async () => {
  const result = actionApi();
  await onExecutePostLogin(event(), result.api);

  assert.equal(result.denial(), undefined);
  assert.equal(
    result.claims.get("https://api.payday.sh/auth/method"),
    "email_otp",
  );
  assert.equal(result.claims.get("https://api.payday.sh/auth/email"), "merchant@example.com");
  assert.match(
    result.claims.get("https://api.payday.sh/auth/event_id"),
    /^[0-9a-f-]{36}$/,
  );
  assert.deepEqual(
    [...result.claims.keys()].sort(),
    [
      "https://api.payday.sh/auth/authenticated_at",
      "https://api.payday.sh/auth/client_id",
      "https://api.payday.sh/auth/email",
      "https://api.payday.sh/auth/event_id",
      "https://api.payday.sh/auth/method",
    ],
  );
  assert.equal(
    result.claims.get("https://api.payday.sh/auth/client_id"),
    "payday-dashboard",
  );
});

test("denies an unknown client, another authentication method, or a missing client", async () => {
  const secretsWithoutClient = {
    PAYDAY_API_AUDIENCE: "https://api.payday.sh",
  };
  for (const invalid of [
    event({ client: { client_id: "other-client" } }),
    event({ connection: { strategy: "google-oauth2" } }),
    event({ authentication: { methods: [{ name: "federated" }] } }),
    // The client secret is not configured yet: the dashboard must stay out.
    event({ secrets: secretsWithoutClient, client: { client_id: "payday-dashboard" } }),
    // No client at all must not match an unset secret.
    event({ secrets: secretsWithoutClient, client: {} }),
  ]) {
    const result = actionApi();
    await onExecutePostLogin(invalid, result.api);
    assert.match(result.denial(), /fresh email OTP/);
    assert.equal(result.claims.size, 0);
  }
});

test("does not affect tokens for unrelated APIs", async () => {
  const result = actionApi();
  await onExecutePostLogin(
    event({ resource_server: { identifier: "https://unrelated.example" } }),
    result.api,
  );

  assert.equal(result.denial(), undefined);
  assert.equal(result.claims.size, 0);
});
