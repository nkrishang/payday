import type { EndpointGroup, FieldDoc } from "./types";

const SESSION_HEADER: FieldDoc[] = [
  {
    name: "Payday-Payer-Session",
    type: "string",
    description:
      "The payer session from verification, a client-secret exchange, or a wallet challenge.",
  },
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
      summary:
        "The request as the payer may see it. Always the issuer name, heading, status, and requirements; the amounts and details once unlocked; the address and QR once a wallet is bound.",
      body: (
        <>
          <p>
            For a permissionless request everything but the address is present at once. For the
            gated modes <code>chain</code>, <code>token</code>, the amounts, and{" "}
            <code>details</code> are <code>null</code> until the session satisfies the policy, and{" "}
            <code>settlement_tx_hash</code> is withheld too, since it would reveal the amount and
            payout address. <code>payer_wallet</code>, <code>address</code>, and{" "}
            <code>deposit_uri</code> appear once a wallet is bound; <code>deposit_uri</code> becomes{" "}
            <code>null</code> again once the request is not payable.
          </p>
          <p>
            With a session, <code>requirements</code> reports that session&apos;s facts; without one
            it reports what the request as a whole has completed, which never unlocks a read. The
            response carries no merchant data: no payout or recovery address, metadata, customer, or
            policy assertion, only a masked email hint.
          </p>
        </>
      ),
      headers: SESSION_HEADER,
      pathParams: [
        { name: "id", type: "dr_ id", required: true, description: "From the deposit link." },
      ],
      response: {
        fields: [
          { name: "issuer_name, heading", type: "string", description: "Always shown." },
          {
            name: "payer_policy",
            type: "object",
            description: "{ mode, expected_email_hint }; the hint is null for merchant sessions.",
          },
          {
            name: "requirements",
            type: "object",
            description: "{ email, wallet, merchant_session, complete }.",
          },
          {
            name: "status, payable, expires_at",
            type: "…",
            description: "Lifecycle; payable is false once the address must stop being shown.",
          },
          {
            name: "server_timestamp",
            type: "string",
            description: "Unix seconds. Render the countdown from this, never the device clock.",
          },
          {
            name: "content_unlocked",
            type: "boolean",
            description: "Whether the gated fields are present.",
          },
          {
            name: "chain, token, amount, received, remaining",
            type: "… | null",
            description: "Present once unlocked, with base-unit twins.",
          },
          {
            name: "payer_wallet, address, address_explorer_url, deposit_uri",
            type: "string | null",
            description: "Present once a wallet is bound.",
          },
          {
            name: "details",
            type: "object | null",
            description: "{ amount, payer, notes, reference, attachment } once unlocked.",
          },
          {
            name: "payer_message",
            type: "string | null",
            description: "Safety guidance, only when the request needs attention.",
          },
        ],
      },
      answers: [
        {
          status: 401,
          code: "invalid_deposit_link",
          when: "The id does not resolve to a request.",
        },
      ],
      examples: {
        curl: `curl -fsS "$API/v1/payer/deposit-requests/dr_0198f80c-8d2f-7dc1-a369-90556a64f700" \\
  -H "Payday-Payer-Session: $PAYER_SESSION"   # omit for a first read`,
        ts: `import { PaydayPayerClient } from "@payday/sdk";

const payer = new PaydayPayerClient(); // no key
const view = await payer.depositRequests.get(id, { payerSession });`,
        response: UNLOCKED,
        responseTitle: "200 OK, unlocked and bound",
      },
    },
    {
      slug: "qr",
      title: "Get the QR code",
      method: "GET",
      path: "/v1/payer/deposit-requests/{id}/qr",
      auth: "payer_session",
      summary: "An SVG encoding deposit_uri: the EIP-681 request for the amount still due.",
      headers: SESSION_HEADER,
      pathParams: [
        { name: "id", type: "dr_ id", required: true, description: "From the deposit link." },
      ],
      response: { description: "image/svg+xml." },
      answers: [
        { status: 401, code: "verification_required", when: "The content is still gated." },
        {
          status: 409,
          code: "wallet_required",
          when: "No wallet is bound; there is no address yet.",
        },
        {
          status: 410,
          code: "deposit_request_not_payable",
          when: "The address must no longer be presented.",
        },
      ],
      examples: {
        curl: `curl -fsS "$API/v1/payer/deposit-requests/dr_0198f80c-…/qr" \\
  -H "Payday-Payer-Session: $PAYER_SESSION" -o qr.svg`,
        ts: `const svg = await payer.depositRequests.qr(id, payerSession); // Blob for an <img>`,
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
      summary:
        "The attached PDF's descriptor with a short-lived download_url, once the content is unlocked.",
      headers: SESSION_HEADER,
      pathParams: [
        { name: "id", type: "dr_ id", required: true, description: "From the deposit link." },
      ],
      response: { description: "{ id, filename, mime_type, byte_length, sha256, download_url }." },
      answers: [
        { status: 401, code: "verification_required", when: "The content is still gated." },
        { status: 404, code: "attachment_not_found", when: "The request has no attachment." },
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
      summary: "The session's requirements, or the request's as a whole without a session.",
      headers: SESSION_HEADER,
      pathParams: [
        { name: "id", type: "dr_ id", required: true, description: "From the deposit link." },
      ],
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
      summary:
        "Send a code to the mailbox the merchant asserted. The request names no mailbox. Answers the payer session the code belongs to.",
      body: (
        <p>
          The session is an opaque token valid for 24 hours, of which Payday stores only a hash.
          Sending it back in the header on a second <code>start</code> resends the code on the same
          session. At most one code per request per minute, whoever asks. This write is answered
          cross-origin for the hosted checkout only.
        </p>
      ),
      headers: [{ ...SESSION_HEADER[0]!, description: "Optional: resend on an existing session." }],
      pathParams: [
        { name: "id", type: "dr_ id", required: true, description: "A verified_email request." },
      ],
      response: {
        fields: [
          {
            name: "payer_session",
            type: "string",
            description: "Send it in Payday-Payer-Session from now on.",
          },
          { name: "expires_at", type: "timestamp", description: "24 hours out." },
        ],
      },
      answers: [
        { status: 429, code: "otp_resend_cooldown", when: "One code per minute; see Retry-After." },
        { status: 409, code: "verification_not_required", when: "The request is permissionless." },
        {
          status: 409,
          code: "verification_method_not_applicable",
          when: "The request is a merchant session; it sends no codes.",
        },
        {
          status: 410,
          code: "deposit_request_not_payable",
          when: "Closed without verifying. A settled request that did verify re-proves the mailbox to reopen its receipt.",
        },
        {
          status: 503,
          code: "verification_unavailable",
          when: "The environment has no email verification configured.",
        },
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
      summary:
        "Exchange the code on the session. The content unlocks and verification.approved is raised.",
      body: (
        <p>
          If the code was accepted but could not be recorded, the answer is{" "}
          <code>503 verification_persistence_unavailable</code> with a short-lived{" "}
          <code>continuation</code>: retry this route with <code>{`{ "continuation": "…" }`}</code>{" "}
          and the same session instead of a new code.
        </p>
      ),
      headers: SESSION_HEADER,
      pathParams: [
        { name: "id", type: "dr_ id", required: true, description: "A verified_email request." },
      ],
      bodyFields: [
        { name: "otp", type: "string", description: "The six digits from the email." },
        {
          name: "continuation",
          type: "string",
          description: "Instead of otp, after a 503 with one.",
        },
      ],
      response: { description: "{ requirements }." },
      answers: [
        { status: 401, code: "otp_invalid", when: "Wrong, spent, or expired." },
        {
          status: 401,
          code: "payer_session_invalid",
          when: "The header is missing, unknown, expired, or for another request.",
        },
        {
          status: 409,
          code: "verification_not_started",
          when: "No code was sent, or it was already spent.",
        },
        {
          status: 503,
          code: "verification_persistence_unavailable",
          when: "Accepted but not recorded; retry with the continuation.",
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
      summary:
        "Exchange a merchant-session client secret for a payer session that already satisfies the policy. The exchange is the verification, and it happens exactly once per secret.",
      body: (
        <p>
          The hosted checkout reads the secret from the URL fragment (<code>#cs=…</code>), removes
          it from the address bar, and calls this. It mints a 24-hour session, records an approved
          attempt, and, while the request is live, sets <code>verification_completed_at</code>. An
          unknown, malformed, expired, or wrong-request secret gets one answer, so a guess learns
          nothing.
        </p>
      ),
      pathParams: [
        { name: "id", type: "dr_ id", required: true, description: "A merchant_session request." },
      ],
      bodyFields: [
        {
          name: "client_secret",
          type: "string",
          required: true,
          description: "From the fragment.",
        },
      ],
      response: { description: "{ payer_session, expires_at, requirements }. Not cached." },
      answers: [
        {
          status: 409,
          code: "client_secret_used",
          when: "Already exchanged; the link was opened once. Mint another.",
        },
        {
          status: 401,
          code: "client_secret_invalid",
          when: "Unknown, malformed, expired, or for another request.",
        },
        { status: 410, code: "deposit_request_not_payable", when: "Closed without verifying." },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/payer/deposit-requests/dr_0198f80c-…/session" \\
  -H "Content-Type: application/json" \\
  -d '{ "client_secret": "cs_…" }'`,
        ts: `const opened = await payer.verification.exchangeClientSecret(id, clientSecret);
opened.requirements.complete; // true`,
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
      summary:
        "Mint a one-time nonce for the wallet that will pay and return the EIP-712 document to sign. Every request, gated or not, takes this step before it has an address.",
      body: (
        <p>
          A gated request needs the session that satisfied its policy. A permissionless request with
          no session yet is given one here, returned as <code>payer_session</code>. Hand{" "}
          <code>typed_data</code> to the wallet&apos;s <code>eth_signTypedData_v4</code> verbatim.
          The challenge is void after ten minutes.
        </p>
      ),
      headers: [
        {
          ...SESSION_HEADER[0]!,
          description: "Required for gated requests; optional for permissionless ones.",
        },
      ],
      pathParams: [
        { name: "id", type: "dr_ id", required: true, description: "From the deposit link." },
      ],
      bodyFields: [
        {
          name: "wallet",
          type: "string",
          required: true,
          description: "The address that will pay.",
        },
      ],
      response: {
        fields: [
          {
            name: "payer_session",
            type: "string",
            description: "The session the challenge belongs to.",
          },
          { name: "expires_at", type: "timestamp", description: "Ten minutes out." },
          {
            name: "typed_data",
            type: "object",
            description:
              "Domain { name: Payday, version: 1, chainId, verifyingContract }, primary type PayerAttestation, message { statement, attributionHash, wallet, nonce, expiresAt }.",
          },
        ],
      },
      answers: [
        {
          status: 401,
          code: "verification_required / payer_session_invalid",
          when: "A gated request without an unlocked session.",
        },
        {
          status: 409,
          code: "wallet_already_bound",
          when: "Another wallet already won; the message names it.",
        },
        { status: 410, code: "deposit_request_not_payable", when: "Past its deadline or closed." },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/payer/deposit-requests/dr_0198f80c-…/wallet/challenge" \\
  -H "Payday-Payer-Session: $PAYER_SESSION" \\
  -H "Content-Type: application/json" \\
  -d '{ "wallet": "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed" }'`,
        ts: `const challenge = await payer.wallet.challenge(id, account, { payerSession });
const signature = await wallet.signTypedData({ account, ...challenge.typed_data });`,
        response: `{
  "payer_session": "…",
  "expires_at": "2026-09-06T12:15:00Z",
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
      summary:
        "Hand back the signature. The salt, the recovery term, and the one-time address are written together, once, and the unlocked view is returned with address and payer_wallet set.",
      body: (
        <p>
          The signature must recover to <code>wallet</code>: externally owned accounts only for now.
          Binding raises <code>deposit_request.ready</code>. From here, only transfers from this
          wallet are the payer&apos;s, and anything returned goes back to it.
        </p>
      ),
      headers: SESSION_HEADER,
      pathParams: [
        { name: "id", type: "dr_ id", required: true, description: "From the deposit link." },
      ],
      bodyFields: [
        {
          name: "wallet",
          type: "string",
          required: true,
          description: "The address that will pay.",
        },
        {
          name: "signature",
          type: "string",
          required: true,
          description: "The 65-byte signature over the challenge's typed data.",
        },
      ],
      response: { description: "The payer view, unlocked, with address and payer_wallet." },
      answers: [
        {
          status: 401,
          code: "wallet_signature_invalid",
          when: "The signature does not recover to the stated wallet.",
        },
        {
          status: 401,
          code: "verification_required / payer_session_invalid",
          when: "A gated request without an unlocked session.",
        },
        {
          status: 409,
          code: "wallet_challenge_required",
          when: "No outstanding, unexpired challenge on the session.",
        },
        { status: 409, code: "wallet_already_bound", when: "Another wallet already won." },
        { status: 410, code: "deposit_request_not_payable", when: "Past its deadline or closed." },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/payer/deposit-requests/dr_0198f80c-…/wallet/attest" \\
  -H "Payday-Payer-Session: $PAYER_SESSION" \\
  -H "Content-Type: application/json" \\
  -d '{ "wallet": "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed", "signature": "0x…" }'`,
        ts: `const ready = await payer.wallet.attest(id, account, signature, challenge.payer_session);
ready.address; // "0x2222…"`,
        response: UNLOCKED,
      },
    },
  ],
};

export const PAYER_LOCKED_EXAMPLE = LOCKED;
