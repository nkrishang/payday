const assert = require("node:assert/strict");
const test = require("node:test");

const { onExecutePostLogin } = require("./gum-email-otp");

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
    resource_server: { identifier: "https://api.gum.money" },
    secrets: {
      GUM_API_AUDIENCE: "https://api.gum.money",
      GUM_CLIENT_ID: "gum-dashboard",
    },
    client: { client_id: "gum-dashboard" },
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
    result.claims.get("https://api.gum.money/auth/method"),
    "email_otp",
  );
  assert.equal(result.claims.get("https://api.gum.money/auth/email"), "merchant@example.com");
  assert.match(
    result.claims.get("https://api.gum.money/auth/event_id"),
    /^[0-9a-f-]{36}$/,
  );
  assert.deepEqual(
    [...result.claims.keys()].sort(),
    [
      "https://api.gum.money/auth/authenticated_at",
      "https://api.gum.money/auth/client_id",
      "https://api.gum.money/auth/email",
      "https://api.gum.money/auth/event_id",
      "https://api.gum.money/auth/method",
    ],
  );
  assert.equal(
    result.claims.get("https://api.gum.money/auth/client_id"),
    "gum-dashboard",
  );
});

test("denies an unknown client, another authentication method, or a missing client", async () => {
  const secretsWithoutClient = {
    GUM_API_AUDIENCE: "https://api.gum.money",
  };
  for (const invalid of [
    event({ client: { client_id: "other-client" } }),
    event({ connection: { strategy: "google-oauth2" } }),
    event({ authentication: { methods: [{ name: "federated" }] } }),
    // The client secret is not configured yet: the dashboard must stay out.
    event({ secrets: secretsWithoutClient, client: { client_id: "gum-dashboard" } }),
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
