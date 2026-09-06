import type { EndpointGroup, FieldDoc } from "./types";

/** The `DepositRequest` object. */
export const DEPOSIT_REQUEST_FIELDS: FieldDoc[] = [
  { name: "id", type: "dr_ id", description: "Deposit request id." },
  { name: "deposit_url", type: "string", description: "Hosted checkout URL." },
  {
    name: "status",
    type: "enum",
    description: (
      <>
        <code>awaiting_deposit</code> · <code>partially_deposited</code> · <code>deposited</code> ·{" "}
        <code>settled</code> · <code>expired</code> · <code>returned</code> ·{" "}
        <code>needs_attention</code>
      </>
    ),
  },
  { name: "chain", type: "object", description: "{ id, name }." },
  {
    name: "token",
    type: "object",
    description: "{ symbol, address, decimals }. The exact USDC contract.",
  },
  { name: "currency", type: "string", description: "USDC." },
  {
    name: "address",
    type: "string | null",
    description: "One-time deposit address. Null until the payer's wallet attestation is accepted.",
  },
  {
    name: "address_explorer_url",
    type: "string | null",
    description: "When an explorer is configured.",
  },
  { name: "payout_address", type: "string", description: "Receives exactly amount at settlement." },
  {
    name: "payer_wallet",
    type: "string | null",
    description: "Attested payer wallet. Null until bound.",
  },
  {
    name: "recovery_address",
    type: "string | null",
    description: "Equals payer_wallet. Destination of all returns.",
  },
  { name: "wallet_bound_at", type: "timestamp | null", description: "" },
  {
    name: "amount, received, remaining",
    type: "decimal string",
    description: (
      <>
        Six-decimal USDC. Each has an integer <code>*_base_units</code> counterpart.
      </>
    ),
  },
  {
    name: "fee_amount, net_amount",
    type: "decimal string",
    description: "Fees are zero. net_amount = amount.",
  },
  {
    name: "issuer, payer",
    type: "Party",
    description: "{ name, email?, details? }. Snapshot at issuance.",
  },
  { name: "heading, reference, notes", type: "string | null", description: "As submitted." },
  { name: "metadata", type: "object", description: "As submitted. Merchant-only." },
  { name: "customer_id, issuer_id", type: "id | null", description: "Linked records." },
  {
    name: "payer_policy",
    type: "object",
    description: "Full policy, including the assertion. Merchant-only.",
  },
  {
    name: "attachment",
    type: "object | null",
    description: "{ id, filename, mime_type, byte_length, sha256 }.",
  },
  {
    name: "client_secret, client_secret_expires_at",
    type: "string",
    description: "merchant_session only. Present on the issuing 201; never again.",
  },
  {
    name: "verification_completed_at",
    type: "timestamp | null",
    description: "Payer policy satisfied.",
  },
  {
    name: "likely_unsolicited_at",
    type: "timestamp | null",
    description: "First finalized credit from a wallet other than payer_wallet.",
  },
  { name: "created_at, updated_at, expires_at", type: "timestamp", description: "RFC 3339 UTC." },
  {
    name: "deposited_at, settled_at, expired_at",
    type: "timestamp | null",
    description: "Lifecycle milestones. deposited_at_block and settled_block carry block numbers.",
  },
  {
    name: "cancellation_requested_at",
    type: "timestamp | null",
    description: "Set by POST …/cancel.",
  },
  {
    name: "settlement_tx_hash",
    type: "string | null",
    description:
      "Transaction that executed the deposit contract. settlement_explorer_url when configured.",
  },
  {
    name: "attention",
    type: "object | null",
    description: "{ code, message, action } while status is needs_attention.",
  },
  {
    name: "transfers",
    type: "Transfer[]",
    description: "Finalized transfers to address. Same shape as GET …/transfers.",
  },
  {
    name: "as_of",
    type: "object | null",
    description: "{ block, at }. Commit point of this projection.",
  },
  {
    name: "indexer_freshness",
    type: "object",
    description: "{ last_indexed_block, last_finalized_block, cursor_updated_at }.",
  },
  {
    name: "self_settlement",
    type: "object | null",
    description: "{ factory, salt }. Sufficient to execute the contract. Null until bound.",
  },
  {
    name: "attribution",
    type: "object",
    description: "{ version, hash }. Commitment to the canonical issued document.",
  },
];

