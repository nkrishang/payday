import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { Sequence } from "@/components/docs/diagrams";
import { DisclosureTable, NeverShown } from "@/components/docs/figures";
import {
  Callout,
  Compare,
  CompareItem,
  DocsPage,
  Figure,
  H2,
  H3,
  Pill,
  Table,
} from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Verifying the payer",
  description:
    "The three payer policies, what each one proves, and the wallet attestation every deposit request takes before it has an address.",
};

const POLICIES = `{ "mode": "permissionless" }
{ "mode": "verified_email", "expected_email": "alice@customer.example" }
{ "mode": "merchant_session", "payer_reference": "user_123" }`;

export default function PayerVerificationPage() {
  return (
    <DocsPage
      eyebrow="Getting started"
      title="Verifying the payer"
      lead="Every deposit request names who may pay and what they must prove first. You assert; Payday confirms; and whatever the policy, the payer signs once from the wallet they will pay from before an address exists."
    >
      <p>
        Verification answers a question a bank transfer never could: was the money sent by the
        person you were expecting, and did they see what they were paying for? Payday splits that
        into two parts. The <strong>payer policy</strong> decides what a person proves before the
        request&apos;s content is shown to them. The <strong>wallet step</strong> then ties that
        person to the wallet the funds will come from.
      </p>

      <H2 id="the-three-policies">The three policies</H2>
      <CodeBlock code={POLICIES} lang="json" title="payer_policy" />

      <Compare>
        <CompareItem title="Permissionless" badge={<Pill>open link</Pill>}>
          <p>
            Anyone holding the link sees the request and can pay it. Nothing is proved beyond the
            wallet step.
          </p>
          <p>
            <strong>Use it</strong> for a one-off invoice you email yourself, or when the link is
            handed over in a channel you already trust.
          </p>
        </CompareItem>
        <CompareItem title="Verified email" badge={<Pill tone="green">emailed code</Pill>}>
          <p>
            You name the mailbox. The checkout shows only your name and the heading until the payer
            types a code sent to that mailbox. Payday sends the code; the payer never types an
            address.
          </p>
          <p>
            <strong>Use it</strong> when a request must only be seen and paid by a specific person
            you know by email.
          </p>
        </CompareItem>
        <CompareItem title="Merchant session" badge={<Pill tone="yellow">your sign-in</Pill>}>
          <p>
            Your application has already signed the user in. It creates the request naming that user
            by your own id, receives a single-use client secret, and opens the checkout with it. The
            page opens unlocked; the payer types nothing.
          </p>
          <p>
            <strong>Use it</strong> for an exchange, a fund, or any product crediting a logged-in
            customer.
          </p>
        </CompareItem>
      </Compare>

      <p>
        The merchant asserts, Payday confirms. Payday tells you whether the check passed and when,
        never the payer&apos;s own data. The full policy, including the expected email or the payer
        reference, is returned only to you; the payer page shows a masked hint such as{" "}
        <code>a****@c***.example</code> for verified email, and nothing at all for a merchant
        session.
      </p>

      <H2 id="what-the-payer-sees">What the payer sees, and when</H2>
      <p>
        A gated request discloses progressively. Until the policy is satisfied, only the issuer name
        and heading are shown; until the wallet is bound, there is no address to show.
      </p>
      <Figure caption="Disclosure by moment. A permissionless request starts in the middle column: everything but the address is visible at once.">
        <DisclosureTable />
      </Figure>
      <Figure caption="Never shown to a payer, whatever the policy.">
        <NeverShown />
      </Figure>

      <H2 id="verified-email">Verified email, step by step</H2>
      <Figure caption="The emailed-code exchange. The payer's browser holds a session token, which travels in a header on every later read and unlocks exactly this request.">
        <Sequence
          label="Verified email sequence: the payer opens the link, Payday sends a code to the asserted mailbox, the payer confirms it, the session unlocks the content, the payer signs the wallet attestation and the address appears."
          lanes={["Payer's browser", "Payday", "Asserted mailbox"]}
          messages={[
            { from: 0, to: 1, label: "open deposit_url", note: "issuer name and heading only" },
            { from: 0, to: 1, label: "start email verification" },
            { from: 1, to: 2, label: "six-digit code", accent: "green" },
            {
              from: 1,
              to: 0,
              label: "payer_session",
              reply: true,
              note: "opaque, 24h, stored hashed",
            },
            { from: 0, to: 1, label: "confirm code" },
            { from: 1, to: 0, label: "content unlocked", reply: true, accent: "green" },
            { from: 0, to: 1, label: "wallet challenge → sign → attest" },
            { from: 1, to: 0, label: "one-time address", reply: true, accent: "yellow" },
          ]}
        />
      </Figure>
      <ul>
        <li>
          One code per request per minute, whoever asks. A wrong, spent, or expired code fails.
        </li>
        <li>
          The code is delivered by Payday&apos;s email provider from a Payday domain and names no
          deposit data.
        </li>
        <li>
          Funds sent before verification completes still count, but the request is flagged likely
          unsolicited, because the attested wallet did not pay them.
        </li>
        <li>
          After settlement, the same exchange re-proves the mailbox to reopen the receipt; it never
          makes the request payable again.
        </li>
      </ul>

      <H2 id="merchant-session">Merchant session, step by step</H2>
      <p>
        This is the mode for applications with their own sign-in. Your server creates the request,
        receives a <code>client_secret</code> once, and sends the signed-in user to the checkout
        with that secret in the URL fragment. Exchanging the secret is the verification.
      </p>
      <Figure caption="The merchant-session exchange. The secret rides in the URL fragment, which browsers never send to a server, so it reaches no log, Referer header, or analytics beacon.">
        <Sequence
          label="Merchant session sequence: your server creates the request and receives a client secret, redirects the signed-in user to the deposit URL with the secret in the fragment, the checkout exchanges it for a session, the payer signs and pays, and webhooks carrying the payer reference reach your server."
          lanes={["Your server", "Payer's browser", "Payday"]}
          messages={[
            { from: 0, to: 2, label: "create { mode: merchant_session, payer_reference }" },
            {
              from: 2,
              to: 0,
              label: "201 + client_secret (once, 15 min)",
              reply: true,
              accent: "green",
            },
            { from: 0, to: 1, label: "redirect to deposit_url#cs=…" },
            { from: 1, to: 2, label: "exchange client_secret", note: "spent on first use" },
            { from: 2, to: 1, label: "payer_session, page unlocked", reply: true, accent: "green" },
            { from: 1, to: 2, label: "wallet challenge → sign → attest" },
            { from: 1, to: 2, label: "USDC transfer to the address", accent: "yellow" },
            {
              from: 2,
              to: 0,
              label: "webhooks with payer_reference",
              reply: true,
              accent: "green",
            },
          ]}
        />
      </Figure>
      <ul>
        <li>
          The secret is returned exactly once, on the <code>201</code>. It is never on a replay or a
          later read; Payday stores only its hash. If your server loses it, mint another with{" "}
          <code>POST /v1/deposit-requests/&#123;id&#125;/client-secret</code>.
        </li>
        <li>
          A secret is spent by its first exchange. The same link pasted into a second window is
          refused, and the page says so. When the user comes back later, mint a fresh secret and
          redirect again.
        </li>
        <li>
          Every webhook for the request carries your <code>payer_reference</code>, so a handler
          credits the right ledger with no lookup.
        </li>
        <li>
          What Payday attests is narrow and stated plainly: your server released this secret, and it
          was exchanged before this session saw the request. Who the payer is remains your
          assertion.
        </li>
      </ul>
      <Callout title="Created through the API only">
        The dashboard shows merchant-session requests, with your payer reference and an &quot;Opened
        by your app&quot; entry in the verification activity, but cannot compose one: it has no
        signed-in user to hand the secret to.
      </Callout>

      <H2 id="the-wallet-step">The wallet step</H2>
      <p>
        Whichever policy a request has, the payer signs one message before the address exists. It is
        an <a href="https://eips.ethereum.org/EIPS/eip-712">EIP-712</a> typed-data signature, the
        kind every wallet displays in full before signing: a plain statement, the request&apos;s
        attribution hash, the wallet address, a one-time nonce, and an expiry. No transaction, no
        gas, no approval.
      </p>
      <Table>
        <thead>
          <tr>
            <th>Field</th>
            <th>What it is</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>statement</td>
            <td>A sentence the wallet shows: what the payer is agreeing to.</td>
          </tr>
          <tr>
            <td>attributionHash</td>
            <td>The hash of the issued document, so the signature belongs to this request only.</td>
          </tr>
          <tr>
            <td>wallet</td>
            <td>The address that will pay. The signature must recover to it.</td>
          </tr>
          <tr>
            <td>nonce</td>
            <td>
              Minted for this session once the policy is satisfied, valid for ten minutes. It is
              what ties the person who passed the checks to the wallet.
            </td>
          </tr>
          <tr>
            <td>expiresAt</td>
            <td>When the challenge is void.</td>
          </tr>
        </tbody>
      </Table>
      <p>
        Once accepted, the salt, the recovery term (the wallet), and the address are written
        together, once. A <code>deposit_request.ready</code> webhook reports it, and the checkout
        shows the address, the QR, and the pay button. The pay button refuses to send from any
        wallet but the attested one.
      </p>
      <Callout title="Externally owned accounts for now">
        The signature must recover directly to the wallet address, which is how every ordinary
        wallet signs. Smart-contract wallets are not supported yet.
      </Callout>

      <H3 id="reading-verification-state">Reading verification state</H3>
      <p>Every request reports verification separately from its status:</p>
      <ul>
        <li>
          <code>verification_completed_at</code>: when the policy was satisfied; <code>null</code>{" "}
          until then, and always <code>null</code> for permissionless requests;
        </li>
        <li>
          <code>likely_unsolicited_at</code>: when finalized funds first arrived from a wallet other
          than the attested one;
        </li>
        <li>
          <code>payer_wallet</code> and <code>wallet_bound_at</code>: the attested wallet and when
          it was bound;
        </li>
        <li>
          <code>GET /v1/deposit-requests/&#123;id&#125;/verification</code>: each fact on its own
          (email, merchant session, wallet) and every attempt the payer made, with its outcome and
          time. Never the code, the secret, or the session.
        </li>
      </ul>
      <p>
        The list route filters on{" "}
        <code>verification=not_required|pending|verified|likely_unsolicited</code> alongside status,
        and webhooks raise <code>verification.approved</code>, <code>deposit_request.ready</code>,
        and <code>deposit_request.likely_unsolicited</code> the moment each fact is first recorded.
      </p>
    </DocsPage>
  );
}
