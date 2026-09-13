import type { EndpointGroup, FieldDoc } from "./types";

const SESSION_HEADER: FieldDoc[] = [
  { name: "Payday-Payer-Session", type: "string", description: "Payer session token." },
];

const LOCKED = `{
  "id": "dr_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "issuer_name": "Acme LLC",
  "heading": "March retainer",
  "payer_policy": { "mode": "verified_email", "expected_email_hint": "a****@c***.example" },
  "requirements": { "email": "pending", "wallet": "pending", "merchant_session": "not_required", "complete": false },
  "status": "awaiting_deposit",
  "payable": true,
  "expires_at": "2026-09-06T13:00:00Z",
  "server_timestamp": "1757160000",
  "settlement_tx_hash": null,
  "settlement_explorer_url": null,
  "payer_message": null,
  "content_unlocked": false,
  "networks": null,
  "chain": null,
  "token": null,
  "amount": null,
  "amount_base_units": null,
  "received": null,
  "received_base_units": null,
  "remaining": null,
  "remaining_base_units": null,
  "payer_wallet": null,
  "address": null,
  "address_explorer_url": null,
  "deposit_uri": null,
  "details": null
}`;

const UNLOCKED = `{
  "id": "dr_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "issuer_name": "Acme LLC",
  "heading": "March retainer",
  "payer_policy": { "mode": "verified_email", "expected_email_hint": "a****@c***.example" },
  "requirements": { "email": "approved", "wallet": "approved", "merchant_session": "not_required", "complete": true },
  "status": "awaiting_deposit",
  "payable": true,
  "expires_at": "2026-09-06T13:00:00Z",
  "server_timestamp": "1757160400",
  "settlement_tx_hash": null,
  "settlement_explorer_url": null,
  "payer_message": null,
  "content_unlocked": true,
  "networks": [
    { "chain": { "id": "143", "name": "Monad" }, "token": { "symbol": "USDC", "address": "0x754704Bc059F8C67012fEd69BC8A327a5aafb603", "decimals": 6 } },
    { "chain": { "id": "8453", "name": "Base" }, "token": { "symbol": "USDC", "address": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", "decimals": 6 } },
    { "chain": { "id": "42161", "name": "Arbitrum One" }, "token": { "symbol": "USDC", "address": "0xaf88d065e77c8cC2239327C5EDb3A432268e5831", "decimals": 6 } }
  ],
  "chain": { "id": "143", "name": "Monad" },
  "token": { "symbol": "USDC", "address": "0x754704Bc059F8C67012fEd69BC8A327a5aafb603", "decimals": 6 },
  "amount": "10.500000",
  "amount_base_units": "10500000",
  "received": "0.000000",
  "received_base_units": "0",
  "remaining": "10.500000",
  "remaining_base_units": "10500000",
  "payer_wallet": "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
  "address": "0x2222222222222222222222222222222222222222",
  "address_explorer_url": "https://monadvision.com/address/0x2222222222222222222222222222222222222222",
  "deposit_uri": "ethereum:0x754704Bc059F8C67012fEd69BC8A327a5aafb603@143/transfer?address=0x2222222222222222222222222222222222222222&uint256=10500000",
  "details": {
    "amount": "10.500000",
    "amount_base_units": "10500000",
    "payer": { "name": "Customer Inc" },
    "notes": "Net 30. Thank you.",
    "reference": "INV-1042",
    "attachment": null
  }
}`;

const REQUIREMENTS = `{
  "requirements": { "email": "approved", "wallet": "pending", "merchant_session": "not_required", "complete": true }
}`;