const OBJECT = `{
  "id": "dr_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "deposit_url": "https://payday.sh/pay/dr_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "status": "awaiting_deposit",
  "chain": { "id": "143", "name": "Monad" },
  "token": { "symbol": "USDC", "address": "0x754704Bc059F8C67012fEd69BC8A327a5aafb603", "decimals": 6 },
  "currency": "USDC",
  "address": null,
  "address_explorer_url": null,
  "payout_address": "0x1111111111111111111111111111111111111111",
  "payer_wallet": null,
  "recovery_address": null,
  "wallet_bound_at": null,
  "amount": "10.500000",
  "amount_base_units": "10500000",
  "received": "0.000000",
  "received_base_units": "0",
  "remaining": "10.500000",
  "remaining_base_units": "10500000",
  "fee_amount": "0.000000",
  "fee_amount_base_units": "0",
  "net_amount": "10.500000",
  "net_amount_base_units": "10500000",
  "issuer": { "name": "Acme LLC", "email": "billing@acme.example" },
  "payer": { "name": "Customer Inc", "email": "ap@customer.example" },
  "heading": "March retainer",
  "reference": "INV-1042",
  "notes": "Net 30. Thank you.",
  "metadata": { "po": "PO-77" },
  "customer_id": "cus_0198f80c-1111-7dc1-a369-90556a64f700",
  "issuer_id": "iss_0198f80c-2222-7dc1-a369-90556a64f700",
  "payer_policy": { "mode": "verified_email", "expected_email": "ap@customer.example" },
  "attachment": null,
  "verification_completed_at": null,
  "likely_unsolicited_at": null,
  "created_at": "2026-09-06T12:00:00Z",
  "updated_at": "2026-09-06T12:00:00Z",
  "expires_at": "2026-09-06T13:00:00Z",
  "deposited_at": null,
  "deposited_at_block": null,
  "settled_at": null,
  "settled_block": null,
  "expired_at": null,
  "cancellation_requested_at": null,
  "settlement_tx_hash": null,
  "settlement_explorer_url": null,
  "attention": null,
  "transfers": [],
  "as_of": { "block": "98765000", "at": "2026-09-06T11:59:58Z" },
  "indexer_freshness": {
    "last_indexed_block": "98765000",
    "last_finalized_block": "98765000",
    "cursor_updated_at": "2026-09-06T11:59:59Z"
  },
  "self_settlement": null,
  "attribution": { "version": 2, "hash": "0x…" }
}`;

export const DEPOSIT_REQUEST_EXAMPLE = OBJECT;

const SETTLED = OBJECT.replace('"status": "awaiting_deposit"', '"status": "settled"')
  .replace('"address": null', '"address": "0x2222222222222222222222222222222222222222"')
  .replace('"payer_wallet": null', '"payer_wallet": "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed"')
  .replace(
    '"recovery_address": null',
    '"recovery_address": "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed"',
  )
  .replace('"wallet_bound_at": null', '"wallet_bound_at": "2026-09-06T12:05:40Z"')
  .replace('"received": "0.000000"', '"received": "10.500000"')
  .replace('"received_base_units": "0"', '"received_base_units": "10500000"')
  .replace('"remaining": "10.500000"', '"remaining": "0.000000"')
  .replace('"remaining_base_units": "10500000"', '"remaining_base_units": "0"')
  .replace('"settlement_tx_hash": null', '"settlement_tx_hash": "0x…"')
  .replace('"settled_at": null', '"settled_at": "2026-09-06T12:20:00Z"');

