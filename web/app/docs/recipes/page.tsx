import Link from "next/link";
import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { CodeTabs } from "@/components/docs/code-tabs";
import { Callout, DocsPage, H2, Step, Steps } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Recipes",
  description:
    "Complete integration patterns: crediting a signed-in user, invoicing with a PDF, verified-email requests, reconciliation, saved records, and key rotation.",
};

const CREDIT_CREATE = `// POST /deposits  — your route, behind your own authentication
import { PaydayClient, checkoutUrl } from "@payday/sdk";

const payday = new PaydayClient({ apiKey: process.env.PAYDAY_API_KEY! });

app.post("/deposits", requireUser, async (req, res) => {
  const { amount } = req.body; // "250.00", validated by you
  const deposit = await payday.depositRequests.create(
    {
      amount,
      issuer_id: process.env.PAYDAY_ISSUER_ID!, // saved identity: name, mailbox, payout wallet
      payer: { name: req.user.displayName },
      heading: "Account top-up",
      reference: \`topup-\${req.user.id}-\${Date.now()}\`,
      payer_policy: { mode: "merchant_session", payer_reference: req.user.id },
      expires_in: 3600,
    },
    \`topup-\${req.user.id}-\${req.body.nonce}\`, // idempotency key from the client's nonce
  );

  // client_secret is on this response only. Send the user straight to the page.
  res.redirect(303, checkoutUrl(deposit, deposit.client_secret!));
});`;

const CREDIT_WEBHOOK = `app.post("/payday/webhook", rawJson, async (req, res) => {
  if (!verify(req.body, req.header("Payday-Signature") ?? "", WEBHOOK_SECRET)) return res.status(400).end();
  const event = JSON.parse(req.body.toString("utf8"));
  if (await events.seen(event.id)) return res.status(200).end();

  if (event.type === "deposit_request.settled") {
    const { id, payer_reference, amount_base_units } = event.data.deposit_request;
    // Read it back: the API is the source of truth.
    const fresh = await payday.depositRequests.get(id);
    if (fresh.status === "settled") {
      await ledger.credit(payer_reference, BigInt(amount_base_units), { depositRequestId: id });
    }
  }
  await events.mark(event.id);
  res.status(200).end();
});`;

const CREDIT_RETURN = `// The user comes back to /deposits/:id later: mint a fresh secret and redirect again.
app.get("/deposits/:id", requireUser, async (req, res) => {
  const deposit = await payday.depositRequests.get(req.params.id);
  if (deposit.payer_policy.mode !== "merchant_session" || deposit.payer_policy.payer_reference !== req.user.id) {
    return res.status(404).end();
  }
  const { client_secret } = await payday.depositRequests.createClientSecret(deposit.id);
  res.redirect(303, checkoutUrl(deposit, client_secret));
});`;

const INVOICE_TS = `import { readFile } from "node:fs/promises";

// 1. Upload the PDF. This reserves a slot, PUTs the bytes, and waits for the scan.
const pdf = await payday.attachments.upload(await readFile("INV-1042.pdf"), "INV-1042.pdf");

// 2. Issue the request with the attachment and the payer's mailbox.
const request = await payday.depositRequests.create(
  {
    amount: "1250.00",
    issuer_id: acme.id,
    customer_id: globex.id,           // the saved customer supplies the payer party
    heading: "Consulting, March",
    reference: "INV-1042",
    notes: "Net 30. Thank you.",
    attachment_id: pdf.id,
    payer_policy: { mode: "verified_email", expected_email: "ap@globex.example" },
    expires_in: 30 * 24 * 3600,
  },
  "INV-1042",                         // reuse the invoice number: a retry never double-issues
);
// Payday emails the request to the customer's saved email, if it has one.`;

