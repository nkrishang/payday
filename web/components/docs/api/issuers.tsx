import type { EndpointGroup } from "./types";

const ISSUER = `{
  "id": "iss_0198f80c-2222-7dc1-a369-90556a64f700",
  "name": "Acme LLC",
  "contact_email": "billing@acme.example",
  "details": null,
  "email_verified": false,
  "email_verified_at": null,
  "payout_addresses": [],
  "created_at": "2026-09-06T12:00:00Z",
  "updated_at": "2026-09-06T12:00:00Z"
}`;

const VERIFIED = ISSUER.replace('"email_verified": false', '"email_verified": true').replace(
  '"email_verified_at": null',
  '"email_verified_at": "2026-09-06T12:05:00Z"',
);

const PAYOUT = `{
  "id": "pa_0198f80c-4444-7dc1-a369-90556a64f700",
  "address": "0x1111111111111111111111111111111111111111",
  "label": "Treasury",
  "created_at": "2026-09-06T12:00:00Z"
}`;

export const ISSUERS: EndpointGroup = {
  slug: "issuers",
  title: "Issuers",
  overview: { title: "Issuer identities" },
  endpoints: [
    {
      slug: "create",
      title: "Create an issuer",
      method: "POST",
      path: "/v1/issuers",
      auth: "key",
      summary: "Creates an issuer identity. Unverified until the contact mailbox is confirmed.",
      bodyFields: [
        {
          name: "name",
          type: "string",
          required: true,
          description: "1–255 bytes. Unique per account, case- and whitespace-insensitive.",
        },
        {
          name: "contact_email",
          type: "string",
          required: true,
          description: "3–254 bytes. Lowercased. Not unique.",
        },
        { name: "details", type: "string", description: "≤4,000 bytes." },
      ],
      response: { description: "Issuer." },
      answers: [{ status: 409, code: "issuer_name_taken", when: "" }],
      examples: {
        curl: `curl -fsS "$API/v1/issuers" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{ "name": "Acme LLC", "contact_email": "billing@acme.example" }'`,
        ts: `const acme = await payday.issuers.create({ name: "Acme LLC", contact_email: "billing@acme.example" });`,
        response: ISSUER,
        responseTitle: "201 Created",
      },
    },
    {
      slug: "list",
      title: "List issuers",
      method: "GET",
      path: "/v1/issuers",
      auth: "key",
      summary: "Lists issuer identities.",
      query: [
        { name: "limit", type: "integer", description: "1–100. Default 20." },
        { name: "starting_after", type: "iss_ id", description: "Cursor from next_cursor." },
      ],
      response: { description: "{ issuers: Issuer[], next_cursor: iss_ id | null }." },
      examples: {
        curl: `curl -fsS "$API/v1/issuers" -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const { issuers } = await payday.issuers.list();`,
        response: `{
  "issuers": [ ${VERIFIED.replace(/\n/g, "\n    ")} ],
  "next_cursor": null
}`,
      },
    },
    {
      slug: "get",
      title: "Get an issuer",
      method: "GET",
      path: "/v1/issuers/{id}",
      auth: "key",
      summary: "Retrieves an issuer identity.",
      pathParams: [{ name: "id", type: "iss_ id", required: true, description: "" }],
      response: { description: "Issuer. payout_addresses in association order." },
      answers: [{ status: 404, code: "issuer_not_found", when: "" }],
      examples: {
        curl: `curl -fsS "$API/v1/issuers/iss_0198f80c-2222-7dc1-a369-90556a64f700" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const acme = await payday.issuers.get(id);`,
        response: VERIFIED,
      },
    },
    {
      slug: "update",
      title: "Update an issuer",
      method: "PATCH",
      path: "/v1/issuers/{id}",
      auth: "key",
      summary: "Updates an issuer identity. Partial.",
      body: (
        <p>
          Omitted fields are unchanged. <code>details: null</code> clears. A changed{" "}
          <code>contact_email</code> resets <code>email_verified</code>. Issued requests are
          unaffected.
        </p>
      ),
      pathParams: [{ name: "id", type: "iss_ id", required: true, description: "" }],
      bodyFields: [
        { name: "name", type: "string", description: "1–255 bytes. Unique." },
        { name: "contact_email", type: "string", description: "Resets verification." },
        { name: "details", type: "string | null", description: "" },
      ],
      response: { description: "Issuer." },
      answers: [{ status: 409, code: "issuer_name_taken", when: "" }],
      examples: {
        curl: `curl -fsS -X PATCH "$API/v1/issuers/iss_0198f80c-…" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{ "name": "Acme Holdings LLC" }'`,
        ts: `await payday.issuers.update(id, { name: "Acme Holdings LLC" });`,
        response: VERIFIED.replace('"name": "Acme LLC"', '"name": "Acme Holdings LLC"'),
      },
    },
    {
      slug: "delete",
      title: "Delete an issuer",
      method: "DELETE",
      path: "/v1/issuers/{id}",
      auth: "key",
      summary: "Deletes an issuer identity and its payout-address associations.",
      pathParams: [{ name: "id", type: "iss_ id", required: true, description: "" }],
      response: { description: "204 No Content." },
      answers: [
        { status: 409, code: "issuer_in_use", when: "Requests were issued under the identity." },
      ],
      examples: {
        curl: `curl -fsS -X DELETE "$API/v1/issuers/iss_0198f80c-…" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `await payday.issuers.remove(id);`,
        response: `HTTP/1.1 204 No Content`,
        responseLang: "http",
      },
    },
    {
      slug: "verify-email-start",
      title: "Send the verification code",
      method: "POST",
      path: "/v1/issuers/{id}/verify/email/start",
      auth: "key",
      summary: "Sends a one-time code to the stored contact_email.",
      body: <p>Six digits; five-minute validity. One code per identity per minute.</p>,
      pathParams: [{ name: "id", type: "iss_ id", required: true, description: "" }],
      response: {
        fields: [
          { name: "contact_email", type: "string", description: "" },
          { name: "resend_available_at", type: "timestamp", description: "" },
        ],
      },
      answers: [
        { status: 202, when: "" },
        { status: 429, code: "otp_resend_cooldown", when: "Retry-After set." },
        { status: 409, code: "issuer_email_already_verified", when: "" },
        { status: 503, code: "verification_unavailable", when: "No identity provider configured." },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/issuers/iss_0198f80c-…/verify/email/start" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const { contact_email, resend_available_at } = await payday.issuers.startEmailVerification(id);`,
        response: `{
  "contact_email": "billing@acme.example",
  "resend_available_at": "2026-09-06T12:01:00Z"
}`,
        responseTitle: "202 Accepted",
      },
    },
    {
      slug: "verify-email-confirm",
      title: "Confirm the verification code",
      method: "POST",
      path: "/v1/issuers/{id}/verify/email/confirm",
      auth: "key",
      summary: "Confirms the code. Sets email_verified.",
      pathParams: [{ name: "id", type: "iss_ id", required: true, description: "" }],
      bodyFields: [{ name: "otp", type: "string", required: true, description: "Six digits." }],
      response: { description: "Issuer." },
      answers: [
        {
          status: 401,
          code: "otp_invalid",
          when: "Wrong, spent, expired, or issued for a previous contact_email.",
        },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/issuers/iss_0198f80c-…/verify/email/confirm" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{ "otp": "123456" }'`,
        ts: `const verified = await payday.issuers.confirmEmailVerification(id, "123456");`,
        response: VERIFIED,
      },
    },
    {
      slug: "set-payout-addresses",
      title: "Set payout addresses",
      method: "PUT",
      path: "/v1/issuers/{id}/payout-addresses",
      auth: "key",
      summary: "Replaces the identity's payout-address set.",
      pathParams: [{ name: "id", type: "iss_ id", required: true, description: "" }],
      bodyFields: [
        {
          name: "payout_address_ids",
          type: "pa_ id[]",
          required: true,
          description:
            "≤25. Ordered; the first is the default for issuer_id-only creates. Empty clears.",
        },
      ],
      response: { description: "Issuer." },
      answers: [{ status: 404, code: "payout_address_not_found", when: "" }],
      examples: {
        curl: `curl -fsS -X PUT "$API/v1/issuers/iss_0198f80c-…/payout-addresses" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{ "payout_address_ids": ["pa_0198f80c-4444-7dc1-a369-90556a64f700"] }'`,
        ts: `await payday.issuers.setPayoutAddresses(id, [treasury.id]);`,
        response: VERIFIED.replace(
          '"payout_addresses": []',
          `"payout_addresses": [ ${PAYOUT.replace(/\n/g, "\n    ")} ]`,
        ),
      },
    },
  ],
};

