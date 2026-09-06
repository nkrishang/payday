import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { CodeTabs } from "@/components/docs/code-tabs";
import { SignatureFigure } from "@/components/docs/figures";
import { Callout, DocsPage, Figure, H2, H3, Pill, Table } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Webhooks",
  description:
    "Let Payday tell your server what happened: every event, the signed delivery format, how to verify it, and how retries work.",
};

const REGISTER_CURL = `curl -fsS "$API/v1/webhooks" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{ "url": "https://example.com/payday/webhook" }'`;

const REGISTER_TS = `const endpoint = await payday.webhooks.add("https://example.com/payday/webhook");
// endpoint.secret is returned exactly once. Store it beside the API key.`;

const REGISTER_RESPONSE = `{
  "id": "wh_0198f80c-3333-7dc1-a369-90556a64f700",
  "url": "https://example.com/payday/webhook",
  "secret": "whsec_…",
  "created_at": "2026-09-06T12:00:00Z",
  "disabled_at": null
}`;

const DELIVERY = `POST /payday/webhook HTTP/1.1
Content-Type: application/json
Payday-Event-Id: evt_0198f80c-4444-7dc1-a369-90556a64f700
Payday-Event-Type: deposit_request.settled
Payday-Signature: v1,t=1756728000,sha256=6f1a…9c0e

{
  "id": "evt_0198f80c-4444-7dc1-a369-90556a64f700",
  "type": "deposit_request.settled",
  "occurred_at": "2026-09-01T12:00:00Z",
  "data": {
    "deposit_request": {
      "id": "dr_0198f80c-8d2f-7dc1-a369-90556a64f700",
      "status": "settled",
      "amount": "10.500000",
      "amount_base_units": "10500000",
      "received": "10.500000",
      "received_base_units": "10500000",
      "heading": "March retainer",
      "reference": "INV-1042",
      "metadata": { "po": "PO-77" },
      "customer_id": null,
      "issuer_id": "iss_0198f80c-1111-7dc1-a369-90556a64f700",
      "payer_policy_mode": "merchant_session",
      "payer_reference": "user_123",
      "verification_completed_at": "2026-09-01T11:58:00Z",
      "likely_unsolicited_at": null,
      "payer_wallet": "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed",
      "address": "0x2222222222222222222222222222222222222222",
      "wallet_bound_at": "2026-09-01T11:58:30Z",
      "expires_at": "2026-09-02T11:57:00Z",
      "created_at": "2026-09-01T11:57:00Z"
    }
  }
}`;

const RECOVERY = `{
  "type": "deposit_request.recovered_funds",
  "data": {
    "deposit_request": { "id": "dr_0198f80c-…", "status": "settled", "…": "…" },
    "recovery": {
      "id": "rec_0198f80c-2222-7dc1-a369-90556a64f700",
      "amount": "0.250000",
      "amount_base_units": "250000",
      "reason": "overpayment",
      "transaction_hash": "0x…",
      "block_number": "12345",
      "recovered_at": "2026-09-01T12:00:00Z"
    }
  }
}`;

const VERIFY_NODE = `import { createHmac, timingSafeEqual } from "node:crypto";

const TOLERANCE_SECONDS = 300;

export function verify(rawBody: Buffer, header: string, secret: string): boolean {
  const [scheme, ...rest] = header.split(",");
  if (scheme !== "v1") return false;
  const parts = Object.fromEntries(rest.map((p) => p.split("=", 2) as [string, string]));
  if (!parts.t || !parts.sha256) return false;

  const age = Math.abs(Date.now() / 1000 - Number(parts.t));
  if (!Number.isFinite(age) || age > TOLERANCE_SECONDS) return false;

  const expected = createHmac("sha256", secret)
    .update(\`v1.\${parts.t}.\`)
    .update(rawBody)
    .digest("hex");
  const given = Buffer.from(parts.sha256, "hex");
  const want = Buffer.from(expected, "hex");
  return given.length === want.length && timingSafeEqual(given, want);
}

// Express: keep the raw bytes; a re-serialised body will not verify.
app.post("/payday/webhook", express.raw({ type: "application/json" }), (req, res) => {
  if (!verify(req.body, req.header("Payday-Signature") ?? "", process.env.PAYDAY_WEBHOOK_SECRET!)) {
    return res.status(400).end();
  }
  const event = JSON.parse(req.body.toString("utf8"));
  if (await alreadyHandled(event.id)) return res.status(200).end(); // idempotent
  await handle(event);
  res.status(200).end();
});`;

