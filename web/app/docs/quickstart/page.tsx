import Link from "next/link";
import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { CodeTabs } from "@/components/docs/code-tabs";
import { Callout, DocsPage, H2, Step, Steps } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Quickstart",
  description: "Mint an API key, issue a deposit request, share the link, and watch it settle.",
};

const CREATE_CURL = `export API=https://api.payday.sh
export PAYDAY_API_KEY=payday_live_...

curl -fsS "$API/v1/deposit-requests" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  -H "Content-Type: application/json" \\
  -H "Idempotency-Key: order-1042" \\
  -d '{
    "amount": "25.00",
    "payout_address": "0x1111111111111111111111111111111111111111",
    "issuer": { "name": "Acme LLC" },
    "payer": { "name": "Customer Inc" },
    "heading": "March retainer",
    "reference": "INV-1042",
    "payer_policy": { "mode": "permissionless" },
    "expires_in": 3600
  }' | jq`;

const CREATE_TS = `import { PaydayClient } from "@payday/sdk";

const payday = new PaydayClient({ apiKey: process.env.PAYDAY_API_KEY! });

const request = await payday.depositRequests.create(
  {
    amount: "25.00",
    payout_address: "0x1111111111111111111111111111111111111111",
    issuer: { name: "Acme LLC" },
    payer: { name: "Customer Inc" },
    heading: "March retainer",
    reference: "INV-1042",
    payer_policy: { mode: "permissionless" },
    expires_in: 3600,
  },
  "order-1042", // idempotency key: your own id for this request
);

console.log(request.id, request.deposit_url);`;

const CREATE_RESPONSE = `{
  "id": "dr_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "deposit_url": "https://payday.sh/pay/dr_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "status": "awaiting_deposit",
  "amount": "25.000000",
  "amount_base_units": "25000000",
  "received": "0.000000",
  "remaining": "25.000000",
  "address": null,
  "payer_wallet": null,
  "payout_address": "0x1111111111111111111111111111111111111111",
  "networks": [
    { "chain": { "id": "143", "name": "Monad" }, "token": { "symbol": "USDC", "address": "0x754704Bc059F8C67012fEd69BC8A327a5aafb603", "decimals": 6 } },
    { "chain": { "id": "8453", "name": "Base" }, "token": { "symbol": "USDC", "address": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", "decimals": 6 } },
    { "chain": { "id": "42161", "name": "Arbitrum One" }, "token": { "symbol": "USDC", "address": "0xaf88d065e77c8cC2239327C5EDb3A432268e5831", "decimals": 6 } }
  ],
  "chain": null,
  "token": null,
  "expires_at": "2026-09-06T13:00:00Z",
  "created_at": "2026-09-06T12:00:00Z",
  "...": "…"
}`;

const POLL_CURL = `curl -fsS "$API/v1/deposit-requests/dr_0198f80c-…?wait_for=change&timeout=30" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  | jq '{status, address, received, settlement_tx_hash}'`;

const POLL_TS = `// Returns when the request changes, or after 30 seconds.
const latest = await payday.depositRequests.get(request.id, { waitForChange: true });
console.log(latest.status, latest.address, latest.received);`;

const PROOF_CURL = `curl -fsS "$API/v1/deposit-requests/dr_0198f80c-…/proof" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" > proof.json`;

const PROOF_TS = `const proof = await payday.depositRequests.proof(request.id);
await fs.writeFile("proof.json", JSON.stringify(proof, null, 2));`;

