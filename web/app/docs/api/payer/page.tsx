import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { Answers, Endpoint, Params } from "@/components/docs/endpoint";
import { Callout, DocsPage, H2 } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Payer routes API",
  description:
    "The public, keyless routes behind every deposit link: read a request, verify by email, exchange a merchant-session secret, attest a wallet, fetch the QR and PDF.",
};

const PAYER_OBJECT = `{
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

  "chain": null, "token": null,
  "amount": null, "amount_base_units": null,
  "received": null, "received_base_units": null,
  "remaining": null, "remaining_base_units": null,
  "payer_wallet": null,
  "address": null, "address_explorer_url": null,
  "deposit_uri": null,
  "details": null
}`;

const UNLOCKED = `{
  "…": "…",
  "content_unlocked": true,
  "chain": { "id": "143", "name": "Monad" },
  "token": { "symbol": "USDC", "address": "0x7547…b603", "decimals": 6 },
  "amount": "10.500000", "amount_base_units": "10500000",
  "received": "0.000000", "received_base_units": "0",
  "remaining": "10.500000", "remaining_base_units": "10500000",
  "payer_wallet": "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
  "address": "0x2222222222222222222222222222222222222222",
  "address_explorer_url": "https://monadvision.com/address/0x2222…",
  "deposit_uri": "ethereum:0x7547…b603@143/transfer?address=0x2222…&uint256=10500000",
  "details": { "amount": "10.500000", "amount_base_units": "10500000", "payer": { "name": "Customer Inc" }, "notes": null, "reference": "INV-1042", "attachment": { "id": "att_…", "filename": "INV-1042.pdf", "mime_type": "application/pdf", "byte_length": "48211", "sha256": "0x9f…" } }
}`;

const CHALLENGE = `{
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
}`;

