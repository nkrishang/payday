import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { AddressDerivationDiagram } from "@/components/docs/diagrams";
import { Callout, DocsPage, Figure, H2, Step, Steps, Table } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Proof of Payment",
  description:
    "Every settled deposit request yields a JSON proof tying the document, the payer's wallet, the one-time address, and the transfers together. Anyone can verify it offline.",
};

const PROOF = `{
  "version": "payday.proof.v2",
  "payment_id": "dr_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "canonical_issuance_snapshot": {
    "schema": "payday.invoice",
    "canonicalization": "RFC8785",
    "issuer": { "name": "Acme LLC", "email": "billing@acme.example" },
    "bill_to": { "name": "Customer Inc" },
    "amount_base_units": "25000000",
    "heading": "March retainer",
    "reference": "INV-1042",
    "notes": null,
    "expiration_timestamp": "1757160000",
    "payer_policy": { "mode": "verified_email", "expected_email": "alice@customer.example" },
    "attachment": { "id": "0198f80c-…", "byte_length": "48211", "sha256": "0x9f…" },
    "chain_id": "143",
    "token_address": "0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
    "receiver_address": "0x1111111111111111111111111111111111111111",
    "factory_address": "0x…"
  },
  "canonicalization": "RFC8785",
  "attribution_hash": "0x…",
  "payer_wallet": {
    "address": "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
    "typed_data": { "domain": { "name": "Payday", "version": "1", "…": "…" }, "…": "…" },
    "digest": "0x…",
    "signature": "0x…",
    "method": "ecdsa"
  },
  "salt": "0x…",
  "chain_id": "143",
  "factory_address": "0x…",
  "payment_address": "0x2222222222222222222222222222222222222222",
  "token_address": "0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
  "recovery_address": "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
  "settlement_transaction_hash": "0x…",
  "transfers": [
    {
      "transaction_hash": "0x…",
      "log_index": "12",
      "sender": "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
      "recipient": "0x2222222222222222222222222222222222222222",
      "amount_base_units": "25000000",
      "block_number": "98765432"
    }
  ],
  "verification": {
    "payload": {
      "version": "1",
      "payment_id": "dr_0198f80c-…",
      "attribution_hash": "0x…",
      "chain_id": "143",
      "payment_address": "0x2222…",
      "payer_wallet": "0x5aAe…",
      "wallet_nonce": "0x…",
      "payer_policy_mode": "verified_email",
      "result": "approved",
      "verified_at": "2026-09-01T11:58:00Z",
      "wallet_bound_at": "2026-09-01T11:58:30Z",
      "facts": [
        { "kind": "mailbox", "provider": "auth0", "at": "2026-09-01T11:58:00Z" },
        { "kind": "wallet", "provider": "payday", "at": "2026-09-01T11:58:30Z" }
      ]
    },
    "signer": "0x…",
    "signature": "0x…"
  }
}`;