const INVOICE_CURL = `PDF_SLOT=$(curl -fsS "$API/v1/attachments" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" -H "Content-Type: application/json" \\
  -d '{ "filename": "INV-1042.pdf" }')
UPLOAD_URL=$(echo "$PDF_SLOT" | jq -r .upload_url)
ATT_ID=$(echo "$PDF_SLOT" | jq -r .id)

# PUT the bytes with exactly the returned headers.
curl -fsS -X PUT "$UPLOAD_URL" \\
  $(echo "$PDF_SLOT" | jq -r '.headers | to_entries[] | "-H \\"\\(.key): \\(.value)\\""' | xargs) \\
  --data-binary @INV-1042.pdf

# Finalize; retry on 409 attachment_scan_pending.
curl -fsS -X POST "$API/v1/attachments/$ATT_ID/finalize" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"

curl -fsS "$API/v1/deposit-requests" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" -H "Content-Type: application/json" \\
  -H "Idempotency-Key: INV-1042" \\
  -d "{
    \\"amount\\": \\"1250.00\\",
    \\"issuer_id\\": \\"iss_…\\", \\"customer_id\\": \\"cus_…\\",
    \\"heading\\": \\"Consulting, March\\", \\"reference\\": \\"INV-1042\\",
    \\"attachment_id\\": \\"$ATT_ID\\",
    \\"payer_policy\\": { \\"mode\\": \\"verified_email\\", \\"expected_email\\": \\"ap@globex.example\\" },
    \\"expires_in\\": 2592000
  }"`;

const RECONCILE = `// Everything still open, oldest first once you have paged through.
let cursor: string | undefined;
do {
  const page = await payday.depositRequests.list({ status: "partially_deposited", limit: 100, ...(cursor && { starting_after: cursor }) });
  for (const row of page.deposit_requests) {
    console.log(row.reference, row.received, "of", row.amount, "expires", row.expires_at);
  }
  cursor = page.next_cursor ?? undefined;
} while (cursor);

// One request, by your own reference.
const [byRef] = (await payday.depositRequests.list({ reference: "INV-1042" })).deposit_requests;

// Who paid, exactly: every finalized transfer to the address.
const { transfers } = await payday.depositRequests.transfers(byRef!.id);
for (const t of transfers) console.log(t.sender, t.amount, t.disposition, t.transaction_hash);`;

const SETUP = `// Once, at onboarding. Store the ids beside your API key.
const acme = await payday.issuers.create({ name: "Acme LLC", contact_email: "billing@acme.example" });
await payday.issuers.startEmailVerification(acme.id);          // a code goes to billing@acme.example
await payday.issuers.confirmEmailVerification(acme.id, "123456");

const treasury = await payday.payoutAddresses.create({ address: "0x1111…", label: "Treasury" });
await payday.issuers.setPayoutAddresses(acme.id, [treasury.id]);

const globex = await payday.customers.create({ name: "Globex", email: "ap@globex.example" });

// From then on, the smallest possible request:
await payday.depositRequests.create(
  { amount: "10.00", issuer_id: acme.id, customer_id: globex.id, payer_policy: { mode: "permissionless" } },
  crypto.randomUUID(),
);`;

const SCREEN = `// A screen that follows one request: return the moment it changes, or after 30 s.
let request = await payday.depositRequests.get(id);
while (!["settled", "returned"].includes(request.status)) {
  request = await payday.depositRequests.get(id, { waitForChange: true, timeout: 30 });
  render(request);
}`;