export default function PayerApiPage() {
  return (
    <DocsPage
      eyebrow="API reference"
      title="Payer routes"
      lead="What a deposit link resolves to. These routes take no API key, expose no merchant data, and are what the hosted checkout is built on, so a checkout of your own can use them too."
    >
      <p>
        Reads answer any browser origin. A payer session token, obtained by completing verification,
        travels in the <code>Payday-Payer-Session</code> header on every read and unlocks exactly
        the request it was minted for. The writes (verification, wallet, session exchange) answer
        the hosted checkout&apos;s origin only and limit bodies to 8 KiB. JSON responses are{" "}
        <code>no-store</code>.
      </p>

      <Endpoint method="GET" path="/v1/payer/deposit-requests/{id}">
        The request as the payer may see it.
      </Endpoint>
      <CodeBlock
        code={PAYER_OBJECT}
        lang="json"
        title="Locked: a gated request without a session"
      />
      <CodeBlock
        code={UNLOCKED}
        lang="json"
        title="Unlocked and bound: after verification and the wallet step"
      />
      <ul>
        <li>
          Always present: <code>issuer_name</code>, <code>heading</code>, the policy mode with a
          masked email hint (<code>null</code> for merchant sessions), <code>requirements</code>,{" "}
          <code>status</code>, <code>payable</code>, <code>expires_at</code>,{" "}
          <code>server_timestamp</code>, and <code>content_unlocked</code>.
        </li>
        <li>
          Present once unlocked: chain, token, the amounts, and <code>details</code>. For a
          permissionless request that is immediately.
        </li>
        <li>
          Present once a wallet is bound: <code>payer_wallet</code>, <code>address</code>, its
          explorer URL, and <code>deposit_uri</code>, an EIP-681 request for the amount still due
          that becomes <code>null</code> once the request is not payable.
        </li>
        <li>
          <code>settlement_tx_hash</code> is withheld on gated requests until unlocked, since it
          would reveal the amount and payout address.
        </li>
        <li>
          With a session, <code>requirements</code> reports that session&apos;s facts; without one
          it reports what the request as a whole has completed, which never unlocks a read.
        </li>
      </ul>
      <Answers
        rows={[
          {
            status: 401,
            code: "invalid_deposit_link",
            when: "The id does not resolve to a request.",
          },
        ]}
      />

      <Endpoint method="GET" path="/v1/payer/deposit-requests/{id}/qr">
        An SVG QR encoding <code>deposit_uri</code>, as <code>image/svg+xml</code>.
      </Endpoint>
      <Answers
        rows={[
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
        ]}
      />

      <Endpoint method="GET" path="/v1/payer/deposit-requests/{id}/attachment">
        The PDF descriptor with a short-lived <code>download_url</code>.
      </Endpoint>
      <Answers
        rows={[{ status: 401, code: "verification_required", when: "The content is still gated." }]}
      />

      <H2 id="email-verification">Email verification</H2>
      <Endpoint method="POST" path="/v1/payer/deposit-requests/{id}/verify/email/start">
        Send a code to the mailbox the merchant asserted. Answers{" "}
        <code>{`{ payer_session, expires_at }`}</code>.
      </Endpoint>
      <p>
        The request names no mailbox. The session is an opaque token valid for 24 hours, of which
        Payday stores only a hash. Sending it back on a second <code>start</code> resends the code
        on the same session.
      </p>
      <Answers
        rows={[
          {
            status: 429,
            code: "otp_resend_cooldown",
            when: "One code per request per minute, whoever asks. See Retry-After.",
          },
          {
            status: 409,
            code: "verification_not_required",
            when: "The request is permissionless.",
          },
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
        ]}
      />

      <Endpoint method="POST" path="/v1/payer/deposit-requests/{id}/verify/email/confirm">
        Exchange the code: <code>{`{ "otp": "123456" }`}</code> with the session header. Answers{" "}
        <code>{`{ requirements }`}</code>.
      </Endpoint>
      <Answers
        rows={[
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
            when: "The code was accepted but could not be recorded. Retry with the returned continuation instead of a new code.",
          },
        ]}
      />

      <Endpoint method="GET" path="/v1/payer/deposit-requests/{id}/verify">
        The session&apos;s <code>requirements</code>, or the request&apos;s without a session.
      </Endpoint>

      <H2 id="merchant-session">Merchant session</H2>
      <Endpoint method="POST" path="/v1/payer/deposit-requests/{id}/session">
        Exchange a client secret: <code>{`{ "client_secret": "cs_…" }`}</code>. Answers{" "}
        <code>{`{ payer_session, expires_at, requirements }`}</code>.
      </Endpoint>
      <p>
        The exchange is the verification. It mints a 24-hour session that already satisfies the
        policy, records an approved attempt, and, while the request is live, sets{" "}
        <code>verification_completed_at</code>. The hosted checkout reads the secret from the URL
        fragment (<code>#cs=…</code>), removes it from the address bar, and calls this.
      </p>
      <Answers
        rows={[
          {
            status: 409,
            code: "client_secret_used",
            when: "Already exchanged. The link was opened once; mint another.",
          },
          {
            status: 401,
            code: "client_secret_invalid",
            when: "Unknown, malformed, expired, or for another request. One answer for all four.",
          },
          { status: 410, code: "deposit_request_not_payable", when: "Closed without verifying." },
        ]}
      />

      <H2 id="wallet-attestation">Wallet attestation</H2>
      <p>
        Every request, gated or not, takes this step before it has an address. A gated request needs
        the session that satisfied its policy; a permissionless request with no session yet is given
        one by <code>challenge</code>.
      </p>
      <Endpoint method="POST" path="/v1/payer/deposit-requests/{id}/wallet/challenge">
        Mint a one-time nonce for <code>{`{ "wallet": "0x…" }`}</code> and return the EIP-712
        document to sign.
      </Endpoint>
      <CodeBlock code={CHALLENGE} lang="json" title="200 OK" />
      <p>
        Hand <code>typed_data</code> to the wallet&apos;s <code>eth_signTypedData_v4</code>{" "}
        verbatim. The challenge is void after ten minutes.
      </p>

      <Endpoint method="POST" path="/v1/payer/deposit-requests/{id}/wallet/attest">
        Bind the wallet: <code>{`{ "wallet": "0x…", "signature": "0x…" }`}</code> with the session
        header. Answers the unlocked request with <code>address</code> and <code>payer_wallet</code>{" "}
        set.
      </Endpoint>
      <Params
        title="Body"
        rows={[
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
            description:
              "The 65-byte signature over the challenge's typed data. It must recover to wallet; externally owned accounts only for now.",
          },
        ]}
      />
      <Answers
        rows={[
          {
            status: 200,
            when: "Bound: salt, recovery term, and address written together, once. deposit_request.ready is raised.",
          },
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
          {
            status: 409,
            code: "wallet_already_bound",
            when: "Another wallet already won; the message names it.",
          },
          {
            status: 410,
            code: "deposit_request_not_payable",
            when: "The request is past its deadline or closed.",
          },
        ]}
      />

      <Callout title="Never in a URL">
        The session token goes in the <code>Payday-Payer-Session</code> header, and a client secret
        in a URL fragment. Neither should appear in a path, a query string, or a log.
      </Callout>
    </DocsPage>
  );
}