export const PAYOUT_ADDRESSES: EndpointGroup = {
  slug: "payout-addresses",
  title: "Payout addresses",
  endpoints: [
    {
      slug: "create",
      title: "Save a payout address",
      method: "POST",
      path: "/v1/payout-addresses",
      auth: "key",
      summary: "Saves a payout address. Idempotent per address: an existing row is returned.",
      bodyFields: [
        {
          name: "address",
          type: "string",
          required: true,
          description: "Nonzero EVM address. Stored EIP-55.",
        },
        {
          name: "label",
          type: "string",
          description:
            "≤20 characters. Leading letter or digit; letters, digits, spaces, . _ ' & ( ) -",
        },
      ],
      response: { description: "PayoutAddress." },
      examples: {
        curl: `curl -fsS "$API/v1/payout-addresses" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{ "address": "0x1111111111111111111111111111111111111111", "label": "Treasury" }'`,
        ts: `const treasury = await payday.payoutAddresses.create({ address: "0x1111…", label: "Treasury" });`,
        response: PAYOUT,
        responseTitle: "201 Created",
      },
    },
    {
      slug: "list",
      title: "List payout addresses",
      method: "GET",
      path: "/v1/payout-addresses",
      auth: "key",
      summary: "Lists payout addresses.",
      response: { description: "{ payout_addresses: PayoutAddress[] }." },
      examples: {
        curl: `curl -fsS "$API/v1/payout-addresses" -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const { payout_addresses } = await payday.payoutAddresses.list();`,
        response: `{
  "payout_addresses": [ ${PAYOUT.replace(/\n/g, "\n    ")} ]
}`,
      },
    },
    {
      slug: "delete",
      title: "Delete a payout address",
      method: "DELETE",
      path: "/v1/payout-addresses/{id}",
      auth: "key",
      summary: "Deletes a payout address and its associations. Issued requests are unaffected.",
      pathParams: [{ name: "id", type: "pa_ id", required: true, description: "" }],
      response: { description: "204 No Content." },
      answers: [{ status: 404, code: "payout_address_not_found", when: "" }],
      examples: {
        curl: `curl -fsS -X DELETE "$API/v1/payout-addresses/pa_0198f80c-…" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `await payday.payoutAddresses.remove(id);`,
        response: `HTTP/1.1 204 No Content`,
        responseLang: "http",
      },
    },
  ],
};
