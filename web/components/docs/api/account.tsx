import type { EndpointGroup } from "./types";

const ACCOUNT = `{
  "account_id": "acct_0198f80c-6666-7dc1-a369-90556a64f700",
  "email": "ops@acme.example",
  "wallet_address": "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC",
  "key_hint": "…aB3f9Q",
  "generation": 3,
  "created_at": "2026-08-01T09:00:00Z",
  "rotated_at": "2026-09-06T12:00:00Z",
  "previous_key_expires_at": "2026-09-07T12:00:00Z",
  "revoked_at": null
}`;

export const ACCOUNT_GROUP: EndpointGroup = {
  slug: "account",
  title: "Account",
  endpoints: [
    {
      slug: "get",
      title: "Get the account",
      method: "GET",
      path: "/v1/account",
      auth: "key",
      summary:
        "Who you are, where you settle by default, and the state of your key. Never the key itself.",
      response: {
        fields: [
          { name: "account_id", type: "acct_ id", description: "The account." },
          {
            name: "email",
            type: "string | null",
            description: "The mailbox the account signs in with.",
          },
          {
            name: "wallet_address",
            type: "string | null",
            description:
              "The account's own EVM wallet, created at first sign-in, where deposits settle by default.",
          },
          {
            name: "key_hint",
            type: "string | null",
            description: "The key's last six characters, or null with no active key.",
          },
          {
            name: "generation",
            type: "integer",
            description: "Increments on every issuance or rotation; guards the key routes.",
          },
          {
            name: "created_at, rotated_at, revoked_at",
            type: "timestamp | null",
            description: "Key milestones.",
          },
          {
            name: "previous_key_expires_at",
            type: "timestamp | null",
            description: "Set for 24 hours after a rotation, while the replaced key still works.",
          },
        ],
      },
      examples: {
        curl: `curl -fsS "$API/v1/account" -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const account = await payday.account.get();`,
        response: ACCOUNT,
      },
    },
    {
      slug: "issue-api-key",
      title: "Issue or rotate the API key",
      method: "POST",
      path: "/v1/account/api-key",
      auth: "session",
      summary:
        "Issue a first key, or rotate the current one. The plaintext is returned once. A rotation keeps the previous key working for 24 hours.",
      body: (
        <p>
          Only a signed-in dashboard session may call this. An API key, however valid elsewhere, is
          refused, so whoever holds a key can never widen or prolong their own access. Pass the
          generation the account currently reports; a mismatch means something else changed the key
          first.
        </p>
      ),
      bodyFields: [
        {
          name: "expected_generation",
          type: "integer",
          required: true,
          description: "The account's current generation.",
        },
      ],
      response: {
        fields: [
          {
            name: "api_key",
            type: "string",
            description: "Shown once. Nothing later can return it.",
          },
          { name: "generation", type: "integer", description: "The new generation." },
          { name: "replaced_previous_key", type: "boolean", description: "True on a rotation." },
        ],
      },
      answers: [
        { status: 201, when: "The first key." },
        { status: 200, when: "A rotation." },
        { status: 401, code: "identity_unauthorized", when: "An API key was presented." },
        {
          status: 409,
          code: "api_key_generation_conflict",
          when: "The generation moved. Re-read the account and decide again.",
        },
      ],
      examples: {
        curl: `curl -fsS "$API/v1/account/api-key" \\
  -H "Authorization: Bearer $DASHBOARD_SESSION_TOKEN" \\
  -H "Content-Type: application/json" \\
  -d '{ "expected_generation": 3 }'`,
        ts: `const payday = new PaydayClient({ accessToken: identityToken }); // a session, not a key
const issued = await payday.account.issueApiKey(account.generation);
issued.api_key; // shown once`,
        response: `{
  "api_key": "payday_live_…",
  "generation": 4,
  "replaced_previous_key": true
}`,
      },
    },
    {
      slug: "revoke-api-key",
      title: "Revoke the API key",
      method: "DELETE",
      path: "/v1/account/api-key",
      auth: "session",
      summary:
        "Immediately invalidate the current key and any key still in its rotation grace window. A later issuance starts a new generation.",
      bodyFields: [
        {
          name: "expected_generation",
          type: "integer",
          required: true,
          description: "The account's current generation.",
        },
      ],
      response: { description: "204 No Content." },
      answers: [
        { status: 401, code: "identity_unauthorized", when: "An API key was presented." },
        { status: 409, code: "api_key_generation_conflict", when: "The generation moved." },
      ],
      examples: {
        curl: `curl -fsS -X DELETE "$API/v1/account/api-key" \\
  -H "Authorization: Bearer $DASHBOARD_SESSION_TOKEN" \\
  -H "Content-Type: application/json" \\
  -d '{ "expected_generation": 4 }'`,
        ts: `await payday.account.revokeApiKey(account.generation);`,
        response: `HTTP/1.1 204 No Content`,
        responseLang: "http",
      },
    },
  ],
};

export const STATUS_GROUP: EndpointGroup = {
  slug: "status",
  title: "Status",
  endpoints: [
    {
      slug: "get",
      title: "Get service status",
      method: "GET",
      path: "/v1/status",
      auth: "key",
      summary:
        "Chain, indexer, and settlement-queue health. Use it when a payer can see a transaction that a request does not yet reflect.",
      response: {
        fields: [
          {
            name: "chain",
            type: "object",
            description: "{ id, name, finalized_block, finalized_at }.",
          },
          {
            name: "indexer",
            type: "object",
            description:
              "{ cursor_block, cursor_at, lag_blocks }: how far behind the finalized head the indexer is.",
          },
          {
            name: "sweeper",
            type: "object",
            description: "{ state, queued }: the settlement queue.",
          },
        ],
      },
      answers: [{ status: 503, when: "The state cannot be read." }],
      examples: {
        curl: `curl -fsS "$API/v1/status" -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const status = await payday.status();
status.indexer.lag_blocks; // 2`,
        response: `{
  "chain": { "id": "143", "name": "Monad", "finalized_block": "98765432", "finalized_at": "2026-09-06T12:00:00Z" },
  "indexer": { "cursor_block": "98765430", "cursor_at": "2026-09-06T12:00:01Z", "lag_blocks": 2 },
  "sweeper": { "state": "idle", "queued": 0 }
}`,
      },
    },
    {
      slug: "health",
      title: "Health check",
      method: "GET",
      path: "/health",
      auth: "none",
      summary: "Unauthenticated readiness: ok when the API can reach its database.",
      response: { description: "Plain text." },
      answers: [
        { status: 200, when: "ok" },
        { status: 503, when: "database unavailable" },
      ],
      examples: {
        curl: `curl -fsS "$API/health"`,
        response: `ok`,
        responseLang: "text",
      },
    },
  ],
};