export const PAYER: EndpointGroup = {
  slug: "payer",
  title: "Payer routes",
  overview: { title: "The payer view" },
  endpoints: [
    {
      slug: "get",
      title: "Get the payer view",
      method: "GET",
      path: "/v1/payer/deposit-requests/{id}",
      auth: "payer_session",
      summary: "Retrieves the payer projection of a deposit request.",
      body: (
        <>
          <p>
            Gated modes: <code>networks</code>, amounts, <code>details</code>, and{" "}
            <code>settlement_tx_hash</code> are null until the session satisfies the policy.
            Permissionless: present immediately. <code>chain</code>, <code>token</code>,{" "}
            <code>payer_wallet</code>, <code>address</code>, <code>address_explorer_url</code>, and{" "}
            <code>deposit_uri</code> are null until a wallet is bound on a chosen network;{" "}
            <code>deposit_uri</code> returns to null when <code>payable</code> is false.
          </p>
          <p>
            With a session, <code>requirements</code> reflects that session. Without one, it
            reflects the request as a whole and unlocks nothing. No merchant data is present: no
            payout or recovery address, metadata, customer, or policy assertion.
          </p>
        </>
      ),
      headers: SESSION_HEADER,
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      response: {
        fields: [
          { name: "issuer_name, heading", type: "string", description: "Always present." },
          {
            name: "payer_policy",
            type: "object",
            description: "{ mode, expected_email_hint }. Hint is null under merchant_session.",
          },
          {
            name: "requirements",
            type: "object",
            description: "{ email, wallet, merchant_session, complete }.",
          },
          {
            name: "status, payable, expires_at",
            type: "",
            description: "payable false: stop presenting the address.",
          },
          {
            name: "server_timestamp",
            type: "string",
            description: "Unix seconds. Authoritative clock for the deadline.",
          },
          { name: "content_unlocked", type: "boolean", description: "" },
          {
            name: "networks, amount, received, remaining",
            type: "| null",
            description:
              "Gated. networks lists the chains the payer may choose, each with its USDC contract. Base-unit counterparts included.",
          },
          {
            name: "chain, token",
            type: "object | null",
            description:
              "The payment's network and its USDC contract. Present once bound, or from issuance when the merchant pinned it.",
          },
          {
            name: "relay_available",
            type: "boolean",
            description:
              "Whether the payer may pay from another network through Relay: the deployment offers it, the address exists, and the request is payable.",
          },
          {
            name: "relay",
            type: "object | null",
            description:
              "The newest cross-chain payment quoted for the request: { id, status (quoted | sent | filled | failed | refunded | expired), origin_chain_id, origin_transaction_hash, fill_transaction_hash, created_at }. Gated.",
          },
          {
            name: "payer_wallet, address, address_explorer_url, deposit_uri",
            type: "string | null",
            description: "Present once bound. deposit_uri is EIP-681 for remaining, on the chosen chain.",
          },
          {
            name: "details",
            type: "object | null",
            description:
              "{ amount, amount_base_units, payer, notes, reference, attachment }. Gated.",
          },
          {
            name: "payer_message",
            type: "string | null",
            description: "Present under needs_attention.",
          },
        ],
      },
      answers: [{ status: 401, code: "invalid_deposit_link", when: "Unknown id." }],
      examples: {
        curl: `curl -fsS "$API/v1/payer/deposit-requests/dr_0198f80c-8d2f-7dc1-a369-90556a64f700" \\
  -H "Payday-Payer-Session: $PAYER_SESSION"`,
        ts: `import { PaydayPayerClient } from "@payday/sdk";

const payer = new PaydayPayerClient();
const view = await payer.depositRequests.get(id, { payerSession });`,
        response: UNLOCKED,
        responseTitle: "200 OK — unlocked, bound",
      },
    },
    {
      slug: "qr",
      title: "Get the QR code",
      method: "GET",
      path: "/v1/payer/deposit-requests/{id}/qr",
      auth: "payer_session",
      summary: "Renders deposit_uri as SVG.",
      headers: SESSION_HEADER,
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      response: { description: "image/svg+xml. Cache-Control: no-store." },
      answers: [
        { status: 401, code: "verification_required", when: "Content gated." },
        { status: 409, code: "wallet_required", when: "No wallet bound." },
        { status: 410, code: "deposit_request_not_payable", when: "" },
      ],
      examples: {
        curl: `curl -fsS "$API/v1/payer/deposit-requests/dr_0198f80c-…/qr" \\
  -H "Payday-Payer-Session: $PAYER_SESSION" -o qr.svg`,
        ts: `const svg = await payer.depositRequests.qr(id, payerSession);`,
        response: `HTTP/1.1 200 OK
Content-Type: image/svg+xml
Cache-Control: no-store`,
        responseLang: "http",
      },
    },
    {
      slug: "attachment",
      title: "Get the attachment",
      method: "GET",
      path: "/v1/payer/deposit-requests/{id}/attachment",
      auth: "payer_session",
      summary: "Retrieves the attachment descriptor with a signed download URL.",
      headers: SESSION_HEADER,
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      response: { description: "{ id, filename, mime_type, byte_length, sha256, download_url }." },
      answers: [
        { status: 401, code: "verification_required", when: "Content gated." },
        { status: 404, code: "attachment_not_found", when: "" },
      ],
      examples: {
        curl: `curl -fsS "$API/v1/payer/deposit-requests/dr_0198f80c-…/attachment" \\
  -H "Payday-Payer-Session: $PAYER_SESSION"`,
        ts: `const pdf = await payer.depositRequests.attachment(id, payerSession);`,
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
      slug: "verify-status",
      title: "Get verification status",
      method: "GET",
      path: "/v1/payer/deposit-requests/{id}/verify",
      auth: "payer_session",
      summary: "Retrieves requirements for the session, or for the request without one.",
      headers: SESSION_HEADER,
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      response: { description: "{ requirements }." },
      examples: {
        curl: `curl -fsS "$API/v1/payer/deposit-requests/dr_0198f80c-…/verify" \\
  -H "Payday-Payer-Session: $PAYER_SESSION"`,
        ts: `const { requirements } = await payer.verification.status(id, { payerSession });`,
        response: REQUIREMENTS,
      },
    },
    {
      slug: "verify-email-start",
      title: "Send the email code",
      method: "POST",
      path: "/v1/payer/deposit-requests/{id}/verify/email/start",
      auth: "none",
      summary: "Sends a one-time code to the asserted mailbox and mints a payer session.",
      body: (
        <p>
          The request carries no mailbox. Session: opaque, 24 hours, stored hashed. A session in the
          header resends on that session. One code per request per minute. Cross-origin: hosted
          checkout origin only.
        </p>
      ),
      headers: [{ ...SESSION_HEADER[0]!, description: "Optional. Resend on an existing session." }],
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      response: {
        fields: [
          { name: "payer_session", type: "string", description: "" },
          { name: "expires_at", type: "timestamp", description: "" },
        ],
      },
      answers: [
        { status: 429, code: "otp_resend_cooldown", when: "Retry-After set." },
        { status: 409, code: "verification_not_required", when: "Mode is permissionless." },
        {
          status: 409,
          code: "verification_method_not_applicable",
          when: "Mode is merchant_session.",
        },
        {
          status: 410,
          code: "deposit_request_not_payable",
          when: "Closed without verification. A settled, verified request re-proves for receipt access.",
        },
        { status: 503, code: "verification_unavailable", when: "No identity provider configured." },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/payer/deposit-requests/dr_0198f80c-…/verify/email/start"`,
        ts: `const { payer_session } = await payer.verification.startEmail(id);`,
        response: `{
  "payer_session": "…",
  "expires_at": "2026-09-07T12:00:00Z"
}`,
      },
    },
    {
      slug: "verify-email-confirm",
      title: "Confirm the email code",
      method: "POST",
      path: "/v1/payer/deposit-requests/{id}/verify/email/confirm",
      auth: "payer_session",
      summary: "Confirms the code on the session. Unlocks content; raises verification.approved.",
      body: (
        <p>
          On <code>503 verification_persistence_unavailable</code> the body carries a{" "}
          <code>continuation</code> valid five minutes. Retry this route with{" "}
          <code>{`{ "continuation" }`}</code> on the same session.
        </p>
      ),
      headers: SESSION_HEADER,
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      bodyFields: [
        { name: "otp", type: "string", description: "Six digits." },
        { name: "continuation", type: "string", description: "Exclusive with otp." },
      ],
      response: { description: "{ requirements }." },
      answers: [
        { status: 401, code: "otp_invalid", when: "" },
        { status: 401, code: "payer_session_invalid", when: "" },
        { status: 409, code: "verification_not_started", when: "No outstanding code." },
        {
          status: 503,
          code: "verification_persistence_unavailable",
          when: "Retry with continuation.",
        },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/payer/deposit-requests/dr_0198f80c-…/verify/email/confirm" \\
  -H "Payday-Payer-Session: $PAYER_SESSION" \\
  -H "Content-Type: application/json" \\
  -d '{ "otp": "123456" }'`,
        ts: `const { requirements } = await payer.verification.confirmEmail(id, "123456", payer_session);`,
        response: REQUIREMENTS,
      },
    },
    {
      slug: "session",
      title: "Exchange a client secret",
      method: "POST",
      path: "/v1/payer/deposit-requests/{id}/session",
      auth: "none",
      summary: "Exchanges a merchant_session client secret for a payer session. Single use.",
      body: (
        <p>
          The exchange is the verification: mints a 24-hour session satisfying the policy, records
          an approved <code>merchant_session</code> attempt, and sets{" "}
          <code>verification_completed_at</code> while the request is live. Unknown, malformed,
          expired, and foreign secrets receive one answer. Response is{" "}
          <code>Cache-Control: no-store</code>.
        </p>
      ),
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      bodyFields: [{ name: "client_secret", type: "string", required: true, description: "" }],
      response: { description: "{ payer_session, expires_at, requirements }." },
      answers: [
        { status: 409, code: "client_secret_used", when: "Already exchanged." },
        { status: 401, code: "client_secret_invalid", when: "" },
        { status: 410, code: "deposit_request_not_payable", when: "Closed without verification." },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/payer/deposit-requests/dr_0198f80c-…/session" \\
  -H "Content-Type: application/json" \\
  -d '{ "client_secret": "cs_…" }'`,
        ts: `const opened = await payer.verification.exchangeClientSecret(id, clientSecret);`,
        response: `{
  "payer_session": "…",
  "expires_at": "2026-09-07T12:00:00Z",
  "requirements": { "email": "not_required", "wallet": "pending", "merchant_session": "approved", "complete": true }
}`,
      },
    },
    {
      slug: "wallet-challenge",
      title: "Create a wallet challenge",
      method: "POST",
      path: "/v1/payer/deposit-requests/{id}/wallet/challenge",
      auth: "payer_session",
      summary: "Mints a one-time nonce and returns the EIP-712 attestation to sign.",
      body: (
        <p>
          Required for every request before an address exists. Gated modes require a session that
          satisfies the policy; permissionless requests without a session receive one. The payer
          chooses the network here: <code>chain_id</code> must be one of the request&apos;s{" "}
          <code>networks</code> (a single entry when the merchant pinned it), and the typed
          data&apos;s domain names that chain and its factory,
          so the wallet must be on it to sign. The attestation binds the wallet to that chain and
          the address exists only there. Pass <code>typed_data</code> to{" "}
          <code>eth_signTypedData_v4</code> unmodified. Challenge validity: ten minutes.
        </p>
      ),
      headers: [{ ...SESSION_HEADER[0]!, description: "Required for gated modes." }],
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      bodyFields: [
        { name: "wallet", type: "string", required: true, description: "Paying wallet." },
        {
          name: "chain_id",
          type: "string",
          required: true,
          description: "The chosen network. One of the request's networks[].chain.id.",
        },
      ],
      response: {
        fields: [
          { name: "payer_session", type: "string", description: "" },
          { name: "expires_at", type: "timestamp", description: "" },
          { name: "chain", type: "object", description: "{ id, name }. The chosen network." },
          {
            name: "typed_data",
            type: "object",
            description:
              'Domain { name: "Payday", version: "1", chainId, verifyingContract } for the chosen network. Primary type PayerAttestation. Message { statement, attributionHash, wallet, nonce, expiresAt }.',
          },
        ],
      },
      answers: [
        {
          status: 401,
          code: "verification_required / payer_session_invalid",
          when: "Gated; session missing or unsatisfied.",
        },
        { status: 409, code: "wallet_already_bound", when: "Message names the bound wallet." },
        { status: 410, code: "deposit_request_not_payable", when: "" },
        { status: 422, code: "unsupported_chain", when: "chain_id is not one of the request's networks." },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/payer/deposit-requests/dr_0198f80c-…/wallet/challenge" \\
  -H "Payday-Payer-Session: $PAYER_SESSION" \\
  -H "Content-Type: application/json" \\
  -d '{ "wallet": "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed", "chain_id": "143" }'`,
        ts: `const challenge = await payer.wallet.challenge(id, account, "143", { payerSession });
const signature = await wallet.signTypedData({ account, ...challenge.typed_data });`,
        response: `{
  "payer_session": "…",
  "expires_at": "2026-09-06T12:15:00Z",
  "chain": { "id": "143", "name": "Monad" },
  "typed_data": {
    "domain": { "name": "Payday", "version": "1", "chainId": 143, "verifyingContract": "0x…" },
    "primaryType": "PayerAttestation",
    "types": { "EIP712Domain": [ "…" ], "PayerAttestation": [ "…" ] },
    "message": {
      "statement": "I will pay this Payday deposit request from this wallet.",
      "attributionHash": "0x…",
      "wallet": "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
      "nonce": "0x…",
      "expiresAt": 1757160900
    }
  }
}`,
      },
    },
    {
      slug: "wallet-attest",
      title: "Attest the wallet",
      method: "POST",
      path: "/v1/payer/deposit-requests/{id}/wallet/attest",
      auth: "payer_session",
      summary: "Submits the signature. Binds the wallet and derives the deposit address.",
      body: (
        <p>
          Signature must recover to <code>wallet</code> (EOA only). Salt, recovery term, and address
          are written atomically, once. Raises <code>deposit_request.ready</code>.
        </p>
      ),
      headers: SESSION_HEADER,
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      bodyFields: [
        { name: "wallet", type: "string", required: true, description: "" },
        { name: "signature", type: "string", required: true, description: "65 bytes, 0x hex." },
      ],
      response: { description: "Payer view, unlocked, with address and payer_wallet." },
      answers: [
        { status: 401, code: "wallet_signature_invalid", when: "" },
        { status: 401, code: "verification_required / payer_session_invalid", when: "" },
        {
          status: 409,
          code: "wallet_challenge_required",
          when: "No unexpired challenge on the session.",
        },
        { status: 409, code: "wallet_already_bound", when: "" },
        { status: 410, code: "deposit_request_not_payable", when: "" },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/payer/deposit-requests/dr_0198f80c-…/wallet/attest" \\
  -H "Payday-Payer-Session: $PAYER_SESSION" \\
  -H "Content-Type: application/json" \\
  -d '{ "wallet": "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed", "signature": "0x…" }'`,
        ts: `const ready = await payer.wallet.attest(id, account, signature, challenge.payer_session);`,
        response: UNLOCKED,
      },
    },
    {
      slug: "relay-chains",
      title: "Networks to pay from",
      method: "GET",
      path: "/v1/payer/deposit-requests/{id}/relay/chains",
      auth: "payer_session",
      summary: "The networks USDC may be paid from through Relay.",
      body: (
        <p>
          Every network Relay takes USDC deposits on, except the request&apos;s own. Requires the
          address (<code>409 wallet_required</code> before) and a payable request. Each entry
          carries the network&apos;s USDC contract, a public RPC and explorer, and an icon, so a
          wallet that lacks the network can be asked to add it.
        </p>
      ),
      headers: [{ ...SESSION_HEADER[0]!, description: "Required for gated modes." }],
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      response: {
        fields: [
          {
            name: "chains[]",
            type: "object",
            description:
              "{ chain_id, name, native_symbol, usdc_address, explorer_url, icon_url, rpc_url }.",
          },
        ],
      },
      answers: [
        { status: 404, code: "relay_unavailable", when: "This deployment has no Relay key." },
        { status: 409, code: "wallet_required", when: "" },
        { status: 410, code: "deposit_request_not_payable", when: "" },
      ],
      examples: {
        curl: `curl -fsS "$API/v1/payer/deposit-requests/dr_0198f80c-…/relay/chains"`,
        ts: `const { chains } = await payer.relay.chains(id, { payerSession });`,
        response: `{
  "chains": [
    { "chain_id": "8453", "name": "Base", "native_symbol": "ETH",
      "usdc_address": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
      "explorer_url": "https://basescan.org", "icon_url": "https://assets.relay.link/icons/8453/light.png",
      "rpc_url": "https://mainnet.base.org" }
  ]
}`,
      },
    },
    {
      slug: "relay-quote",
      title: "Quote a payment from another network",
      method: "POST",
      path: "/v1/payer/deposit-requests/{id}/relay/quotes",
      auth: "payer_session",
      summary: "Asks Relay for a route and records it as a quote.",
      body: (
        <p>
          Payday makes the quote, never the page: it pins the attested wallet as the sender, the
          payment address as the recipient, USDC on the request&apos;s network as what lands, and
          exactly the amount still due as the output. The answer is the transactions the wallet
          sends on the origin network, in order (an ERC-20 approve, then Relay&apos;s deposit),
          and what they cost. Ask again after <code>expires_at</code>. Every quote is a{" "}
          <code>rli_</code> record; only one reported as sent is followed.
        </p>
      ),
      headers: [{ ...SESSION_HEADER[0]!, description: "Required for gated modes." }],
      pathParams: [{ name: "id", type: "dr_ id", required: true, description: "" }],
      bodyFields: [
        {
          name: "origin_chain_id",
          type: "string",
          required: true,
          description: "Decimal chain id, one of the networks to pay from.",
        },
      ],
      response: {
        fields: [
          { name: "id", type: "rli_ id", description: "" },
          { name: "request_id", type: "string", description: "Relay's request id." },
          { name: "origin", type: "object", description: "The network, as listed." },
          {
            name: "amount_in, amount_out",
            type: "string",
            description:
              "USDC the wallet sends on the origin network, and exactly what lands on the payment address (the amount due). Base-unit counterparts included.",
          },
          { name: "relayer_fee_usd, time_estimate_seconds, expires_at", type: "", description: "" },
          {
            name: "steps[]",
            type: "object",
            description: "{ id: approve | deposit, transaction: { chain_id, to, data, value, gas } }.",
          },
        ],
      },
      answers: [
        { status: 404, code: "relay_unavailable", when: "" },
        { status: 409, code: "wallet_required", when: "" },
        { status: 410, code: "deposit_request_not_payable", when: "" },
        {
          status: 422,
          code: "relay_unsupported_origin",
          when: "Not one of the networks to pay from, or the request's own.",
        },
        { status: 502, code: "relay_quote_failed", when: "Relay has no route, or is unreachable." },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/payer/deposit-requests/dr_0198f80c-…/relay/quotes" \\
  -H "Content-Type: application/json" \\
  -d '{ "origin_chain_id": "8453" }'`,
        ts: `const quote = await payer.relay.quote(id, "8453", { payerSession });
for (const step of quote.steps) {
  await wallet.sendTransaction({ account, to: step.transaction.to, data: step.transaction.data, value: BigInt(step.transaction.value) });
}`,
        response: `{
  "id": "rli_0198f80c-8d2f-7dc1-a369-90556a64f7b1",
  "request_id": "0x1789…",
  "origin": { "chain_id": "8453", "name": "Base", "…": "…" },
  "amount_in": "10.520798", "amount_in_base_units": "10520798",
  "amount_out": "10.500000", "amount_out_base_units": "10500000",
  "relayer_fee_usd": "0.02", "time_estimate_seconds": 1,
  "expires_at": "2026-09-06T12:20:00Z",
  "steps": [
    { "id": "approve", "transaction": { "chain_id": "8453", "to": "0x8335…", "data": "0x095e…", "value": "0", "gas": "73112" } },
    { "id": "deposit", "transaction": { "chain_id": "8453", "to": "0x4cd0…", "data": "0xe801…", "value": "0", "gas": null } }
  ]
}`,
      },
    },
    {
      slug: "relay-sent",
      title: "Report the deposit",
      method: "POST",
      path: "/v1/payer/deposit-requests/{id}/relay/quotes/{rli}/sent",
      auth: "payer_session",
      summary: "The wallet sent the quote's deposit; Payday follows it from here.",
      body: (
        <p>
          Once. From then on the indexer asks Relay what became of the request, attributes the
          delivered USDC to the attested wallet when Relay names it as the depositor, and the payer
          view&apos;s <code>relay</code> block shows where it stands. The hash is advisory; the
          origin transaction Relay records is what the proof names.
        </p>
      ),
      headers: [{ ...SESSION_HEADER[0]!, description: "Required for gated modes." }],
      pathParams: [
        { name: "id", type: "dr_ id", required: true, description: "" },
        { name: "rli", type: "rli_ id", required: true, description: "The quote." },
      ],
      bodyFields: [
        {
          name: "transaction_hash",
          type: "string",
          required: true,
          description: "The deposit step's transaction, 0x hex, 32 bytes.",
        },
      ],
      response: { description: "Payer view, with relay.status sent." },
      answers: [
        { status: 404, code: "relay_intent_not_found", when: "Not this request's quote." },
        { status: 409, code: "relay_intent_not_quoted", when: "Already reported, or expired unsent." },
        { status: 409, code: "relay_transaction_claimed", when: "That hash already pays another quote." },
        { status: 410, code: "deposit_request_not_payable", when: "" },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/payer/deposit-requests/dr_0198f80c-…/relay/quotes/rli_0198f80c-…/sent" \\
  -H "Content-Type: application/json" \\
  -d '{ "transaction_hash": "0x…" }'`,
        ts: `const following = await payer.relay.sent(id, quote.id, depositHash, { payerSession });`,
        response: `{ "…": "the payer view", "relay": { "id": "rli_0198f80c-…", "status": "sent", "origin_chain_id": "8453", "origin_transaction_hash": "0x…", "fill_transaction_hash": null, "created_at": "2026-09-06T12:06:00Z" } }`,
      },
    },
  ],
};

export const PAYER_LOCKED_EXAMPLE = LOCKED;