const CREATE_BODY: FieldDoc[] = [
  {
    name: "amount",
    type: "string",
    required: true,
    description: "Positive USDC decimal; at most six fractional digits. Settled exactly.",
  },
  {
    name: "payer_policy",
    type: "object",
    required: true,
    description: (
      <>
        <code>{`{"mode":"permissionless"}`}</code> ·{" "}
        <code>{`{"mode":"verified_email","expected_email"}`}</code> ·{" "}
        <code>{`{"mode":"merchant_session","payer_reference"}`}</code>. <code>expected_email</code>{" "}
        is trimmed and lowercased. <code>payer_reference</code>: 1–128 printable bytes, no
        whitespace, case preserved. Assertions are rejected on other modes.
      </>
    ),
  },
  {
    name: "payout_address",
    type: "string",
    description: (
      <>
        Nonzero EVM address. Required unless <code>issuer_id</code> names an identity with a saved
        payout address, whose first address is used.
      </>
    ),
  },
  {
    name: "issuer",
    type: "Party",
    description: (
      <>
        <code>name</code> 1–255 bytes; <code>email</code> 3–254 bytes; <code>details</code> ≤4,000
        bytes. Required unless <code>issuer_id</code> is given. Inline wins.
      </>
    ),
  },
  {
    name: "payer",
    type: "Party",
    description: (
      <>
        Same shape. Required unless <code>customer_id</code> is given. Inline wins.{" "}
        <code>payer.email</code> receives the issued request by email, except under{" "}
        <code>merchant_session</code>.
      </>
    ),
  },
  {
    name: "issuer_id",
    type: "iss_ id",
    description:
      "Stored immutably on the request. Supplies issuer and payout_address when omitted.",
  },
  {
    name: "customer_id",
    type: "cus_ id",
    description:
      "Stored on the request. Supplies payer when omitted; the request keeps its own snapshot.",
  },
  {
    name: "heading",
    type: "string",
    description: "≤200 bytes. Visible to the payer before verification.",
  },
  {
    name: "reference",
    type: "string",
    description: "≤128 characters. Exact-match filter on list.",
  },
  {
    name: "notes",
    type: "string",
    description: "≤4,000 bytes. Visible to the payer after verification.",
  },
  {
    name: "attachment_id",
    type: "att_ id",
    description: "A finalized attachment. One per request.",
  },
  {
    name: "expires_in",
    type: "integer",
    description: "Seconds. Exclusive with expires_at. Default 86,400; range 600 to 31,622,400.",
  },
  { name: "expires_at", type: "timestamp", description: "RFC 3339. Same range." },
  {
    name: "metadata",
    type: "object",
    description:
      "≤16 keys; ≤512 encoded bytes per value. Returned on reads and webhooks. Merchant-only.",
  },
  {
    name: "chain_id, token_address",
    type: "string",
    description: "Deployment overrides. Must equal the environment's values when present.",
  },
];

