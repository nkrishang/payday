import Link from "next/link";
import type { Metadata } from "next";
import { AddressDerivationDiagram, IndexerDiagram } from "@/components/docs/diagrams";
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
  title: "How Payday works",
  description:
    "The architecture and security properties of Payday, written for a customer doing due diligence: no custody, terms fixed in the address, finality-gated crediting, and how credentials are handled.",
};

export default function SecurityPage() {
  return (
    <DocsPage
      eyebrow="Trust"
      title="How Payday works"
      lead="What happens between a deposit request and a settled deposit, and the properties that hold along the way. Written for the person deciding whether to build on Payday, with as much detail as that decision needs."
    >
      <H2 id="in-one-paragraph">In one paragraph</H2>
      <p>
        Payday never holds funds. Each request gets a one-time smart-contract address whose
        settlement terms are fixed before it exists, derived from the issued document and the
        payer&apos;s own signature. A finality-gated indexer reads USDC transfer events from the
        chain into a durable ledger and updates each request from that ledger. When a request is
        funded or expires, a settlement transaction executes the contract, which can only move the
        funds under its own fixed terms: the amount to your wallet, and anything else to the
        payer&apos;s wallet. Every step is recorded, and a settled request yields a proof that can
        be checked without Payday.
      </p>

      <H2 id="no-custody">No custody</H2>
      <Compare>
        <CompareItem title="What Payday holds" badge={<Pill tone="green">nothing of yours</Pill>}>
          <ul>
            <li>Your deposit requests, customers, and identities</li>
            <li>A ledger of every observed transfer</li>
            <li>Hashes of keys and secrets, never the values</li>
            <li>An operator wallet that pays gas for settlement transactions</li>
          </ul>
        </CompareItem>
        <CompareItem
          title="What Payday never holds"
          badge={<Pill tone="yellow">by construction</Pill>}
        >
          <ul>
            <li>The requested amount, which moves address to your wallet</li>
            <li>Returns, which move address to the payer&apos;s wallet</li>
            <li>Your wallet&apos;s keys, or the payer&apos;s</li>
            <li>Plaintext API keys, webhook secrets, or verification codes</li>
          </ul>
        </CompareItem>
      </Compare>
      <p>
        The operator wallet submits settlement transactions and pays their gas. It cannot redirect
        funds: the deposit contract reads its destination, amount, deadline, and recovery address
        from the terms committed into its own address, and anyone may execute it with the same
        result. A compromised operator key could at worst pause settlement, not divert it.
      </p>

      <H2 id="terms-fixed-in-the-address">Terms fixed in the address</H2>
      <p>
        The one-time address is a CREATE3 address: it depends on a factory contract and a salt, and
        the contract that will live there is deployed only when it is time to settle. Payday derives
        the salt from two things: the hash of the issued document, canonicalized with RFC 8785 so
        that the same document always hashes the same way, and the EIP-712 digest of the
        payer&apos;s wallet attestation over that hash.
      </p>
      <Figure caption="The derivation. Because the payer's signature is an input, no address exists until a wallet has committed to the request, and because the terms are inputs, no one can deploy a contract at that address with different terms.">
        <AddressDerivationDiagram />
      </Figure>
      <ul>
        <li>
          <strong>Immutability.</strong> Changing the amount, the payout address, the deadline, or
          the recovery address would change the address. An issued request cannot be edited; it is
          cancelled and reissued.
        </li>
        <li>
          <strong>Counterfactual.</strong> USDC can arrive before any contract exists at the
          address. Deployment later routes whatever balance is there under the committed terms.
        </li>
        <li>
          <strong>Recovery is the payer.</strong> The recovery term is the wallet that signed, so an
          overpayment remainder, an expired balance, or a late transfer returns to the payer
          on-chain without anyone deciding where it should go.
        </li>
        <li>
          <strong>Settlement needs only the salt.</strong> The attribution material exists so the
          document and the wallet can be proven afterwards, not so that funds depend on it.
        </li>
      </ul>

      <H2 id="detection">Detection and crediting</H2>
      <p>
        Payday runs its own indexer against the chain. It does not download blocks or trust a
        third-party notification; it queries the USDC contract&apos;s <code>Transfer</code> event
        logs for the addresses it is expecting, in bounded block ranges, up to the chain&apos;s
        finalized boundary.
      </p>
      <Figure caption="From chain to settlement. The ledger is append-only and every observation is identified by chain, token, transaction, and log index, so a transfer can never be credited twice.">
        <IndexerDiagram />
      </Figure>
      <p>A transfer is credited only when all of the following hold:</p>
      <ol>
        <li>It was emitted by the exact USDC contract configured for the environment.</li>
        <li>Its recipient is a known deposit address.</li>
        <li>Its block is at or below the finalized boundary.</li>
        <li>Its block timestamp is no later than the request&apos;s deadline.</li>
        <li>It has not been recorded before.</li>
      </ol>
      <p>
        A transfer that fails the deadline test is recorded as <strong>late</strong> and queued for
        return. A transfer from a wallet other than the attested one is credited but flags the
        request <strong>likely unsolicited</strong>. There is no privileged way to mark a request
        paid: the ledger is the only input.
      </p>
      <Table>
        <thead>
          <tr>
            <th>Property</th>
            <th>Guarantee</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>Finality</td>
            <td>Nothing is credited from a block that can still be reorganised.</td>
          </tr>
          <tr>
            <td>Exact asset</td>
            <td>
              USDC is identified by chain id and contract address, never by symbol or name. Upgrades
              to the USDC proxy are monitored.
            </td>
          </tr>
          <tr>
            <td>Idempotency</td>
            <td>
              Every observation has a unique identity; retries and restarts cannot double-count.
            </td>
          </tr>
          <tr>
            <td>Chain time</td>
            <td>
              Deadlines are judged by block timestamp, not by any clock Payday or the payer holds.
            </td>
          </tr>
          <tr>
            <td>Halt on doubt</td>
            <td>
              If the indexer&apos;s stored position stops matching the chain, or a request cannot be
              settled safely, movement pauses and the request reports <code>needs_attention</code>{" "}
              rather than guessing.
            </td>
          </tr>
        </tbody>
      </Table>

      <H2 id="settlement">Settlement</H2>
      <p>
        A funded or expired request is claimed by a settlement worker, batched with others, and
        executed in one transaction through the factory. The transaction&apos;s finalized receipt,
        not its submission, determines the outcome recorded against each request. Execution at or
        before the deadline sends exactly the amount to your payout address and any remainder to the
        payer; execution after it sends the whole balance to the payer. Funds arriving after
        execution are forwarded to the payer by the same contract.
      </p>
      <p>
        Because the contract enforces this, a settlement transaction submitted by anyone, including
        a payer or an auditor holding the salt, has the same effect. Payday&apos;s worker exists so
        that no one has to.
      </p>

      <H2 id="verification">Verification</H2>
      <p>
        Payday separates who a payer is from which wallet paid, and is precise about which of the
        two it vouches for.
      </p>
      <Table>
        <thead>
          <tr>
            <th>Fact</th>
            <th>Established by</th>
            <th>Who can check it</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>The mailbox was opened</td>
            <td>
              A one-time code from an established identity provider, exchanged by Payday. The payer
              never names an address; it goes to the one you asserted.
            </td>
            <td>Payday attests it, signed into the proof.</td>
          </tr>
          <tr>
            <td>Your application released the secret</td>
            <td>A single-use client secret, stored hashed, spent on first exchange.</td>
            <td>Payday attests it; the reference is your assertion.</td>
          </tr>
          <tr>
            <td>This wallet committed to this request</td>
            <td>
              An EIP-712 signature over the document hash and a nonce issued only after the policy
              passed.
            </td>
            <td>Anyone, from the proof.</td>
          </tr>
          <tr>
            <td>This wallet paid</td>
            <td>Transfer events from that wallet to the address.</td>
            <td>Anyone, against the chain.</td>
          </tr>
        </tbody>
      </Table>
      <p>
        The result is the <Link href="/docs/proof-of-payment">Proof of Payment</Link>: the
        recomputable parts are recomputable, the attested parts are signed and bound to that request
        and that payer, and Payday declines to issue a proof it cannot stand behind.
      </p>

      <H2 id="credentials">Credentials and secrets</H2>
      <H3 id="api-keys">API keys</H3>
      <ul>
        <li>Shown once. Stored as a SHA-256 digest with a six-character hint.</li>
        <li>One active key per account; rotation keeps the previous key for 24 hours.</li>
        <li>
          Minting and revoking need a signed-in merchant, never a key. Whoever holds a key cannot
          widen their own access.
        </li>
        <li>Headers and bodies are never logged, which keeps keys out of logs.</li>
      </ul>
      <H3 id="merchant-sign-in">Merchant sign-in</H3>
      <p>
        Merchants sign in through an established wallet-and-identity provider with an emailed code.
        The provider creates the account&apos;s embedded wallet and holds its key material; Payday
        verifies the provider&apos;s signed identity token on every request with the provider&apos;s
        published keys, and refuses to start if it cannot fetch them. Payday never sees a password
        and never holds a merchant&apos;s wallet key.
      </p>
      <H3 id="payer-sessions">Payer sessions and codes</H3>
      <ul>
        <li>
          Verification codes are created and checked by the identity provider; Payday sees the code
          only to pass it on, and stores neither it nor the mailbox beyond the proof it needs.
        </li>
        <li>Session tokens are opaque, 24-hour, stored hashed, and bound to one request.</li>
        <li>
          Client secrets are returned once, stored hashed, spent on first use, and carried in URL
          fragments so they never reach a server log.
        </li>
        <li>One code per request per minute limits guessing; a wrong code reveals nothing.</li>
      </ul>
      <H3 id="webhook-secrets">Webhook secrets</H3>
      <p>
        Returned once at registration. Stored as a hash for identification and as an authenticated
        ciphertext, with a versioned key identifier, for delivery. Endpoint URLs are checked against
        private and reserved networks at registration and again before every delivery.
      </p>
      <H3 id="attachments">Attachments</H3>
      <p>
        PDFs go straight to object storage through a presigned, write-once upload. They are scanned
        for malware before they can be attached, hashed by Payday from the stored bytes, and pinned
        to the exact object version admitted. A rejected file is deleted.
      </p>

      <H2 id="the-web-application">The hosted checkout and dashboard</H2>
      <ul>
        <li>
          A deposit link is public by design and carries no merchant data. For a gated request,
          withheld fields are absent from the response, so the page cannot render them by accident.
        </li>
        <li>
          Pages are served with a strict Content-Security-Policy, cannot be framed, and send no
          Referer. The checkout&apos;s pay button sends a plain USDC transfer of the amount still
          due: no approval, no contract call, nothing that could redirect funds.
        </li>
        <li>
          The dashboard holds no API key. It authenticates with the merchant&apos;s session and
          renders nothing on the server, so no request or token is ever in a page&apos;s HTML.
        </li>
      </ul>

      <H2 id="operations">Operations</H2>
      <ul>
        <li>
          Production and sandbox are separate deployments with separate databases, keys, signers,
          and identity providers.
        </li>
        <li>
          The API answers <code>/health</code> for readiness and <code>/v1/status</code> for chain
          and indexer position, and every response carries a request id.
        </li>
        <li>
          Structured logs record method, path, status, latency, and account id. Never headers or
          bodies.
        </li>
        <li>
          The core library that derives addresses and verifies proofs is open source, so the checks
          a proof relies on can be read and run by anyone.
        </li>
      </ul>

      <Callout title="Questions for a security review">
        Write to <a href="mailto:support@payday.sh">support@payday.sh</a>. We are glad to walk
        through any of the above in more depth, share the contract sources, or run a live settlement
        with you on the sandbox.
      </Callout>
    </DocsPage>
  );
}