const VERIFY_PY = `import hmac, hashlib, time

TOLERANCE = 300

def verify(raw_body: bytes, header: str, secret: str) -> bool:
    scheme, *rest = header.split(",")
    if scheme != "v1":
        return False
    parts = dict(p.split("=", 1) for p in rest)
    if "t" not in parts or "sha256" not in parts:
        return False
    if abs(time.time() - int(parts["t"])) > TOLERANCE:
        return False
    signed = b"v1." + parts["t"].encode() + b"." + raw_body
    expected = hmac.new(secret.encode(), signed, hashlib.sha256).hexdigest()
    return hmac.compare_digest(expected, parts["sha256"])`;

export default function WebhooksPage() {
  return (
    <DocsPage
      eyebrow="Using Payday"
      title="Webhooks"
      lead="Register an HTTPS endpoint and Payday tells your server the moment a request is ready, funded, settled, expired, or returned, and every time funds go back to the payer. Each delivery is signed; each event happens once."
    >
      <H2 id="register">Register an endpoint</H2>
      <p>
        The URL must be a public HTTPS destination with a DNS hostname and no credentials in it.
        Payday resolves and rejects private, loopback, and reserved addresses when the endpoint is
        created and again before every delivery, and never follows redirects.
      </p>
      <CodeTabs
        tabs={[
          { label: "curl", content: <CodeBlock code={REGISTER_CURL} lang="bash" /> },
          { label: "TypeScript", content: <CodeBlock code={REGISTER_TS} lang="ts" /> },
        ]}
      />
      <CodeBlock code={REGISTER_RESPONSE} lang="json" title="201 Created" />
      <p>
        The <code>secret</code> is returned once. Later reads of the endpoint omit it. To rotate,
        register the same URL again for a new secret and disable the old endpoint; its delivery
        history stays readable. Send yourself a test with{" "}
        <code>POST /v1/webhooks/&#123;id&#125;/test</code>, which queues a <code>webhook.test</code>{" "}
        event to that endpoint alone.
      </p>

      <H2 id="events">Events</H2>
      <Table>
        <thead>
          <tr>
            <th>Event</th>
            <th>When</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>
              <Pill tone="green">deposit_request.ready</Pill>
            </td>
            <td>
              The payer attested their wallet and the one-time address now exists. The moment an
              integration may quote the address.
            </td>
          </tr>
          <tr>
            <td>
              <Pill tone="green">verification.approved</Pill>
            </td>
            <td>
              The payer policy was satisfied: a mailbox proven, or a merchant-session secret
              exchanged.
            </td>
          </tr>
          <tr>
            <td>
              <Pill>deposit_request.deposited</Pill>
            </td>
            <td>Finalized credits reached the amount. Settlement is queued.</td>
          </tr>
          <tr>
            <td>
              <Pill tone="green">deposit_request.settled</Pill>
            </td>
            <td>Exactly the amount reached your payout address.</td>
          </tr>
          <tr>
            <td>
              <Pill>deposit_request.expired</Pill>
            </td>
            <td>The deadline passed before settlement.</td>
          </tr>
          <tr>
            <td>
              <Pill tone="yellow">deposit_request.returned</Pill>
            </td>
            <td>The whole balance went back to the payer&apos;s wallet after expiry.</td>
          </tr>
          <tr>
            <td>
              <Pill tone="yellow">deposit_request.recovered_funds</Pill>
            </td>
            <td>
              Funds went back to the payer&apos;s wallet: an overpayment remainder, an expired
              balance, or a late transfer. One event per amount returned, so a request can raise
              this more than once.
            </td>
          </tr>
          <tr>
            <td>
              <Pill tone="yellow">deposit_request.likely_unsolicited</Pill>
            </td>
            <td>Finalized funds first arrived from a wallet other than the attested one.</td>
          </tr>
          <tr>
            <td>
              <Pill tone="yellow">deposit_request.needs_attention</Pill>
            </td>
            <td>
              Automatic movement paused. The payload carries the same <code>attention</code> object
              the API shows.
            </td>
          </tr>
          <tr>
            <td>
              <Pill>webhook.test</Pill>
            </td>
            <td>Sent on request to one endpoint. Carries no deposit request.</td>
          </tr>
        </tbody>
      </Table>
      <p>
        The lifecycle events (<code>deposited</code>, <code>settled</code>, <code>expired</code>,{" "}
        <code>returned</code>, <code>needs_attention</code>) happen at most once per request, and
        each is written in the same database transaction that changes the request, so an event can
        never exist without its state change or the other way around.
      </p>

      <H2 id="delivery">What a delivery looks like</H2>
      <p>
        Every delivery is a <code>POST</code> with three headers and a JSON envelope. The{" "}
        <code>data.deposit_request</code> object is a strict subset of the API&apos;s own deposit
        request, under the same names, units, and formats, so a handler can hand <code>id</code>{" "}
        straight to the API and compare <code>amount</code> without conversion.
      </p>
      <CodeBlock code={DELIVERY} lang="http" title="A delivery" />
      <ul>
        <li>
          <code>payer_reference</code> is your own id for the payer on a merchant-session request
          and <code>null</code> otherwise, so a settled handler credits the right ledger directly.
        </li>
        <li>
          <code>payer_wallet</code>, <code>address</code>, and <code>wallet_bound_at</code> are{" "}
          <code>null</code> before <code>deposit_request.ready</code>.
        </li>
        <li>
          The payload never carries the expected email or any payer data beyond what you asserted.
        </li>
        <li>
          A settled event for an overpaid request looks the same as one for an exact deposit; the
          recovery is its own event.
        </li>
      </ul>
      <p>
        <code>deposit_request.recovered_funds</code> adds <code>data.recovery</code>:
      </p>
      <CodeBlock code={RECOVERY} lang="json" />

      <H2 id="verify">Verify the signature</H2>
      <Figure caption="The signature header. Your secret, the timestamp, and the exact bytes of the body are the only inputs.">
        <SignatureFigure />
      </Figure>
      <p>
        Compute HMAC-SHA256 with the endpoint&apos;s secret over the bytes{" "}
        <code>v1.&lt;timestamp&gt;.&lt;raw body&gt;</code>, compare it with the header&apos;s hex in
        constant time, and reject timestamps outside your tolerance. Use the raw request bytes; a
        body that has been parsed and re-serialised will not match.
      </p>
      <CodeTabs
        tabs={[
          { label: "Node", content: <CodeBlock code={VERIFY_NODE} lang="ts" /> },
          { label: "Python", content: <CodeBlock code={VERIFY_PY} lang="text" /> },
        ]}
      />

      <H2 id="retries">Retries and idempotency</H2>
      <ul>
        <li>
          Answer with any <code>2xx</code> within a few seconds. Do the work afterwards if it is
          slow.
        </li>
        <li>
          A non-<code>2xx</code> answer, a timeout, or a connection failure is retried with
          exponential backoff, up to 12 attempts, with the gap capped at one hour. Redirects are not
          followed.
        </li>
        <li>
          After the twelfth failure the delivery is <code>failed</code> and stays that way. The{" "}
          <a href="/docs/api/webhooks#get-webhook-deliveries">deliveries route</a> shows every
          attempt with its status, error, time, and duration, so a missed event can be reconciled by
          hand.
        </li>
        <li>
          Deduplicate on <code>Payday-Event-Id</code>. A retry carries the same id and the same
          body.
        </li>
      </ul>

      <H3 id="poll-or-subscribe">Poll or subscribe?</H3>
      <p>
        Use webhooks for anything automatic: crediting a ledger, sending a receipt, releasing an
        order. Use the API&apos;s long polling (<code>wait_for=change</code>) for a screen that is
        open right now. Treat the API as the source of truth: a handler that reads the request back
        before acting is never wrong, whatever order deliveries arrived in.
      </p>

      <Callout title="Webhook secrets">
        Secrets are stored encrypted, returned once, and never logged. Keep yours beside the API key
        in a secret manager. Support will never ask for it.
      </Callout>
    </DocsPage>
  );
}