export const DEPOSIT_REQUESTS: EndpointGroup = {
  slug: "deposit-requests",
  title: "Deposit requests",
  overview: { title: "The deposit request object" },
  endpoints: [
    {
      slug: "create",
      title: "Create a deposit request",
      method: "POST",
      path: "/v1/deposit-requests",
      auth: "key",
      summary: "Issues a deposit request.",
      body: (
        <>
          <p>
            Minimal body: <code>amount</code>, <code>issuer_id</code>, <code>customer_id</code>,{" "}
            <code>payer_policy</code>. Unknown fields are rejected. Text fields reject control
            characters. Every immutable field participates in idempotency, including the
            attachment&apos;s hash.
          </p>
          <p>
            <code>address</code>, <code>payer_wallet</code>, <code>recovery_address</code>,{" "}
            <code>wallet_bound_at</code>, and <code>self_settlement</code> are null until the
            payer&apos;s wallet attestation is accepted, signalled by{" "}
            <code>deposit_request.ready</code>. Recovery is not a request field;{" "}
            <code>recovery_address</code> is always the attested wallet.
          </p>
        </>
      ),
      headers: [
        { name: "Idempotency-Key", type: "string", required: true, description: "1–255 bytes." },
      ],
      bodyFields: CREATE_BODY,
      response: {
        description: (
          <>
            <code>DepositRequest</code>. Under <code>merchant_session</code>, also{" "}
            <code>client_secret</code> and <code>client_secret_expires_at</code>, once.
          </>
        ),
      },
      answers: [
        { status: 201, when: "Issued." },
        { status: 200, when: "Idempotent replay. Idempotency-Replayed: true." },
        {
          status: 400,
          code: "invalid_request",
          when: "Shape, unknown field, type, or control character. Message names the field.",
        },
        { status: 400, code: "invalid_amount", when: "Syntax, precision, or sign." },
        { status: 400, code: "missing_idempotency_key", when: "Header absent." },
        {
          status: 404,
          code: "customer_not_found / issuer_not_found",
          when: "Not owned by the account.",
        },
        {
          status: 409,
          code: "idempotency_conflict",
          when: "Key reused with a different document.",
        },
        {
          status: 409,
          code: "attachment_not_ready",
          when: "Attachment not finalized, rejected, or expired unused.",
        },
        {
          status: 409,
          code: "attachment_already_attached",
          when: "Attachment belongs to another request.",
        },
        {
          status: 409,
          code: "account_contact_required",
          when: "Account has no verified email. Re-authenticate in the dashboard.",
        },
        {
          status: 422,
          code: "unsupported_chain / unsupported_token",
          when: "Override not served by the environment.",
        },
      ],
      examples: {
        curl: `curl -fsS "$API/v1/deposit-requests" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  -H "Content-Type: application/json" \\
  -H "Idempotency-Key: INV-1042" \\
  -d '{
    "amount": "10.50",
    "payout_address": "0x1111111111111111111111111111111111111111",
    "issuer": { "name": "Acme LLC", "email": "billing@acme.example" },
    "payer": { "name": "Customer Inc", "email": "ap@customer.example" },
    "heading": "March retainer",
    "reference": "INV-1042",
    "notes": "Net 30. Thank you.",
    "payer_policy": { "mode": "verified_email", "expected_email": "ap@customer.example" },
    "customer_id": "cus_0198f80c-1111-7dc1-a369-90556a64f700",
    "issuer_id": "iss_0198f80c-2222-7dc1-a369-90556a64f700",
    "expires_in": 3600,
    "metadata": { "po": "PO-77" }
  }'`,
        ts: `const request = await payday.depositRequests.create(
  {
    amount: "10.50",
    payout_address: "0x1111111111111111111111111111111111111111",
    issuer: { name: "Acme LLC", email: "billing@acme.example" },
    payer: { name: "Customer Inc", email: "ap@customer.example" },
    heading: "March retainer",
    reference: "INV-1042",
    notes: "Net 30. Thank you.",
    payer_policy: { mode: "verified_email", expected_email: "ap@customer.example" },
    customer_id: "cus_0198f80c-1111-7dc1-a369-90556a64f700",
    issuer_id: "iss_0198f80c-2222-7dc1-a369-90556a64f700",
    expires_in: 3600,
    metadata: { po: "PO-77" },
  },
  "INV-1042", // Idempotency-Key
);`,
        response: OBJECT,
        responseTitle: "201 Created",
      },
    },
    {
      slug: "list",
      title: "List deposit requests",
      method: "GET",
      path: "/v1/deposit-requests",
      auth: "key",
      summary: "Lists deposit requests, newest first, as summaries.",
      query: [
        { name: "status", type: "enum", description: "One public status." },
        { name: "reference", type: "string", description: "Exact match." },
        { name: "customer_id", type: "cus_ id", description: "" },
        { name: "issuer_id", type: "iss_ id", description: "" },
        {
          name: "verification",
          type: "enum",
          description: (
            <>
              <code>not_required</code> · <code>pending</code> · <code>verified</code> ·{" "}
              <code>likely_unsolicited</code>. Independent of <code>status</code>.
            </>
          ),
        },
        { name: "limit", type: "integer", description: "1–100. Default 20." },
        { name: "starting_after", type: "dr_ id", description: "Cursor from next_cursor." },
      ],
      response: {
        description: (
          <>
            <code>{`{ deposit_requests: DepositRequestSummary[], next_cursor: dr_ id | null }`}</code>
            . Summary fields: <code>id</code>, <code>deposit_url</code>, <code>heading</code>,{" "}
            <code>payer_name</code>, <code>reference</code>, <code>metadata</code>,{" "}
            <code>payer_policy_mode</code>, <code>customer_id</code>, <code>issuer_id</code>,{" "}
            <code>has_attachment</code>, <code>verification_completed_at</code>,{" "}
            <code>likely_unsolicited_at</code>, <code>status</code>, <code>amount</code>,{" "}
            <code>received</code>, <code>cancellation_requested_at</code>, <code>created_at</code>,{" "}
            <code>updated_at</code>, <code>expires_at</code>.
          </>
        ),
      },
      examples: {
        curl: `curl -fsS "$API/v1/deposit-requests?status=partially_deposited&limit=20" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const page = await payday.depositRequests.list({ status: "partially_deposited", limit: 20 });`,
        response: `{
  "deposit_requests": [
    {
      "id": "dr_0198f80c-8d2f-7dc1-a369-90556a64f700",
      "deposit_url": "https://payday.sh/pay/dr_0198f80c-8d2f-7dc1-a369-90556a64f700",
      "heading": "March retainer",
      "payer_name": "Customer Inc",
      "reference": "INV-1042",
      "metadata": { "po": "PO-77" },
      "payer_policy_mode": "verified_email",
      "customer_id": "cus_0198f80c-1111-7dc1-a369-90556a64f700",
      "issuer_id": "iss_0198f80c-2222-7dc1-a369-90556a64f700",
      "has_attachment": false,
      "verification_completed_at": null,
      "likely_unsolicited_at": null,
      "status": "partially_deposited",
      "amount": "10.500000",
      "received": "4.000000",
      "cancellation_requested_at": null,
      "created_at": "2026-09-06T12:00:00Z",
      "updated_at": "2026-09-06T12:10:00Z",
      "expires_at": "2026-09-06T13:00:00Z"
    }
  ],
  "next_cursor": null
}`,
      },
    },
    {
      slug: "get",
      title: "Get a deposit request",
      method: "GET",
      path: "/v1/deposit-requests/{id}",
      auth: "key",
      summary: "Retrieves a deposit request by id or by deposit address.",
      pathParams: [
        {
          name: "id",
          type: "dr_ id | address",
          required: true,
          description: "Complete canonical id, or the one-time address.",
        },
      ],
      query: [
        {
          name: "wait_for",
          type: '"change"',
          description: (
            <>
              Long-poll. Returns when <code>updated_at</code> changes or <code>timeout</code>{" "}
              elapses.
            </>
          ),
        },
        {
          name: "timeout",
          type: "integer",
          description: "1–30 seconds. Default 30. Requires wait_for.",
        },
      ],
      response: { description: "DepositRequest." },
      answers: [
        { status: 200, when: "" },
        { status: 400, code: "invalid_request", when: "Partial id." },
        {
          status: 404,
          code: "deposit_request_not_found",
          when: "Unknown, or not owned by the account.",
        },
      ],
      examples: {
        curl: `curl -fsS "$API/v1/deposit-requests/dr_0198f80c-8d2f-7dc1-a369-90556a64f700?wait_for=change&timeout=30" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const request = await payday.depositRequests.get(id);
const latest = await payday.depositRequests.get(id, { waitForChange: true, timeout: 30 });`,
        response: SETTLED,
        responseTitle: "200 OK",
      },
    },
    {
      slug: "cancel",
      title: "Cancel a deposit request",
      method: "POST",
      path: "/v1/deposit-requests/{id}/cancel",
      auth: "key",
      summary: "Records a cancellation request.",
      body: (
        <p>
          Presentation only. The address remains live and its terms unchanged; later transfers are
          detected and routed under those terms. Idempotent.
        </p>
      ),
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      response: {
        description: (
          <>
            <code>DepositRequest</code> with <code>cancellation_requested_at</code> set.
          </>
        ),
      },
      examples: {
        curl: `curl -fsS -X POST "$API/v1/deposit-requests/dr_0198f80c-…/cancel" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const cancelled = await payday.depositRequests.cancel(id);`,
        response: OBJECT.replace(
          '"cancellation_requested_at": null',
          '"cancellation_requested_at": "2026-09-06T12:30:00Z"',
        ),
        responseTitle: "200 OK",
      },
    },
    {
      slug: "transfers",
      title: "List transfers",
      method: "GET",
      path: "/v1/deposit-requests/{id}/transfers",
      auth: "key",
      summary: "Lists finalized transfers to the deposit address.",
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      response: {
        description: <code>{`{ transfers: Transfer[] }`}</code>,
        fields: [
          { name: "timestamp", type: "timestamp", description: "Block time." },
          { name: "amount, amount_base_units", type: "string", description: "" },
          { name: "sender", type: "string", description: "" },
          {
            name: "transaction_hash",
            type: "string",
            description: "explorer_url when configured.",
          },
          { name: "block", type: "string", description: "" },
          {
            name: "disposition",
            type: "enum",
            description: (
              <>
                <code>credited</code> · <code>late</code> · <code>zero</code>
              </>
            ),
          },
          { name: "collected", type: "boolean", description: "Moved on by settlement or return." },
        ],
      },
      examples: {
        curl: `curl -fsS "$API/v1/deposit-requests/dr_0198f80c-…/transfers" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const { transfers } = await payday.depositRequests.transfers(id);`,
        response: `{
  "transfers": [
    {
      "timestamp": "2026-09-06T12:14:07Z",
      "amount": "10.500000",
      "amount_base_units": "10500000",
      "sender": "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
      "transaction_hash": "0x…",
      "explorer_url": "https://monadvision.com/tx/0x…",
      "block": "98765432",
      "disposition": "credited",
      "collected": true
    }
  ]
}`,
      },
    },
    {
      slug: "verification",
      title: "Get verification",
      method: "GET",
      path: "/v1/deposit-requests/{id}/verification",
      auth: "key",
      summary: "Retrieves verification facts and attempts.",
      body: (
        <p>
          Fact values: <code>not_required</code>, <code>pending</code>, <code>approved</code>;{" "}
          <code>wallet</code> is never <code>not_required</code>. Attempt kinds: <code>email</code>,{" "}
          <code>wallet</code>, <code>merchant_session</code>; statuses <code>pending</code>,{" "}
          <code>approved</code>, <code>abandoned</code>. Sessions, client secrets, and codes are
          never returned.
        </p>
      ),
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      response: {
        fields: [
          { name: "payer_policy_mode", type: "enum", description: "" },
          { name: "verification_completed_at", type: "timestamp | null", description: "" },
          { name: "likely_unsolicited_at", type: "timestamp | null", description: "" },
          {
            name: "facts",
            type: "object",
            description: "{ email, merchant_session, wallet, complete }.",
          },
          {
            name: "attempts",
            type: "object[]",
            description: "{ id, kind, status, verified_at, created_at }.",
          },
        ],
      },
      examples: {
        curl: `curl -fsS "$API/v1/deposit-requests/dr_0198f80c-…/verification" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const detail = await payday.depositRequests.verification(id);`,
        response: `{
  "payer_policy_mode": "verified_email",
  "verification_completed_at": "2026-09-06T12:05:00Z",
  "likely_unsolicited_at": null,
  "facts": { "email": "approved", "merchant_session": "not_required", "wallet": "approved", "complete": true },
  "attempts": [
    { "id": "va_0198f80c-…", "kind": "email", "status": "approved", "verified_at": "2026-09-06T12:05:00Z", "created_at": "2026-09-06T12:04:10Z" },
    { "id": "va_0198f80c-…", "kind": "wallet", "status": "approved", "verified_at": "2026-09-06T12:05:40Z", "created_at": "2026-09-06T12:05:30Z" }
  ]
}`,
      },
    },
    {
      slug: "client-secret",
      title: "Create a client secret",
      method: "POST",
      path: "/v1/deposit-requests/{id}/client-secret",
      auth: "key",
      summary: "Mints a single-use client secret for a merchant_session request.",
      body: (
        <p>
          Valid fifteen minutes; spent on first exchange. Earlier unspent secrets remain valid.
          Transport: URL fragment, <code>deposit_url#cs=&lt;secret&gt;</code>. Response is{" "}
          <code>Cache-Control: no-store</code>.
        </p>
      ),
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      response: {
        fields: [
          { name: "client_secret", type: "string", description: "Returned once. Stored hashed." },
          { name: "expires_at", type: "timestamp", description: "" },
        ],
      },
      answers: [
        { status: 201, when: "" },
        { status: 409, code: "verification_not_required", when: "Mode is permissionless." },
        {
          status: 409,
          code: "verification_method_not_applicable",
          when: "Mode is verified_email.",
        },
        {
          status: 410,
          code: "deposit_request_not_payable",
          when: "Closed without verification. A settled, verified request still mints.",
        },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/deposit-requests/dr_0198f80c-…/client-secret" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `import { checkoutUrl } from "@payday/sdk";

const { client_secret } = await payday.depositRequests.createClientSecret(id);
res.redirect(303, checkoutUrl(request, client_secret));`,
        response: `{
  "client_secret": "cs_…",
  "expires_at": "2026-09-06T12:15:00Z"
}`,
        responseTitle: "201 Created",
      },
    },
    {
      slug: "preview-session",
      title: "Create a preview session",
      method: "POST",
      path: "/v1/deposit-requests/{id}/preview-session",
      auth: "key",
      summary:
        "Mints a payer session that opens the request's payer view unlocked, for the issuing account.",
      body: (
        <p>
          Not verification: records no attempt and never sets <code>verification_completed_at</code>
          . Any policy mode. Transport: URL fragment, <code>deposit_url#ps=&lt;session&gt;</code>.
        </p>
      ),
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      response: {
        fields: [
          { name: "payer_session", type: "string", description: "" },
          { name: "expires_at", type: "timestamp", description: "" },
        ],
      },
      answers: [
        { status: 410, code: "deposit_request_not_payable", when: "Closed without verification." },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/deposit-requests/dr_0198f80c-…/preview-session" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `import { previewUrl } from "@payday/sdk";

const { payer_session } = await payday.depositRequests.previewSession(id);
window.open(previewUrl(request, payer_session));`,
        response: `{
  "payer_session": "…",
  "expires_at": "2026-09-07T12:00:00Z"
}`,
      },
    },
    {
      slug: "attachment",
      title: "Get the attachment",
      method: "GET",
      path: "/v1/deposit-requests/{id}/attachment",
      auth: "key",
      summary: "Retrieves the attachment descriptor with a signed download URL.",
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      response: {
        fields: [
          { name: "id", type: "att_ id", description: "" },
          {
            name: "filename, mime_type",
            type: "string",
            description: "mime_type is application/pdf.",
          },
          { name: "byte_length", type: "string", description: "Decimal." },
          {
            name: "sha256",
            type: "string",
            description: "0x hex. Committed into the deposit address.",
          },
          { name: "download_url", type: "string", description: "Signed. Valid 300 seconds." },
        ],
      },
      answers: [{ status: 404, code: "attachment_not_found", when: "No attachment." }],
      examples: {
        curl: `curl -fsS "$API/v1/deposit-requests/dr_0198f80c-…/attachment" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const pdf = await payday.depositRequests.attachment(id);`,
        response: `{
  "id": "att_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "filename": "INV-1042.pdf",
  "mime_type": "application/pdf",
  "byte_length": "48211",
  "sha256": "0x9f…",
  "download_url": "https://…"
}`,
      },
    },
    {
      slug: "request-pdf",
      title: "Render the request PDF",
      method: "GET",
      path: "/v1/deposit-requests/{id}/request.pdf",
      auth: "key",
      summary:
        "Renders the deposit request as PDF. Deterministic: identical input yields identical bytes.",
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      response: { description: "application/pdf." },
      examples: {
        curl: `curl -fsS "$API/v1/deposit-requests/dr_0198f80c-…/request.pdf" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" -o request.pdf`,
        ts: `const blob = await payday.depositRequests.requestPdf(id);`,
        response: `HTTP/1.1 200 OK
Content-Type: application/pdf
Content-Length: 31288`,
        responseLang: "http",
      },
    },
    {
      slug: "proof",
      title: "Get the Proof of Payment",
      method: "GET",
      path: "/v1/deposit-requests/{id}/proof",
      auth: "key",
      summary: "Retrieves the Proof of Payment for a settled request.",
      body: (
        <p>
          Document version <code>payday.proof.v2</code>: canonical issuance snapshot, attribution
          hash, payer wallet attestation, salt, chain, factory, token, deposit and recovery
          addresses, credited transfers, settlement transaction hash, and a Payday-signed
          verification attestation. Verifiable offline; reference implementation{" "}
          <code>gateway_core::verify_proof</code>. Schema:{" "}
          <a href="/docs/proof-of-payment">Proof of Payment</a>.
        </p>
      ),
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      response: { description: "ProofOfPayment." },
      answers: [
        { status: 200, when: "" },
        { status: 409, code: "deposit_request_not_settled", when: "" },
        {
          status: 409,
          code: "deposit_sender_mismatch",
          when: "A credited transfer originated from a wallet other than payer_wallet. No proof is issued.",
        },
      ],
      examples: {
        curl: `curl -fsS "$API/v1/deposit-requests/dr_0198f80c-…/proof" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" > proof.json`,
        ts: `const proof = await payday.depositRequests.proof(id);`,
        response: `{
  "version": "payday.proof.v2",
  "payment_id": "dr_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "canonical_issuance_snapshot": { "schema": "payday.invoice", "canonicalization": "RFC8785", "…": "…" },
  "canonicalization": "RFC8785",
  "attribution_hash": "0x…",
  "payer_wallet": { "address": "0x5aAe…", "typed_data": { "…": "…" }, "digest": "0x…", "signature": "0x…", "method": "ecdsa" },
  "salt": "0x…",
  "chain_id": "143",
  "factory_address": "0x…",
  "payment_address": "0x2222222222222222222222222222222222222222",
  "token_address": "0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
  "recovery_address": "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
  "settlement_transaction_hash": "0x…",
  "transfers": [ { "transaction_hash": "0x…", "log_index": "12", "sender": "0x5aAe…", "recipient": "0x2222…", "amount_base_units": "10500000", "block_number": "98765432" } ],
  "verification": { "payload": { "…": "…" }, "signer": "0x…", "signature": "0x…" }
}`,
      },
    },
  ],
};