export default function QuickstartPage() {
  return (
    <DocsPage
      eyebrow="Getting started"
      title="Quickstart"
      lead="Issue a USDC deposit request, share its link, and watch finalized funds settle to your wallet. Five steps, one API key, no smart-contract code."
    >
      <Callout title="Prefer not to write code?">
        Everything on this page is also in the <Link href="/docs/dashboard">dashboard</Link>, which
        signs in with an emailed code and needs no API key. Start there if you want to see a request
        settle before integrating.
      </Callout>

      <Steps>
        <Step title="Sign in and mint an API key">
          <p>
            Open <a href="https://payday.sh">payday.sh</a>, choose <strong>Start Building</strong>,
            and enter the code emailed to you. The account is created the first time a code is
            accepted, with its own wallet where deposits settle by default.
          </p>
          <p>
            In the dashboard&apos;s <strong>API key</strong> section, generate a key. It is shown
            exactly once. Store it as <code>PAYDAY_API_KEY</code> in your server&apos;s secret
            store, never in a browser or a repository. A key has full authority over the account;
            see <Link href="/docs/api#authentication">Authentication</Link> for rotation and
            revocation.
          </p>
          <p>
            To try against test USDC first, use the sandbox at{" "}
            <code>https://api.sandbox.payday.sh</code> with a <code>payday_test_</code> key. See{" "}
            <Link href="/docs/environments">Environments</Link>.
          </p>
        </Step>

        <Step title="Issue a deposit request">
          <p>
            One call. The amount is used exactly as given; there are no line items. The{" "}
            <code>Idempotency-Key</code> header is required, and a retry with the same key and body
            returns the original request instead of creating a second one.
          </p>
          <CodeTabs
            tabs={[
              { label: "curl", content: <CodeBlock code={CREATE_CURL} lang="bash" /> },
              { label: "TypeScript", content: <CodeBlock code={CREATE_TS} lang="ts" /> },
            ]}
          />
          <p>The response is the full deposit request. The fields that matter first:</p>
          <CodeBlock code={CREATE_RESPONSE} lang="json" title="201 Created" />
          <ul>
            <li>
              <code>deposit_url</code> is what you give the payer.
            </li>
            <li>
              <code>address</code> is <code>null</code>. The one-time address exists only after the
              payer signs from the wallet they will pay from. Never quote an address before then; a{" "}
              <code>deposit_request.ready</code> webhook reports the moment it exists.
            </li>
            <li>
              Expiry defaults to 24 hours. Pass <code>expires_in</code> in seconds or an RFC 3339{" "}
              <code>expires_at</code>; the window is 10 minutes to 366 days.
            </li>
          </ul>
        </Step>

        <Step title="Share the link">
          <p>
            Send <code>deposit_url</code> to the payer. If the request carried a{" "}
            <code>payer.email</code>, Payday has already emailed it to them. The hosted checkout
            shows the request, the networks it may be paid on, the amount still due, the exact
            token and one-time address and QR once the payer has chosen a network and signed, a
            deadline countdown, and live finalized status. It can also send the transfer from a connected wallet in the page.
          </p>
          <p>
            The link is open by design: anyone holding it can read the request and pay it. It
            carries no merchant data, so nothing about your payout wallet, metadata, or policy
            assertions is exposed.
          </p>
        </Step>

        <Step title="Track settlement">
          <p>
            Read the request whenever you like, or long-poll so a screen updates the moment
            something changes. For automation, register a <Link href="/docs/webhooks">webhook</Link>{" "}
            instead and let Payday tell you.
          </p>
          <CodeTabs
            tabs={[
              { label: "curl", content: <CodeBlock code={POLL_CURL} lang="bash" /> },
              { label: "TypeScript", content: <CodeBlock code={POLL_TS} lang="ts" /> },
            ]}
          />
          <p>
            A normal deposit moves <code>awaiting_deposit</code> → <code>deposited</code> →{" "}
            <code>settled</code>, with <code>partially_deposited</code> in between when transfers
            arrive in pieces. Payday credits only finalized transfers, so a payer&apos;s wallet may
            show a confirmed transaction a little before <code>received</code> changes.
          </p>
        </Step>

        <Step title="Download the Proof of Payment">
          <p>
            Once settled, every request yields a JSON proof tying the exact document to the
            payer&apos;s wallet, the one-time address, the transfers that paid it, and the
            settlement transaction. It verifies offline without trusting Payday. Keep it with your
            records.
          </p>
          <CodeTabs
            tabs={[
              { label: "curl", content: <CodeBlock code={PROOF_CURL} lang="bash" /> },
              { label: "TypeScript", content: <CodeBlock code={PROOF_TS} lang="ts" /> },
            ]}
          />
        </Step>
      </Steps>

      <H2 id="next">Next</H2>
      <ul>
        <li>
          <Link href="/docs/payer-verification">Verify the payer</Link> with an emailed code, or
          open the checkout from your own application for a user it has signed in.
        </li>
        <li>
          <Link href="/docs/webhooks">Register a webhook</Link> so your system credits the deposit
          without polling.
        </li>
        <li>
          <Link href="/docs/api/attachments">Attach a PDF</Link> for an itemised breakdown; its hash
          is committed into the address.
        </li>
        <li>
          <Link href="/docs/recipes">Recipes</Link> walk through complete integrations.
        </li>
      </ul>
    </DocsPage>
  );
}
