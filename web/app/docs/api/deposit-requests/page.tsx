import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { CodeTabs } from "@/components/docs/code-tabs";
import { Answers, Endpoint, Params } from "@/components/docs/endpoint";
import { Callout, DocsPage, H2, Table } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Deposit requests API",
  description:
    "Create, read, list, and cancel deposit requests; read their transfers, documents, verification, and Proof of Payment.",
};

const CREATE_BODY = `{
  "amount": "10.50",
  "payout_address": "0x1111111111111111111111111111111111111111",
  "issuer": { "name": "Acme LLC", "email": "billing@acme.example" },
  "payer": { "name": "Customer Inc", "email": "ap@customer.example", "details": "12 Main St, Springfield" },
  "heading": "March retainer",
  "reference": "INV-1042",
  "notes": "Net 30. Thank you.",
  "payer_policy": { "mode": "verified_email", "expected_email": "ap@customer.example" },
  "attachment_id": "att_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "customer_id": "cus_0198f80c-1111-7dc1-a369-90556a64f700",
  "issuer_id": "iss_0198f80c-2222-7dc1-a369-90556a64f700",
  "expires_in": 3600,
  "metadata": { "po": "PO-77" }
}`;

const CREATE_MIN = `{
  "amount": "10.50",
  "issuer_id": "iss_0198f80c-…",
  "customer_id": "cus_0198f80c-…",
  "payer_policy": { "mode": "permissionless" }
}`;

const CREATE_CURL = `curl -fsS "$API/v1/deposit-requests" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  -H "Content-Type: application/json" \\
  -H "Idempotency-Key: INV-1042" \\
  -d @request.json`;

