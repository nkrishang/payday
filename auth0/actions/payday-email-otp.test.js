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
      PAYDAY_CLIENT_ID: "payday-cli",
    },
    client: { client_id: "payday-cli" },
    connection: { strategy: "email" },
    authentication: {
      methods: [{ name: "email", timestamp: new Date().toISOString() }],
    },
    ...overrides,
  };
}

test("adds fresh email OTP claims for the Payday client", async () => {
  const result = actionApi();
  await onExecutePostLogin(event(), result.api);

  assert.equal(result.denial(), undefined);
  assert.equal(
    result.claims.get("https://api.payday.sh/auth/method"),
    "email_otp",
  );
  assert.equal(
    result.claims.get("https://api.payday.sh/auth/client_id"),
    "payday-cli",
  );
  assert.match(
    result.claims.get("https://api.payday.sh/auth/event_id"),
    /^[0-9a-f-]{36}$/,
  );
});

test("denies another client or authentication method", async () => {
  for (const invalid of [
    event({ client: { client_id: "other-client" } }),
    event({ connection: { strategy: "google-oauth2" } }),
    event({ authentication: { methods: [{ name: "federated" }] } }),
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
