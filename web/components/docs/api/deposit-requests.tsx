import type { EndpointGroup, FieldDoc } from "./types";

/** The `DepositRequest` object, as the create and read routes return it. */
export const DEPOSIT_REQUEST_FIELDS: FieldDoc[] = [
  { name: "id", type: "dr_ id", description: "The deposit request." },
  {
    name: "deposit_url",
    type: "string",
    description: "The hosted checkout. Give this to the payer.",
  },
  {
    name: "status",
    type: "string",
    description: (
      <>
        <code>awaiting_deposit</code>, <code>partially_deposited</code>, <code>deposited</code>,{" "}
        <code>settled</code>, <code>expired</code>, <code>returned</code>, or{" "}
        <code>needs_attention</code>.
      </>
    ),
  },
  {
    name: "chain",
    type: "object",
    description: "{ id, name } of the chain the request settles on.",
  },
  {
    name: "token",
    type: "object",
    description: "{ symbol, address, decimals } of the exact USDC contract.",
  },
  {
    name: "address",
    type: "string | null",
    description: "The one-time deposit address. Null until the payer signs the wallet attestation.",
  },
  {
    name: "address_explorer_url",
    type: "string | null",
    description: "Explorer link for the address, when the chain has one.",
  },
  { name: "payout_address", type: "string", description: "Your wallet; receives exactly amount." },
  {
    name: "payer_wallet",
    type: "string | null",
    description: "The wallet the payer attested. Only its transfers are the payer's.",
  },
  {
    name: "recovery_address",
    type: "string | null",
    description: "Always equal to payer_wallet: where returns go.",
  },
  {
    name: "wallet_bound_at",
    type: "timestamp | null",
    description: "When the attestation was accepted and the address derived.",
  },
  {
    name: "amount, received, remaining",
    type: "decimal strings",
    description: (
      <>
        Six-decimal USDC, each beside an integer <code>_base_units</code> twin.
      </>
    ),
  },
  {
    name: "fee_amount, net_amount",
    type: "decimal strings",
    description: "Fees are currently zero, so net_amount equals amount.",
  },
  {
    name: "issuer, payer",
    type: "object",
    description: "The parties as snapshotted at issuance: { name, email?, details? }.",
  },
  { name: "heading, reference, notes", type: "string | null", description: "As given." },
  {
    name: "metadata",
    type: "object",
    description: "Your own keys and values. Never shown to the payer.",
  },
  {
    name: "customer_id, issuer_id",
    type: "id | null",
    description: "The saved records the request was issued from, if any.",
  },
  {
    name: "payer_policy",
    type: "object",
    description: "The full policy including its assertion. Merchant-only.",
  },
  {
    name: "attachment",
    type: "object | null",
    description: "{ id, filename, mime_type, byte_length, sha256 } when a PDF is attached.",
  },
  {
    name: "client_secret, client_secret_expires_at",
    type: "string",
    description: "merchant_session only, and only on the 201 that issued the request.",
  },
  {
    name: "verification_completed_at",
    type: "timestamp | null",
    description: "When the payer policy was satisfied.",
  },
  {
    name: "likely_unsolicited_at",
    type: "timestamp | null",
    description: "When finalized funds first arrived from a wallet other than the attested one.",
  },
  { name: "created_at, updated_at, expires_at", type: "timestamp", description: "RFC 3339 UTC." },
  {
    name: "deposited_at, settled_at, expired_at",
    type: "timestamp | null",
    description:
      "Lifecycle milestones, with deposited_at_block and settled_block beside the first two.",
  },
  {
    name: "cancellation_requested_at",
    type: "timestamp | null",
    description: "Set by the cancel route.",
  },
  {
    name: "settlement_tx_hash",
    type: "string | null",
    description:
      "The transaction that executed the contract and paid you, with settlement_explorer_url.",
  },
  {
    name: "attention",
    type: "object | null",
    description: "{ code, message, action } while the request needs attention.",
  },
  {
    name: "transfers",
    type: "Transfer[]",
    description: "Every finalized transfer to the address; see the transfers route.",
  },
  {
    name: "as_of",
    type: "object | null",
    description: "{ block, at } through which this state is committed.",
  },
  {
    name: "indexer_freshness",
    type: "object",
    description: "{ last_indexed_block, last_finalized_block, cursor_updated_at }.",
  },
  {
    name: "self_settlement",
    type: "object | null",
    description: "{ factory, salt }: what a third party needs to execute the contract.",
  },
  {
    name: "attribution",
    type: "object",
    description: "{ version, hash }: the commitment to the issued document.",
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
    description:
      "Positive USDC decimal with at most six fractional digits. Used exactly as given; nothing is summed or reconciled.",
  },
  {
    name: "payer_policy",
    type: "object",
    required: true,
    description: (
      <>
        <code>{`{"mode":"permissionless"}`}</code>,{" "}
        <code>{`{"mode":"verified_email","expected_email":"…"}`}</code>, or{" "}
        <code>{`{"mode":"merchant_session","payer_reference":"…"}`}</code>. The expected email is
        trimmed and lowercased. The payer reference is 1 to 128 bytes of printable text with no
        whitespace, case preserved: your own id for the user. Each assertion is forbidden on the
        other modes.
      </>
    ),
  },
  {
    name: "payout_address",
    type: "string",
    description: (
      <>
        Nonzero EVM address that receives exactly <code>amount</code>. Optional when{" "}
        <code>issuer_id</code> names an identity with a saved payout address; its first one is used.
      </>
    ),
  },
  {
    name: "issuer",
    type: "object",
    description: (
      <>
        The issuing party: <code>name</code> (1 to 255 bytes), optional <code>email</code> (3 to 254
        bytes), optional <code>details</code> (up to 4,000 bytes, shown verbatim). Optional when{" "}
        <code>issuer_id</code> is given; an inline party always wins.
      </>
    ),
  },
  {
    name: "payer",
    type: "object",
    description: (
      <>
        The paying party, same shape. Optional when <code>customer_id</code> is given. A{" "}
        <code>payer.email</code> is also where Payday emails the issued request, except for
        merchant-session requests.
      </>
    ),
  },
  {
    name: "issuer_id",
    type: "iss_ id",
    description:
      "A saved issuer identity. Stored immutably beside the request; supplies the issuer party and payout address when they are left out.",
  },
  {
    name: "customer_id",
    type: "cus_ id",
    description:
      "A saved customer. Supplies the payer party when it is left out; the request still stores its own snapshot.",
  },
  {
    name: "heading",
    type: "string",
    description: "Up to 200 bytes. Shown to the payer before verification on gated requests.",
  },
  {
    name: "reference",
    type: "string",
    description: "Your own reference, up to 128 characters. Filterable on the list route.",
  },
  {
    name: "notes",
    type: "string",
    description: "Up to 4,000 bytes, shown to the payer after verification.",
  },
  {
    name: "attachment_id",
    type: "att_ id",
    description: "A finalized attachment. One PDF per request.",
  },
  {
    name: "expires_in",
    type: "integer",
    description:
      "Lifetime in seconds. Mutually exclusive with expires_at. Default 24 hours; 10 minutes to 366 days.",
  },
  { name: "expires_at", type: "string", description: "RFC 3339 deadline. Same window." },
  {
    name: "metadata",
    type: "object",
    description:
      "Up to 16 keys, each value at most 512 encoded bytes. Returned to you and carried on webhooks; never shown to the payer.",
  },
  {
    name: "chain_id, token_address",
    type: "string",
    description:
      "Deployment overrides. Leave them out; the environment's chain and USDC contract apply.",
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
      summary:
        "Issue a deposit request. The response is the full object with a deposit_url to give the payer; the one-time address arrives later, once the payer signs from their wallet.",
      body: (
        <>
          <p>
            The smallest valid request names saved records and nothing else: <code>amount</code>,{" "}
            <code>issuer_id</code>, <code>customer_id</code>, and <code>payer_policy</code>. Every
            immutable field takes part in idempotency, so a retry with the same key and body returns
            the original instead of issuing twice.
          </p>
          <p>
            <code>address</code>, <code>payer_wallet</code>, <code>recovery_address</code>,{" "}
            <code>wallet_bound_at</code>, and <code>self_settlement</code> are <code>null</code> on
            this response and stay so until the payer signs the wallet attestation on the hosted
            page. Wait for <code>deposit_request.ready</code>, or poll until <code>address</code> is
            set, before quoting an address anywhere. Recovery is never a request field: the
            payer&apos;s attested wallet is the recovery address, always.
          </p>
        </>
      ),
      headers: [
        {
          name: "Idempotency-Key",
          type: "string",
          required: true,
          description: "1 to 255 bytes. Use your own order or invoice id.",
        },
      ],
      bodyFields: CREATE_BODY,
      response: {
        description: (
          <>
            The deposit request object. For <code>merchant_session</code> it also carries{" "}
            <code>client_secret</code> and <code>client_secret_expires_at</code>, exactly once.
          </>
        ),
      },
      answers: [
        { status: 201, when: "Issued." },
        {
          status: 200,
          when: "An idempotent replay: the original, with Idempotency-Replayed: true.",
        },
        {
          status: 400,
          code: "invalid_request",
          when: "A field is missing, unknown, malformed, or carries a control character; the message names it.",
        },
        {
          status: 400,
          code: "invalid_amount",
          when: "Bad syntax, more than six decimals, or not positive.",
        },
        { status: 400, code: "missing_idempotency_key", when: "The header is absent." },
        {
          status: 404,
          code: "customer_not_found / issuer_not_found",
          when: "The named record is not yours.",
        },
        {
          status: 409,
          code: "idempotency_conflict",
          when: "The key was used with a different document. Fetch the original.",
        },
        {
          status: 409,
          code: "attachment_not_ready",
          when: "The attachment is not finalized, was rejected, or expired unused.",
        },
        {
          status: 409,
          code: "attachment_already_attached",
          when: "The attachment belongs to another request.",
        },
        {
          status: 409,
          code: "account_contact_required",
          when: "Sign in to the dashboard again to attach a verified email to the account.",
        },
        {
          status: 422,
          code: "unsupported_chain / unsupported_token",
          when: "An override the environment does not serve.",
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
  "INV-1042", // idempotency key
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
      summary:
        "Deposit requests newest first, as summaries with what a list needs to render and link each row without a second read.",
      query: [
        { name: "status", type: "string", description: "One public status." },
        { name: "reference", type: "string", description: "Exact match on your reference." },
        {
          name: "customer_id",
          type: "cus_ id",
          description: "Only requests addressed to this customer.",
        },
        {
          name: "issuer_id",
          type: "iss_ id",
          description: "Only requests issued under this identity.",
        },
        {
          name: "verification",
          type: "string",
          description: (
            <>
              <code>not_required</code>, <code>pending</code>, <code>verified</code>, or{" "}
              <code>likely_unsolicited</code>. Separate from status: a gated request can be funded
              before its payer has verified.
            </>
          ),
        },
        { name: "limit", type: "integer", description: "1 to 100, default 20." },
        { name: "starting_after", type: "dr_ id", description: "The previous page's next_cursor." },
      ],
      response: {
        description: (
          <>
            <code>{`{ deposit_requests: DepositRequestSummary[], next_cursor: dr_ id | null }`}</code>
            . A summary carries <code>id</code>, <code>deposit_url</code>, <code>heading</code>,{" "}
            <code>payer_name</code>, <code>reference</code>, <code>metadata</code>,{" "}
            <code>payer_policy_mode</code>, <code>customer_id</code>, <code>issuer_id</code>,{" "}
            <code>has_attachment</code>, <code>verification_completed_at</code>,{" "}
            <code>likely_unsolicited_at</code>, <code>status</code>, <code>amount</code>,{" "}
            <code>received</code>, <code>cancellation_requested_at</code>, <code>created_at</code>,{" "}
            <code>updated_at</code>, and <code>expires_at</code>.
          </>
        ),
      },
      examples: {
        curl: `curl -fsS "$API/v1/deposit-requests?status=partially_deposited&limit=20" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const page = await payday.depositRequests.list({ status: "partially_deposited", limit: 20 });
// page.next_cursor → pass as starting_after for the next page`,
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
      summary:
        "One deposit request, by its dr_ id or by its deposit address. Add wait_for=change to long-poll.",
      pathParams: [
        {
          name: "id",
          type: "dr_ id | address",
          required: true,
          description: "A complete id in canonical form, or the one-time address.",
        },
      ],
      query: [
        {
          name: "wait_for",
          type: '"change"',
          description: (
            <>
              Long-poll: the response returns when <code>updated_at</code> changes or the timeout
              elapses.
            </>
          ),
        },
        {
          name: "timeout",
          type: "integer",
          description: "1 to 30 seconds, default 30. Only with wait_for.",
        },
      ],
      response: { description: "The deposit request object." },
      answers: [
        { status: 200, when: "The deposit request." },
        { status: 400, code: "invalid_request", when: "A partial id. Ids must be complete." },
        { status: 404, code: "deposit_request_not_found", when: "Missing, or another account's." },
      ],
      examples: {
        curl: `curl -fsS "$API/v1/deposit-requests/dr_0198f80c-8d2f-7dc1-a369-90556a64f700?wait_for=change&timeout=30" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const request = await payday.depositRequests.get(id);
// or block until something changes, up to 30 s:
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
      summary:
        "Record a cancellation. Presentation only: it asks Payday's own pages to stop showing the request, but cannot disable the address or change the terms the address commits to.",
      body: (
        <p>
          A transfer sent after cancellation is still detected and routed under the address&apos;s
          terms. To change anything about a request, cancel it and issue another.
        </p>
      ),
      pathParams: [
        { name: "id", type: "dr_ id", required: true, description: "The deposit request." },
      ],
      response: {
        description: (
          <>
            The deposit request with <code>cancellation_requested_at</code> set.
          </>
        ),
      },
      examples: {
        curl: `curl -fsS -X POST "$API/v1/deposit-requests/dr_0198f80c-…/cancel" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const cancelled = await payday.depositRequests.cancel(id);
cancelled.cancellation_requested_at; // "2026-09-06T12:30:00Z"`,
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
      summary:
        "Every finalized transfer to the one-time address: who sent it, when, and what became of it.",
      pathParams: [
        { name: "id", type: "dr_ id", required: true, description: "The deposit request." },
      ],
      response: {
        fields: [
          { name: "timestamp", type: "timestamp", description: "Block time of the transfer." },
          {
            name: "amount, amount_base_units",
            type: "strings",
            description: "The transferred USDC.",
          },
          {
            name: "sender",
            type: "string",
            description: "The sending wallet. Compare with payer_wallet.",
          },
          {
            name: "transaction_hash",
            type: "string",
            description: "With explorer_url when the chain has an explorer.",
          },
          { name: "block", type: "string", description: "Block number." },
          {
            name: "disposition",
            type: "string",
            description: (
              <>
                <code>credited</code>, <code>late</code>, or <code>zero</code>.
              </>
            ),
          },
          {
            name: "collected",
            type: "boolean",
            description: "Whether settlement or a return has moved the funds on.",
          },
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
      summary:
        "The merchant's verification view: each fact on its own and every attempt the payer made.",
      body: (
        <p>
          Facts are <code>not_required</code>, <code>pending</code>, or <code>approved</code>;{" "}
          <code>wallet</code> is never <code>not_required</code>. Attempts are <code>email</code>,{" "}
          <code>wallet</code>, or <code>merchant_session</code>, each <code>pending</code>,{" "}
          <code>approved</code>, or <code>abandoned</code>. The payer&apos;s session, the client
          secret, and the code are never here.
        </p>
      ),
      pathParams: [
        { name: "id", type: "dr_ id", required: true, description: "The deposit request." },
      ],
      response: {
        fields: [
          { name: "payer_policy_mode", type: "string", description: "The request's policy mode." },
          {
            name: "verification_completed_at",
            type: "timestamp | null",
            description: "When the policy was satisfied.",
          },
          {
            name: "likely_unsolicited_at",
            type: "timestamp | null",
            description: "When funds first arrived from another wallet.",
          },
          {
            name: "facts",
            type: "object",
            description: "{ email, merchant_session, wallet, complete }.",
          },
          {
            name: "attempts",
            type: "object[]",
            description: "{ id, kind, status, verified_at, created_at } each.",
          },
        ],
      },
      examples: {
        curl: `curl -fsS "$API/v1/deposit-requests/dr_0198f80c-…/verification" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const detail = await payday.depositRequests.verification(id);
detail.facts.complete; // true once the policy is satisfied`,
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
      summary:
        "Mint a fresh single-use client secret for a merchant-session request, for a user your application signs in again after the first secret was spent or expired.",
      body: (
        <p>
          Earlier unspent secrets stay valid until they expire, so retrying a redirect never breaks
          a link already sent. Put the secret in the deposit link&apos;s fragment (
          <code>deposit_url#cs=…</code>); the SDK&apos;s <code>checkoutUrl</code> does this. The
          response is not cached.
        </p>
      ),
      pathParams: [
        {
          name: "id",
          type: "dr_ id",
          required: true,
          description: "A merchant_session deposit request.",
        },
      ],
      response: {
        fields: [
          {
            name: "client_secret",
            type: "string",
            description: "Returned once; Payday stores only its hash.",
          },
          { name: "expires_at", type: "timestamp", description: "Fifteen minutes out." },
        ],
      },
      answers: [
        { status: 201, when: "A secret valid for fifteen minutes." },
        { status: 409, code: "verification_not_required", when: "The request is permissionless." },
        {
          status: 409,
          code: "verification_method_not_applicable",
          when: "The request is verified_email.",
        },
        {
          status: 410,
          code: "deposit_request_not_payable",
          when: "Closed without verifying. A settled request that did verify still mints, to reopen its receipt.",
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
        "A session that opens the request's payer view unlocked, for its own issuer, so you can see a gated request exactly as a verified payer would before sending it.",
      body: (
        <p>
          This is not verification: it records no attempt and never marks the request verified. Put
          the session in the link&apos;s fragment (<code>deposit_url#ps=…</code>); the SDK&apos;s{" "}
          <code>previewUrl</code> does this. Works for every policy.
        </p>
      ),
      pathParams: [
        { name: "id", type: "dr_ id", required: true, description: "The deposit request." },
      ],
      response: {
        fields: [
          { name: "payer_session", type: "string", description: "An opaque preview session." },
          { name: "expires_at", type: "timestamp", description: "When it stops opening the page." },
        ],
      },
      answers: [
        {
          status: 410,
          code: "deposit_request_not_payable",
          when: "The request closed without verifying.",
        },
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
      summary: "The attached PDF's descriptor with a signed download_url valid for a few minutes.",
      pathParams: [
        { name: "id", type: "dr_ id", required: true, description: "The deposit request." },
      ],
      response: {
        fields: [
          { name: "id", type: "att_ id", description: "The attachment." },
          { name: "filename, mime_type", type: "string", description: "Always application/pdf." },
          { name: "byte_length", type: "string", description: "Decimal byte count." },
          {
            name: "sha256",
            type: "string",
            description: "Hash of the stored bytes; committed into the address.",
          },
          { name: "download_url", type: "string", description: "Signed, short-lived." },
        ],
      },
      answers: [
        { status: 404, code: "attachment_not_found", when: "The request has no attachment." },
      ],
      examples: {
        curl: `curl -fsS "$API/v1/deposit-requests/dr_0198f80c-…/attachment" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const pdf = await payday.depositRequests.attachment(id);
// fetch pdf.download_url within a few minutes`,
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
        "Payday's own PDF rendering of the request. Deterministic: the same request always produces byte-identical output, so two copies can be compared by hash.",
      pathParams: [
        { name: "id", type: "dr_ id", required: true, description: "The deposit request." },
      ],
      response: { description: "application/pdf bytes." },
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
      summary:
        "The offline-verifiable record for a settled request: the canonical document, the payer's wallet attestation, the salt and addresses, the credited transfers, the settlement transaction, and Payday's signed verification facts.",
      body: (
        <p>
          See <a href="/docs/proof-of-payment">Proof of Payment</a> for what each field
          commits to and how to verify one.
        </p>
      ),
      pathParams: [
        { name: "id", type: "dr_ id", required: true, description: "A settled deposit request." },
      ],
      response: { description: "The payday.proof.v2 document." },
      answers: [
        { status: 200, when: "The proof." },
        { status: 409, code: "deposit_request_not_settled", when: "Not settled yet." },
        {
          status: 409,
          code: "deposit_sender_mismatch",
          when: "A credited transfer came from a wallet other than the attested one; no proof can claim the payer paid it.",
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