export default function ProofPage() {
  return (
    <DocsPage
      eyebrow="Using Payday"
      title="Proof of Payment"
      lead="A PDF receipt proves nothing on its own. A Proof of Payment lets anyone, with no access to Payday, recompute that this exact document, signed by this wallet, could only have been paid at this address, and that this wallet paid it."
    >
      <p>
        Once a deposit request settles, <code>GET /v1/deposit-requests/&#123;id&#125;/proof</code>{" "}
        returns a JSON document. It is available to you and shared at your discretion with a payer,
        an auditor, or a counterparty. It is not a public link.
      </p>

      <H2 id="what-it-proves">What it proves</H2>
      <p>The sentence a proof supports is precise:</p>
      <Callout title="The claim">
        A session that satisfied this request&apos;s policy proved control of wallet W. Every
        credited transfer came from W to an address that can belong only to this document and this
        attestation. Exactly the requested amount reached the merchant&apos;s payout address in the
        named transaction.
      </Callout>
      <p>
        Most of that is recomputable by anyone from the proof alone. What remains Payday&apos;s word
        is the identity facts: that the mailbox code was exchanged, or that your server&apos;s
        secret was redeemed, and that the wallet&apos;s nonce was issued only after the policy
        passed. Those facts are listed in the proof and signed by Payday together with the document
        hash, the chain, the address, the wallet, and the nonce, so the statement belongs to this
        request and this payer alone and cannot be transplanted onto another.
      </p>

      <Figure caption="What a verifier recomputes: the hash from the document, the signature's validity and digest, the salt from both, and the address from the salt and the terms. Then it checks the transfers on-chain.">
        <AddressDerivationDiagram />
      </Figure>

      <H2 id="whats-inside">What is inside</H2>
      <Table>
        <thead>
          <tr>
            <th>Field</th>
            <th>Meaning</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>canonical_issuance_snapshot</td>
            <td>
              The exact document that was hashed at issuance, in canonical form: parties, amount in
              base units, heading, reference, notes, deadline, policy, the attachment&apos;s length
              and SHA-256, chain, token, your payout address, and the factory.
            </td>
          </tr>
          <tr>
            <td>attribution_hash</td>
            <td>The hash of that document, after RFC 8785 canonicalization.</td>
          </tr>
          <tr>
            <td>payer_wallet</td>
            <td>
              The wallet address, the exact EIP-712 typed data it signed, the digest, and the
              signature. A verifier recovers the signer and checks it is the wallet.
            </td>
          </tr>
          <tr>
            <td>salt</td>
            <td>Derived from the attribution hash and the attestation digest.</td>
          </tr>
          <tr>
            <td>payment_address</td>
            <td>The one-time address, recomputable from the factory, the salt, and the terms.</td>
          </tr>
          <tr>
            <td>recovery_address</td>
            <td>Always the attested wallet.</td>
          </tr>
          <tr>
            <td>transfers</td>
            <td>
              Every credited USDC transfer into the address: transaction, log index, sender, amount,
              block. All must come from the attested wallet and sum to at least the amount.
            </td>
          </tr>
          <tr>
            <td>settlement_transaction_hash</td>
            <td>
              The transaction that executed the contract and moved the amount to you. The same hash
              the request reports as <code>settlement_tx_hash</code>.
            </td>
          </tr>
          <tr>
            <td>verification</td>
            <td>
              Payday&apos;s signed statement of the verification facts, bound to this request,
              address, wallet, and nonce.
            </td>
          </tr>
        </tbody>
      </Table>

      <details>
        <summary>A complete proof, abbreviated</summary>
        <CodeBlock code={PROOF} lang="json" />
      </details>

      <H2 id="verifying">Verifying one</H2>
      <p>
        The checks are published as <code>verify_proof</code> in Payday&apos;s open-source core
        library, which is the reference. In words:
      </p>
      <Steps>
        <Step title="Recompute the hash">
          <p>
            Canonicalize <code>canonical_issuance_snapshot</code> with RFC 8785 and hash it. It must
            equal <code>attribution_hash</code>. If a PDF is attached, hash the file you were given
            and compare it with the snapshot&apos;s <code>attachment.sha256</code>.
          </p>
        </Step>
        <Step title="Check the wallet signature">
          <p>
            Confirm the typed data names this attribution hash and this wallet, compute its EIP-712
            digest, and recover the signer from the signature. It must be{" "}
            <code>payer_wallet.address</code>.
          </p>
        </Step>
        <Step title="Recompute the salt and the address">
          <p>
            Derive the salt from the hash and the digest, then the address from the factory, the
            salt, and the terms in the snapshot with the wallet as the recovery term. It must equal{" "}
            <code>payment_address</code>.
          </p>
        </Step>
        <Step title="Check the transfers">
          <p>
            Every listed transfer must be from the attested wallet to the address, and their sum
            must be at least the amount. Whether they and the settlement transaction really executed
            is provable only against the chain: look them up by hash on any node or explorer.
          </p>
        </Step>
        <Step title="Check Payday's attestation">
          <p>
            Verify the signature over the <code>verification.payload</code> against Payday&apos;s
            published signer, and that the payload&apos;s hash, chain, address, wallet, and nonce
            match the rest of the proof.
          </p>
        </Step>
      </Steps>

      <H2 id="when-there-is-no-proof">When there is no proof</H2>
      <ul>
        <li>
          Before settlement: <code>409 deposit_request_not_settled</code>.
        </li>
        <li>
          When any credited transfer came from a wallet other than the attested one:{" "}
          <code>409 deposit_sender_mismatch</code>. The funds still settled, but no proof can claim
          the attested wallet paid them, so Payday does not issue one it cannot stand behind. The
          request&apos;s <code>likely_unsolicited_at</code> and its transfers say what happened.
        </li>
      </ul>

      <Callout title="Also available">
        <code>GET /v1/deposit-requests/&#123;id&#125;/request.pdf</code> renders Payday&apos;s own
        PDF summary of any request, settled or not. It is deterministic: the same request always
        renders byte-identical output, so two copies can be compared by hash.
      </Callout>
    </DocsPage>
  );
}
