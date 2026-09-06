import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { Callout, DocsPage, H2, Table } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "TypeScript SDK",
  description:
    "A zero-dependency, typed client for the Payday API: the merchant client for your server and the payer client for a checkout of your own.",
};

const INSTALL = `npm install @payday/sdk`;

const BASIC = `import { PaydayClient } from "@payday/sdk";

const payday = new PaydayClient({
  apiKey: process.env.PAYDAY_API_KEY!,
  // baseUrl: "https://api.sandbox.payday.sh",   // the sandbox, or a local gateway
});

const request = await payday.depositRequests.create(
  {
    amount: "10.00",
    payout_address: "0x1111111111111111111111111111111111111111",
    issuer: { name: "Acme LLC", email: "billing@acme.example" },
    payer: { name: "Customer Inc", details: "12 Main St, Springfield" },
    heading: "March retainer",
    reference: "INV-1042",
    payer_policy: { mode: "permissionless" },
    expires_in: 3600,
  },
  crypto.randomUUID(), // the idempotency key is required
);

await payday.depositRequests.get(request.id);
await payday.depositRequests.get(request.id, { waitForChange: true, timeout: 30 });
await payday.depositRequests.list({ status: "awaiting_deposit", limit: 20 });`;

const ERRORS = `import { PaydayError } from "@payday/sdk";

try {
  await payday.depositRequests.get("dr_does-not-exist");
} catch (error) {
  if (error instanceof PaydayError) {
    error.code;      // "deposit_request_not_found"
    error.status;    // 404
    error.message;   // names the problem, and the field when a body does not fit
    error.requestId; // quote this to support
  }
}`;

const PAYER = `import { PaydayPayerClient } from "@payday/sdk";

// No key. Safe in a browser: a deposit link is open by design.
const payer = new PaydayPayerClient();

const request = await payer.depositRequests.get("dr_0198f80c-…");
const qr = await payer.depositRequests.qr(request.id, payerSession);          // SVG blob
const pdf = await payer.depositRequests.attachment(request.id, payerSession); // descriptor + download_url

// Email verification, wallet attestation, and merchant-session exchange:
await payer.verification.startEmail(request.id);
await payer.verification.confirmEmail(request.id, "123456", payerSession);
await payer.verification.exchangeClientSecret(request.id, clientSecret);
await payer.wallet.challenge(request.id, walletAddress, { payerSession });
await payer.wallet.attest(request.id, walletAddress, signature, payerSession);`;

const SESSION = `// The dashboard's own way in: the signed-in merchant's session token instead of a key.
const payday = new PaydayClient({ accessToken: identityToken });
await payday.account.get();
await payday.account.issueApiKey(account.generation); // session only; a key is refused here`;

export default function SdkPage() {
  return (
    <DocsPage
      eyebrow="Using Payday"
      title="TypeScript SDK"
      lead="A typed client with no runtime dependencies, for Node 18 and up or any runtime with fetch. Field names are the API's own snake_case, so the reference applies unchanged."
    >
      <CodeBlock code={INSTALL} lang="bash" />
      <p>
        There are two clients. <code>PaydayClient</code> holds a credential and is for your server.{" "}
        <code>PaydayPayerClient</code> holds nothing and reads the public routes behind a deposit
        link, so it can run in a browser.
      </p>

      <H2 id="merchant-client">The merchant client</H2>
      <CodeBlock code={BASIC} lang="ts" />

      <Table>
        <thead>
          <tr>
            <th>Namespace</th>
            <th>Methods</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>depositRequests</td>
            <td>
              <code>create</code>, <code>get</code>, <code>list</code>, <code>cancel</code>,{" "}
              <code>transfers</code>, <code>attachment</code>, <code>requestPdf</code>,{" "}
              <code>proof</code>, <code>verification</code>, <code>createClientSecret</code>,{" "}
              <code>previewSession</code>
            </td>
          </tr>
          <tr>
            <td>customers</td>
            <td>
              <code>create</code>, <code>get</code>, <code>list</code>, <code>update</code>
            </td>
          </tr>
          <tr>
            <td>issuers</td>
            <td>
              <code>create</code>, <code>get</code>, <code>list</code>, <code>update</code>,{" "}
              <code>remove</code>, <code>startEmailVerification</code>,{" "}
              <code>confirmEmailVerification</code>, <code>setPayoutAddresses</code>
            </td>
          </tr>
          <tr>
            <td>payoutAddresses</td>
            <td>
              <code>create</code>, <code>list</code>, <code>remove</code>
            </td>
          </tr>
          <tr>
            <td>attachments</td>
            <td>
              <code>upload</code> (the whole exchange), <code>create</code>, <code>finalize</code>
            </td>
          </tr>
          <tr>
            <td>webhooks</td>
            <td>
              <code>add</code>, <code>list</code>, <code>get</code>, <code>remove</code>,{" "}
              <code>test</code>, <code>deliveries</code>
            </td>
          </tr>
          <tr>
            <td>account</td>
            <td>
              <code>get</code>, <code>issueApiKey</code>, <code>revokeApiKey</code>
            </td>
          </tr>
          <tr>
            <td>status</td>
            <td>Chain, indexer, and settlement-queue health.</td>
          </tr>
        </tbody>
      </Table>
      <p>
        Every method maps to one route in the <a href="/docs/api">API reference</a>, takes and
        returns the same fields, and is typed. <code>attachments.upload</code> is the one
        convenience: it reserves the slot, sends the bytes with the presigned headers, and polls
        finalization with backoff until the scan admits or rejects the file. Pass an{" "}
        <code>AbortSignal</code> to cancel any call.
      </p>

      <H2 id="errors">Errors</H2>
      <p>
        Every API failure throws <code>PaydayError</code> with the stable code, the HTTP status, the
        request id, and a message that names the problem.
      </p>
      <CodeBlock code={ERRORS} lang="ts" />

      <H2 id="helpers">Helpers</H2>
      <ul>
        <li>
          <code>checkoutUrl(request, clientSecret)</code> builds the merchant-session link with the
          secret in the URL fragment.
        </li>
        <li>
          <code>previewUrl(request, previewSession)</code> builds the link that opens a
          request&apos;s payer view unlocked for its own issuer, for checking before sending.
        </li>
      </ul>

      <H2 id="payer-client">The payer client</H2>
      <p>
        For a checkout of your own. It reads what the hosted page reads and exposes the verification
        and wallet writes. See{" "}
        <a href="/docs/checkout#build-your-own">Building your own checkout</a> for the flow.
      </p>
      <CodeBlock code={PAYER} lang="ts" />

      <H2 id="dashboard-sessions">Dashboard sessions</H2>
      <p>
        The client also accepts a dashboard session token in place of a key, which is how
        Payday&apos;s own dashboard calls the API from a browser without a key ever existing there.
        Exactly one of the two credentials is allowed.
      </p>
      <CodeBlock code={SESSION} lang="ts" />

      <Callout tone="danger" title="Never ship an API key to a browser">
        A key has full authority over the account. Browser code uses <code>PaydayPayerClient</code>,
        which needs no credential, or calls your own server.
      </Callout>
    </DocsPage>
  );
}
