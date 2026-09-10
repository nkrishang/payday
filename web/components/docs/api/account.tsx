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
      summary: "Retrieves the account and its key state. The key itself is never returned.",
      response: {
        fields: [
          { name: "account_id", type: "acct_ id", description: "" },
          { name: "email", type: "string | null", description: "Sign-in mailbox." },
          {
            name: "wallet_address",
            type: "string | null",
            description: "Account wallet, EIP-55. Default settlement destination.",
          },
          {
            name: "key_hint",
            type: "string | null",
            description: "Last six characters of the active key.",
          },
          {
            name: "generation",
            type: "integer",
            description: "Increments per issuance or rotation.",
          },
          { name: "created_at, rotated_at, revoked_at", type: "timestamp | null", description: "" },
          {
            name: "previous_key_expires_at",
            type: "timestamp | null",
            description: "End of the 24-hour rotation grace window.",
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
      summary: "Issues the first key, or rotates the current one. Plaintext returned once.",
      body: (
        <p>
          Dashboard session only. On rotation the previous key remains valid for 24 hours.{" "}
          <code>expected_generation</code> must equal the account&apos;s current{" "}
          <code>generation</code>.
        </p>
      ),
      bodyFields: [
        { name: "expected_generation", type: "integer", required: true, description: "" },
      ],
      response: {
        fields: [
          { name: "api_key", type: "string", description: "Plaintext. Once." },
          { name: "generation", type: "integer", description: "" },
          { name: "replaced_previous_key", type: "boolean", description: "" },
        ],
      },
      answers: [
        { status: 201, when: "First issuance." },
        { status: 200, when: "Rotation." },
        { status: 401, code: "identity_unauthorized", when: "API key presented." },
        { status: 409, code: "api_key_generation_conflict", when: "Generation mismatch." },
      ],
      examples: {
        curl: `curl -fsS "$API/v1/account/api-key" \\
  -H "Authorization: Bearer $DASHBOARD_SESSION_TOKEN" \\
  -H "Content-Type: application/json" \\
  -d '{ "expected_generation": 3 }'`,
        ts: `const payday = new PaydayClient({ accessToken: identityToken });
const issued = await payday.account.issueApiKey(account.generation);`,
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
      summary: "Revokes the current key and any key in its grace window. Immediate.",
      bodyFields: [
        { name: "expected_generation", type: "integer", required: true, description: "" },
      ],
      response: { description: "204 No Content." },
      answers: [
        { status: 401, code: "identity_unauthorized", when: "API key presented." },
        { status: 409, code: "api_key_generation_conflict", when: "Generation mismatch." },
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
      summary: "Reports every network's finalized position, indexer cursor, and settlement queue.",
      response: {
        fields: [
          {
            name: "chains",
            type: "array",
            description:
              "One entry per supported network: { id, name, finalized_block, finalized_at, indexer: { cursor_block, cursor_at, lag_blocks }, sweeper: { state, queued } }.",
          },
        ],
      },
      answers: [{ status: 503, when: "State unreadable." }],
      examples: {
        curl: `curl -fsS "$API/v1/status" -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const status = await payday.status();`,
        response: `{
  "chains": [
    {
      "id": "143", "name": "Monad", "finalized_block": "98765432", "finalized_at": "2026-09-06T12:00:00Z",
      "indexer": { "cursor_block": "98765430", "cursor_at": "2026-09-06T12:00:01Z", "lag_blocks": 2 },
      "sweeper": { "state": "idle", "queued": 0 }
    },
    {
      "id": "8453", "name": "Base", "finalized_block": "35012345", "finalized_at": "2026-09-06T12:00:00Z",
      "indexer": { "cursor_block": "35012345", "cursor_at": "2026-09-06T11:58:30Z", "lag_blocks": 0 },
      "sweeper": { "state": "idle", "queued": 0 }
    },
    {
      "id": "42161", "name": "Arbitrum One", "finalized_block": "380123456", "finalized_at": "2026-09-06T12:00:00Z",
      "indexer": { "cursor_block": "380123456", "cursor_at": "2026-09-06T11:58:30Z", "lag_blocks": 0 },
      "sweeper": { "state": "idle", "queued": 0 }
    }
  ]
}`,
      },
    },
    {
      slug: "health",
      title: "Health check",
      method: "GET",
      path: "/health",
      auth: "none",
      summary: "Readiness. Requires a database round trip.",
      response: { description: "text/plain." },
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