export default function RecipesPage() {
  return (
    <DocsPage
      eyebrow="Using Payday"
      title="Recipes"
      lead="Complete patterns, end to end. Each one is a few dozen lines against the API, and each is something a real integration has needed."
    >
      <H2 id="credit-a-signed-in-user">Credit a signed-in user</H2>
      <p>
        An exchange, a fund, a marketplace: your application already knows who the user is. Create
        the request naming them, hand them straight to the checkout, and credit their ledger from
        the webhook. The payer types nothing and verifies nothing; your sign-in is the verification.
      </p>
      <Steps>
        <Step title="Create and redirect">
          <CodeBlock code={CREDIT_CREATE} lang="ts" />
          <p>
            The secret rides in the URL fragment, which the browser never sends to a server. The
            checkout reads it, removes it from the address bar, and opens unlocked.
          </p>
        </Step>
        <Step title="Credit from the webhook">
          <CodeBlock code={CREDIT_WEBHOOK} lang="ts" />
          <p>
            <code>payer_reference</code> is the id you passed, so no lookup is needed. Credit on{" "}
            <code>settled</code>, the strongest state; use <code>deposited</code> for a provisional
            credit if your risk policy allows it.
          </p>
        </Step>
        <Step title="Reopen for a returning user">
          <CodeBlock code={CREDIT_RETURN} lang="ts" />
          <p>
            A secret is spent by its first exchange, so a user who navigates away and comes back
            gets a fresh one. Earlier unspent secrets stay valid until they expire, so a retried
            redirect never breaks a link already sent.
          </p>
        </Step>
      </Steps>

      <H2 id="invoice-with-a-pdf">Invoice a customer with a PDF</H2>
      <p>
        An itemised invoice, sent to a named accounts-payable mailbox, payable only by someone who
        can open that mailbox. The PDF is scanned, hashed into the address, and shown to the payer
        after they verify.
      </p>
      <CodeTabs
        tabs={[
          { label: "TypeScript", content: <CodeBlock code={INVOICE_TS} lang="ts" /> },
          { label: "curl", content: <CodeBlock code={INVOICE_CURL} lang="bash" /> },
        ]}
      />
      <Callout title="Use the invoice number as the idempotency key">
        Reusing the key with the same document returns the original request instead of a duplicate;
        reusing it with a different amount or attachment is refused. Your invoicing job can be
        retried freely.
      </Callout>

      <H2 id="set-up-once">Set up once, issue in one line</H2>
      <p>
        Save the party you issue under, prove its mailbox, save the wallets you settle to, and save
        your customers. After that a request is an amount, two ids, and a policy.
      </p>
      <CodeBlock code={SETUP} lang="ts" />
      <p>
        The request still stores its own snapshot of the issuer and the payer, so editing a saved
        record later never changes a request already issued. It also remembers{" "}
        <code>issuer_id</code> and <code>customer_id</code>, so the list route can filter by either.
      </p>

      <H2 id="reconcile">Reconcile</H2>
      <p>
        Save the <code>dr_</code> id and your own <code>reference</code> on every request you issue.
        Then the API answers every reconciliation question directly.
      </p>
      <CodeBlock code={RECONCILE} lang="ts" />
      <ul>
        <li>
          The list route filters on <code>status</code>, <code>reference</code>,{" "}
          <code>customer_id</code>, <code>issuer_id</code>, and <code>verification</code>, and pages
          with <code>starting_after</code>.
        </li>
        <li>
          A request can also be fetched by its deposit address, which is what appears in your
          wallet&apos;s incoming transaction.
        </li>
        <li>
          Once settled, the <Link href="/docs/proof-of-payment">Proof of Payment</Link> is the
          durable record: keep it with the invoice.
        </li>
      </ul>

      <H2 id="follow-a-request-live">Follow a request on a screen</H2>
      <p>
        For a page that is open right now, long-poll instead of hammering the read route. The
        request returns when <code>updated_at</code> changes or the timeout elapses.
      </p>
      <CodeBlock code={SCREEN} lang="ts" />

      <H2 id="rotate-a-key">Rotate an API key without downtime</H2>
      <Steps>
        <Step title="Roll the key in the dashboard">
          <p>
            The new key is shown once. The previous key keeps working for 24 hours, and the account
            reports <code>previous_key_expires_at</code> so you can see the window.
          </p>
        </Step>
        <Step title="Deploy the new key">
          <p>
            Update the secret in every service that calls Payday. Confirm with a read of the
            account.
          </p>
        </Step>
        <Step title="Let the old key lapse, or revoke">
          <p>
            Nothing else to do. If a key may have leaked, use <strong>Revoke</strong> instead, which
            invalidates the current key and any key in its grace window immediately, then generate a
            new one.
          </p>
        </Step>
      </Steps>

      <H2 id="test-in-the-sandbox">Test in the sandbox first</H2>
      <p>
        Point the client at <code>https://api.sandbox.payday.sh</code> with a{" "}
        <code>payday_test_</code> key and run the same code against the testnets and Circle&apos;s
        test USDC. The sandbox runs real indexing and finality rather than a fake &quot;mark
        paid&quot; button, so what you see there is what production does. See{" "}
        <Link href="/docs/environments">Environments</Link>.
      </p>
    </DocsPage>
  );
}