const CREATE_TS = `const request = await payday.depositRequests.create(body, "INV-1042");`;

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

  "amount": "10.500000",            "amount_base_units": "10500000",
  "received": "0.000000",           "received_base_units": "0",
  "remaining": "10.500000",         "remaining_base_units": "10500000",
  "fee_amount": "0.000000",         "fee_amount_base_units": "0",
  "net_amount": "10.500000",        "net_amount_base_units": "10500000",

  "issuer": { "name": "Acme LLC", "email": "billing@acme.example" },
  "payer": { "name": "Customer Inc", "email": "ap@customer.example", "details": "12 Main St, Springfield" },
  "heading": "March retainer",
  "reference": "INV-1042",
  "notes": "Net 30. Thank you.",
  "metadata": { "po": "PO-77" },
  "customer_id": "cus_0198f80c-1111-7dc1-a369-90556a64f700",
  "issuer_id": "iss_0198f80c-2222-7dc1-a369-90556a64f700",
  "payer_policy": { "mode": "verified_email", "expected_email": "ap@customer.example" },
  "attachment": { "id": "att_0198f80c-…", "filename": "INV-1042.pdf", "mime_type": "application/pdf", "byte_length": "48211", "sha256": "0x9f…" },

  "verification_completed_at": null,
  "likely_unsolicited_at": null,

  "created_at": "2026-09-06T12:00:00Z",
  "updated_at": "2026-09-06T12:00:00Z",
  "expires_at": "2026-09-06T13:00:00Z",
  "deposited_at": null,             "deposited_at_block": null,
  "settled_at": null,               "settled_block": null,
  "expired_at": null,
  "cancellation_requested_at": null,
  "settlement_tx_hash": null,
  "settlement_explorer_url": null,
  "attention": null,

  "transfers": [],
  "as_of": { "block": "98765000", "at": "2026-09-06T11:59:58Z" },
  "indexer_freshness": { "last_indexed_block": "98765000", "last_finalized_block": "98765000", "cursor_updated_at": "2026-09-06T11:59:59Z" },
  "self_settlement": null,
  "attribution": { "version": 2, "hash": "0x…" }
}`;

const LIST_RESPONSE = `{
  "deposit_requests": [
    {
      "id": "dr_0198f80c-…",
      "deposit_url": "https://payday.sh/pay/dr_0198f80c-…",
      "heading": "March retainer",
      "payer_name": "Customer Inc",
      "reference": "INV-1042",
      "metadata": { "po": "PO-77" },
      "payer_policy_mode": "verified_email",
      "customer_id": "cus_0198f80c-…",
      "issuer_id": "iss_0198f80c-…",
      "has_attachment": true,
      "verification_completed_at": null,
      "likely_unsolicited_at": null,
      "status": "awaiting_deposit",
      "amount": "10.500000",
      "received": "0.000000",
      "cancellation_requested_at": null,
      "created_at": "2026-09-06T12:00:00Z",
      "updated_at": "2026-09-06T12:00:00Z",
      "expires_at": "2026-09-06T13:00:00Z"
    }
  ],
  "next_cursor": null
}`;

const TRANSFERS = `{
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
}`;

const VERIFICATION = `{
  "payer_policy_mode": "verified_email",
  "verification_completed_at": "2026-09-06T12:05:00Z",
  "likely_unsolicited_at": null,
  "facts": { "email": "approved", "merchant_session": "not_required", "wallet": "approved", "complete": true },
  "attempts": [
    { "id": "va_…", "kind": "email", "status": "approved", "verified_at": "2026-09-06T12:05:00Z", "created_at": "2026-09-06T12:04:10Z" },
    { "id": "va_…", "kind": "wallet", "status": "approved", "verified_at": "2026-09-06T12:05:40Z", "created_at": "2026-09-06T12:05:30Z" }
  ]
}`;

const ATTACHMENT = `{
  "id": "att_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "filename": "INV-1042.pdf",
  "mime_type": "application/pdf",
  "byte_length": "48211",
  "sha256": "0x9f…",
  "download_url": "https://…"
}`;

export default function DepositRequestsApiPage() {
  return (
    <DocsPage
      eyebrow="API reference"
      title="Deposit requests"
      lead="The central resource. A deposit request is the document you issue; the state of its deposit is read from it."
    >
      <Endpoint method="POST" path="/v1/deposit-requests">
        Issue a deposit request. Requires <code>Idempotency-Key</code>.
      </Endpoint>
      <CodeBlock code={CREATE_BODY} lang="json" title="Request body" />
      <Params
        title="Body"
        rows={[
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
                One of <code>{`{"mode":"permissionless"}`}</code>,{" "}
                <code>{`{"mode":"verified_email","expected_email":"…"}`}</code>, or{" "}
                <code>{`{"mode":"merchant_session","payer_reference":"…"}`}</code>. The expected
                email is trimmed and lowercased. The payer reference is 1 to 128 bytes of printable
                text with no whitespace, case preserved: your own id for the user. Each assertion is
                forbidden on the other modes.
              </>
            ),
          },
          {
            name: "payout_address",
            type: "string",
            description: (
              <>
                Nonzero EVM address that receives exactly <code>amount</code>. Optional when{" "}
                <code>issuer_id</code> names an identity with a saved payout address; its first one
                is used.
              </>
            ),
          },
          {
            name: "issuer",
            type: "object",
            description: (
              <>
                The issuing party: <code>name</code> (1 to 255 bytes), optional <code>email</code>{" "}
                (3 to 254 bytes), optional <code>details</code> (up to 4,000 bytes of free text,
                shown verbatim). Optional when <code>issuer_id</code> is given; an inline party
                always wins.
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
            description:
              "Up to 200 bytes. Shown to the payer before verification on gated requests.",
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
            description: (
              <>
                A finalized <a href="/docs/api/attachments">attachment</a>. One PDF per request.
              </>
            ),
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
        ]}
      />
      <p>The smallest valid request names saved records and nothing else:</p>
      <CodeBlock code={CREATE_MIN} lang="json" />
      <CodeTabs
        tabs={[
          { label: "curl", content: <CodeBlock code={CREATE_CURL} lang="bash" /> },
          { label: "TypeScript", content: <CodeBlock code={CREATE_TS} lang="ts" /> },
        ]}
      />
      <Answers
        rows={[
          {
            status: 201,
            when: "The deposit request. For merchant_session it also carries client_secret and client_secret_expires_at, exactly once.",
          },
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
        ]}
      />
      <Callout title="The address comes later">
        <code>address</code>, <code>payer_wallet</code>, <code>recovery_address</code>,{" "}
        <code>wallet_bound_at</code>, and <code>self_settlement</code> are <code>null</code> on the
        create response and stay so until the payer signs the wallet attestation on the hosted page.
        Wait for <code>deposit_request.ready</code>, or poll until <code>address</code> is set,
        before quoting an address anywhere. Recovery is never a request field; the payer&apos;s
        attested wallet is the recovery address, always.
      </Callout>

      <H2 id="object">The deposit request object</H2>
      <CodeBlock code={OBJECT} lang="json" />
      <Table>
        <thead>
          <tr>
            <th>Group</th>
            <th>Fields</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>Identity and instructions</td>
            <td>
              <code>id</code>, <code>deposit_url</code>, <code>chain</code>, <code>token</code>,{" "}
              <code>currency</code>, <code>address</code>, <code>address_explorer_url</code>,{" "}
              <code>payout_address</code>, <code>payer_wallet</code>, <code>recovery_address</code>{" "}
              (always equal to the payer wallet), <code>wallet_bound_at</code>,{" "}
              <code>expires_at</code>.
            </td>
          </tr>
          <tr>
            <td>Accounting</td>
            <td>
              <code>amount</code>, <code>received</code>, <code>remaining</code>,{" "}
              <code>fee_amount</code>, <code>net_amount</code>, each with a <code>_base_units</code>{" "}
              twin. Fees are currently zero.
            </td>
          </tr>
          <tr>
            <td>State</td>
            <td>
              <code>status</code>, <code>deposited_at</code> and block, <code>settled_at</code> and
              block, <code>expired_at</code>, <code>cancellation_requested_at</code>,{" "}
              <code>settlement_tx_hash</code> and explorer URL, <code>attention</code>.
            </td>
          </tr>
          <tr>
            <td>Document</td>
            <td>
              <code>issuer</code>, <code>payer</code>, <code>heading</code>, <code>reference</code>,{" "}
              <code>notes</code>, <code>metadata</code>, <code>customer_id</code>,{" "}
              <code>issuer_id</code>, the full <code>payer_policy</code> (merchant-only),{" "}
              <code>attachment</code>, <code>created_at</code>, <code>updated_at</code>.
            </td>
          </tr>
          <tr>
            <td>Verification</td>
            <td>
              <code>verification_completed_at</code>, <code>likely_unsolicited_at</code>.
            </td>
          </tr>
          <tr>
            <td>Audit and freshness</td>
            <td>
              <code>transfers</code>, <code>as_of</code>, <code>indexer_freshness</code>,{" "}
              <code>self_settlement</code>, <code>attribution</code>.
            </td>
          </tr>
        </tbody>
      </Table>
      <p>
        Statuses are <code>awaiting_deposit</code>, <code>partially_deposited</code>,{" "}
        <code>deposited</code>, <code>settled</code>, <code>expired</code>, <code>returned</code>,
        and <code>needs_attention</code>; see the <a href="/docs/concepts#lifecycle">lifecycle</a>.
      </p>

      <Endpoint method="GET" path="/v1/deposit-requests">
        List deposit requests, newest first.
      </Endpoint>
      <Params
        title="Query"
        rows={[
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
          {
            name: "starting_after",
            type: "dr_ id",
            description: "The previous page's next_cursor.",
          },
        ]}
      />
      <CodeBlock code={LIST_RESPONSE} lang="json" title="200 OK" />
      <p>Summaries carry what a list needs to render and link each row without a second read.</p>

      <Endpoint method="GET" path="/v1/deposit-requests/{id}">
        Read one deposit request, by its <code>dr_</code> id or by its deposit address.
      </Endpoint>
      <Params
        title="Query"
        rows={[
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
        ]}
      />
      <Answers
        rows={[
          { status: 200, when: "The deposit request object." },
          { status: 400, code: "invalid_request", when: "A partial id. Ids must be complete." },
          {
            status: 404,
            code: "deposit_request_not_found",
            when: "Missing, or another account's.",
          },
        ]}
      />

      <Endpoint method="POST" path="/v1/deposit-requests/{id}/cancel">
        Record a cancellation. Answers the deposit request with{" "}
        <code>cancellation_requested_at</code> set.
      </Endpoint>
      <p>
        Cancellation is presentation only. It asks Payday&apos;s own clients to stop showing the
        request, but it cannot disable an address or change the terms the address commits to. A
        transfer sent afterwards is still detected and routed under those terms.
      </p>

      <Endpoint method="GET" path="/v1/deposit-requests/{id}/transfers">
        Every finalized transfer to the address: who sent it, when, and what became of it.
      </Endpoint>
      <CodeBlock code={TRANSFERS} lang="json" title="200 OK" />
      <p>
        <code>disposition</code> is <code>credited</code>, <code>late</code>, or <code>zero</code>;{" "}
        <code>collected</code> says whether the settlement or a return has moved the funds on.
      </p>

      <Endpoint method="GET" path="/v1/deposit-requests/{id}/verification">
        The merchant&apos;s verification view: each fact on its own and every attempt.
      </Endpoint>
      <CodeBlock code={VERIFICATION} lang="json" title="200 OK" />
      <p>
        Facts are <code>not_required</code>, <code>pending</code>, or <code>approved</code>;{" "}
        <code>wallet</code> is never <code>not_required</code>. Attempts are <code>email</code>,{" "}
        <code>wallet</code>, or <code>merchant_session</code>, each <code>pending</code>,{" "}
        <code>approved</code>, or <code>abandoned</code>. The payer&apos;s session, the client
        secret, and the code are never here.
      </p>

      <Endpoint method="POST" path="/v1/deposit-requests/{id}/client-secret">
        Mint a fresh single-use client secret for a merchant-session request.
      </Endpoint>
      <p>
        For a user your application signs in again after the first secret was spent or expired.
        Earlier unspent secrets stay valid until they expire. Answers{" "}
        <code>201 {`{ client_secret, expires_at }`}</code>, not cached.
      </p>
      <Answers
        rows={[
          { status: 201, when: "A secret valid for fifteen minutes, returned once." },
          {
            status: 409,
            code: "verification_not_required",
            when: "The request is permissionless.",
          },
          {
            status: 409,
            code: "verification_method_not_applicable",
            when: "The request is verified_email.",
          },
          {
            status: 410,
            code: "deposit_request_not_payable",
            when: "The request closed without verifying. A settled request that did verify still mints, to reopen its receipt.",
          },
        ]}
      />

      <Endpoint method="POST" path="/v1/deposit-requests/{id}/preview-session">
        A session that opens the request&apos;s payer view unlocked, for its own issuer.
      </Endpoint>
      <p>
        Lets you see a gated request exactly as a verified payer would, before sending it. It is not
        verification: it records no attempt and never marks the request verified. Put the returned
        session in the link&apos;s fragment as the SDK&apos;s <code>previewUrl</code> does.
      </p>

      <Endpoint method="GET" path="/v1/deposit-requests/{id}/attachment">
        The attached PDF&apos;s descriptor with a signed <code>download_url</code> valid for a few
        minutes.
      </Endpoint>
      <CodeBlock code={ATTACHMENT} lang="json" title="200 OK" />
      <Answers
        rows={[
          { status: 404, code: "attachment_not_found", when: "The request has no attachment." },
        ]}
      />

      <Endpoint method="GET" path="/v1/deposit-requests/{id}/request.pdf">
        Payday&apos;s own PDF rendering of the request, as <code>application/pdf</code>.
      </Endpoint>
      <p>
        Deterministic: fixed fonts and object order, no timestamps or random ids, so the same
        request always produces byte-identical output.
      </p>

      <Endpoint method="GET" path="/v1/deposit-requests/{id}/proof">
        The Proof of Payment for a settled request.
      </Endpoint>
      <p>
        See <a href="/docs/proof-of-payment">Proof of Payment</a> for what it contains and how to
        verify it.
      </p>
      <Answers
        rows={[
          { status: 200, when: "The payday.proof.v2 document." },
          { status: 409, code: "deposit_request_not_settled", when: "Not settled yet." },
          {
            status: 409,
            code: "deposit_sender_mismatch",
            when: "A credited transfer came from a wallet other than the attested one; no proof can claim the payer paid it.",
          },
        ]}
      />
    </DocsPage>
  );
}
