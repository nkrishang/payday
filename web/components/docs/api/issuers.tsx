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
      summary:
        "Save the party you issue under: a name and a contact mailbox, unverified until the code is confirmed.",
      bodyFields: [
        {
          name: "name",
          type: "string",
          required: true,
          description:
            "1 to 255 bytes. Unique among your identities, ignoring case and surrounding space.",
        },
        {
          name: "contact_email",
          type: "string",
          required: true,
          description: "3 to 254 bytes, lowercased on write. Two identities may share one.",
        },
        {
          name: "details",
          type: "string",
          description: "Up to 4,000 bytes, shown verbatim on requests.",
        },
      ],
      response: { description: "The issuer." },
      answers: [
        {
          status: 409,
          code: "issuer_name_taken",
          when: "Another of your identities already uses this name.",
        },
      ],
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
      summary: "Your identities, with a cursor.",
      query: [
        { name: "limit", type: "integer", description: "1 to 100, default 20." },
        {
          name: "starting_after",
          type: "iss_ id",
          description: "The previous page's next_cursor.",
        },
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
      summary: "One identity, with its saved payout addresses in association order.",
      pathParams: [{ name: "id", type: "iss_ id", required: true, description: "The identity." }],
      response: { description: "The issuer." },
      answers: [
        {
          status: 404,
          code: "issuer_not_found",
          when: "Missing, malformed, or another account's.",
        },
      ],
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
      summary:
        "Change any subset of name, contact_email, and details. A rename alone never touches a proven mailbox; a different contact_email clears the verification.",
      pathParams: [{ name: "id", type: "iss_ id", required: true, description: "The identity." }],
      bodyFields: [
        { name: "name", type: "string", description: "1 to 255 bytes, still unique." },
        {
          name: "contact_email",
          type: "string",
          description: "A different mailbox is a different claim: it must be proven again.",
        },
        { name: "details", type: "string | null", description: "New details, or null to clear." },
      ],
      response: { description: "The updated issuer." },
      answers: [{ status: 409, code: "issuer_name_taken", when: "The new name is in use." }],
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
      summary:
        "Remove an identity and its payout-address associations. The saved wallets themselves remain.",
      pathParams: [{ name: "id", type: "iss_ id", required: true, description: "The identity." }],
      response: { description: "204 No Content." },
      answers: [
        {
          status: 409,
          code: "issuer_in_use",
          when: "Requests were issued under it. They keep pointing at it.",
        },
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
      summary:
        "Email a six-digit code to the identity's stored contact address. The request names no mailbox.",
      body: (
        <p>
          Payers are told to write to the contact address, so a request cannot carry one that has
          not been proven. One code per identity per minute.
        </p>
      ),
      pathParams: [{ name: "id", type: "iss_ id", required: true, description: "The identity." }],
      response: {
        fields: [
          { name: "contact_email", type: "string", description: "Where the code went." },
          {
            name: "resend_available_at",
            type: "timestamp",
            description: "When another may be requested.",
          },
        ],
      },
      answers: [
        { status: 202, when: "Sent." },
        { status: 429, code: "otp_resend_cooldown", when: "One per minute; see Retry-After." },
        { status: 409, code: "issuer_email_already_verified", when: "Already proven." },
        {
          status: 503,
          code: "verification_unavailable",
          when: "The environment has no email verification configured.",
        },
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
      summary: "Exchange the code. The identity is usable on a deposit request once this succeeds.",
      pathParams: [{ name: "id", type: "iss_ id", required: true, description: "The identity." }],
      bodyFields: [
        {
          name: "otp",
          type: "string",
          required: true,
          description: "The six digits from the email.",
        },
      ],
      response: {
        description: (
          <>
            The issuer with <code>email_verified</code> true.
          </>
        ),
      },
      answers: [
        {
          status: 401,
          code: "otp_invalid",
          when: "Wrong, spent, expired, or confirmed after the address moved.",
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
      summary:
        "Replace the whole set of saved wallets this identity may settle to. The first is the default when a request names issuer_id without a payout_address.",
      pathParams: [{ name: "id", type: "iss_ id", required: true, description: "The identity." }],
      bodyFields: [
        {
          name: "payout_address_ids",
          type: "pa_ id[]",
          required: true,
          description: "At most 25, in the order they should be offered. Empty clears the set.",
        },
      ],
      response: { description: "The issuer with its payout_addresses." },
      answers: [
        { status: 404, code: "payout_address_not_found", when: "An id that is not yours." },
      ],
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
      summary:
        "Save a wallet the account may settle to. Stored EIP-55 checksummed and once per account: saving one you already hold returns the row you have.",
      bodyFields: [
        {
          name: "address",
          type: "string",
          required: true,
          description: "A nonzero EVM address, any case.",
        },
        {
          name: "label",
          type: "string",
          description:
            "At most 20 characters, starting with a letter or digit, using letters, digits, spaces, and . _ ' & ( ) -",
        },
      ],
      response: { description: "The payout address." },
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
      summary: "Every saved wallet on the account.",
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
      summary:
        "Remove a saved wallet and every association to it. Requests already issued to it are unaffected.",
      pathParams: [
        { name: "id", type: "pa_ id", required: true, description: "The payout address." },
      ],
      response: { description: "204 No Content." },
      answers: [
        {
          status: 404,
          code: "payout_address_not_found",
          when: "Missing, malformed, or another account's.",
        },
      ],
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
