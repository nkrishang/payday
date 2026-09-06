import Link from "next/link";
import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { Callout, Card, Cards, DocsPage, H2, H3, Table } from "@/components/docs/prose";
import { config } from "@/lib/config";

export const metadata: Metadata = {
  title: "API reference",
  description:
    "Base URLs, authentication, id prefixes, error shape, pagination, idempotency, rate limits, and the formats every Payday API response shares.",
};

const AUTH = `Authorization: Bearer payday_live_…
Content-Type: application/json`;

const ERROR = `HTTP/1.1 400 Bad Request
X-Request-Id: 0198f80c-5555-7dc1-a369-90556a64f700

{
  "error": {
    "code": "invalid_request",
    "message": "missing field \`amount\`"
  },
  "request_id": "0198f80c-5555-7dc1-a369-90556a64f700"
}`;

const PAGE = `GET /v1/deposit-requests?status=partially_deposited&limit=50
GET /v1/deposit-requests?limit=50&starting_after=dr_0198f80c-…   # the previous page's next_cursor`;

const RATE = `X-RateLimit-Limit: 60
X-RateLimit-Remaining: 42
X-RateLimit-Reset: 1756728061`;

export default function ApiConventionsPage() {
  return (
    <DocsPage
      eyebrow="API reference"
      title="Introduction"
      lead="One HTTP API behind the dashboard, the SDK, and every integration. This page is the set of rules every route follows; the pages in the sidebar are the routes, one each."
    >
      <Cards>
        <Card href={`${config.apiUrl}/docs`} title="OpenAPI reference" external>
          The API also describes itself in OpenAPI 3.1, a machine-readable document for client
          generators and API tools, served at <code>/openapi.json</code> on the API origin and
          rendered as a browsable page at <code>/docs</code> there. The pages here are the written
          reference; that one is the raw contract.
        </Card>
        <Card href="/docs/sdk" title="TypeScript SDK">
          Every route below, as a typed method.
        </Card>
      </Cards>

      <H2 id="base-urls">Base URLs</H2>
      <Table>
        <thead>
          <tr>
            <th>Environment</th>
            <th>Base URL</th>
            <th>Keys</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>Production</td>
            <td>
              <code>https://api.payday.sh</code>
            </td>
            <td>
              <code>payday_live_…</code>
            </td>
          </tr>
          <tr>
            <td>Sandbox</td>
            <td>
              <code>https://api.sandbox.payday.sh</code>
            </td>
            <td>
              <code>payday_test_…</code>
            </td>
          </tr>
        </tbody>
      </Table>
      <p>
        All routes are under <code>/v1</code>. Requests and responses are JSON, except the PDF and
        QR routes, which say so.
      </p>

      <H2 id="authentication">Authentication</H2>
      <CodeBlock code={AUTH} lang="http" />
      <p>
        Every merchant route takes a bearer credential. Normally that is an API key minted in the
        dashboard. The same header also accepts the session token a signed-in dashboard holds, which
        is how the dashboard calls the API without a key; the API tells the two apart. A credential
        has full read and write access to its account. Keys are not scoped yet.
      </p>
      <ul>
        <li>One active key per account. Minting another replaces it.</li>
        <li>
          Rotation keeps the previous key valid for 24 hours; revocation invalidates both at once.
        </li>
        <li>
          Only a dashboard session can mint, roll, or revoke a key. An API key is refused on those
          routes with <code>401 identity_unauthorized</code>.
        </li>
        <li>Keys are shown once and stored hashed. Keep them in a secret manager.</li>
      </ul>
      <p>
        The <Link href="/docs/api/payer">payer routes</Link> take no credential at all. They are the
        public face of every deposit link.
      </p>

      <H2 id="ids">Ids</H2>
      <p>
        Every id is a UUID behind a prefix that says what it names, so a log line or a support
        ticket reads for itself. Pass ids back exactly as received: only the canonical form is
        accepted, and a bare UUID or the wrong prefix is refused.
      </p>
      <Table>
        <thead>
          <tr>
            <th>Prefix</th>
            <th>Resource</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>dr_</td>
            <td>deposit request</td>
          </tr>
          <tr>
            <td>cus_</td>
            <td>customer</td>
          </tr>
          <tr>
            <td>iss_</td>
            <td>issuer identity</td>
          </tr>
          <tr>
            <td>pa_</td>
            <td>payout address</td>
          </tr>
          <tr>
            <td>att_</td>
            <td>attachment</td>
          </tr>
          <tr>
            <td>wh_</td>
            <td>webhook endpoint</td>
          </tr>
          <tr>
            <td>whd_</td>
            <td>webhook delivery</td>
          </tr>
          <tr>
            <td>evt_</td>
            <td>webhook event</td>
          </tr>
          <tr>
            <td>va_</td>
            <td>verification attempt</td>
          </tr>
          <tr>
            <td>rec_</td>
            <td>recovery ledger entry</td>
          </tr>
          <tr>
            <td>acct_</td>
            <td>account</td>
          </tr>
        </tbody>
      </Table>

      <H2 id="errors">Errors</H2>
      <p>
        Every response carries <code>X-Request-Id</code>. Send one of your own, up to 128 printable
        bytes, and it is echoed; otherwise Payday generates one. A JSON error names a stable code, a
        message that says what was wrong, and the same request id.
      </p>
      <CodeBlock code={ERROR} lang="http" />
      <p>
        A body or query that does not fit the route (malformed JSON, a missing or unknown field, a
        wrong type, a missing JSON content type, a control character in a text field) is{" "}
        <code>400 invalid_request</code> with the field named. Every request body rejects unknown
        fields. A missing, malformed, or another account&apos;s id is{" "}
        <code>404 &lt;resource&gt;_not_found</code>. The full list is on{" "}
        <Link href="/docs/api/errors">Errors</Link>.
      </p>

      <H2 id="shapes">Response shapes</H2>
      <ul>
        <li>
          Creates answer <code>201</code> with the resource. Disables and deletes answer{" "}
          <code>204</code>.
        </li>
        <li>
          <code>PATCH</code> is partial: a field left out keeps its value, and only an explicit{" "}
          <code>null</code> clears one. The merged record is validated whole.
        </li>
        <li>
          Lists are enveloped under the resource&apos;s plural (<code>deposit_requests</code>,{" "}
          <code>customers</code>, <code>issuers</code>, <code>payout_addresses</code>,{" "}
          <code>webhooks</code>, <code>deliveries</code>, <code>transfers</code>).
        </li>
        <li>Clients must tolerate new fields and branch only on documented values.</li>
      </ul>

      <H3 id="pagination">Pagination</H3>
      <p>
        Lists that page carry <code>next_cursor</code>: the id of the last item, or{" "}
        <code>null</code> at the end. Pass it back as <code>starting_after</code>.{" "}
        <code>limit</code> is 1 to 100 and defaults to 20. Lists are newest first.
      </p>
      <CodeBlock code={PAGE} lang="http" />

      <H3 id="idempotency">Idempotency</H3>
      <p>
        Creating a deposit request requires an <code>Idempotency-Key</code> header of 1 to 255
        bytes. A retry with the same key and the same document returns the original with{" "}
        <code>200</code> and <code>Idempotency-Replayed: true</code>. The same key with any changed
        immutable field (parties, amount, policy, expiry intent, attachment, metadata) is{" "}
        <code>409 idempotency_conflict</code>; fetch the original instead. Use your own order or
        invoice id as the key.
      </p>

      <H3 id="formats">Formats</H3>
      <Table>
        <thead>
          <tr>
            <th>Kind</th>
            <th>Format</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>USDC amounts</td>
            <td>
              Decimal strings with six fractional digits (<code>&quot;25.000000&quot;</code>),
              always beside an integer base-unit string (<code>&quot;25000000&quot;</code>). Do
              arithmetic on base units.
            </td>
          </tr>
          <tr>
            <td>Timestamps</td>
            <td>
              RFC 3339 in UTC to the second with a <code>Z</code> suffix, in responses and webhooks
              alike, unless a field says Unix seconds.
            </td>
          </tr>
          <tr>
            <td>Addresses</td>
            <td>EIP-55 checksummed on output; any case accepted on input.</td>
          </tr>
          <tr>
            <td>Block numbers, log indexes</td>
            <td>Decimal strings.</td>
          </tr>
          <tr>
            <td>Hashes</td>
            <td>
              <code>0x</code>-prefixed lowercase hex.
            </td>
          </tr>
          <tr>
            <td>Text fields</td>
            <td>
              UTF-8, bounded, no control characters. Multi-line text keeps its line breaks and tabs.
            </td>
          </tr>
        </tbody>
      </Table>

      <H2 id="rate-limits">Rate limits</H2>
      <p>
        API-key traffic has a per-account allowance of 60 requests, refilling one per second. Three
        headers report it; a rejected request is <code>429 rate_limited</code> with{" "}
        <code>Retry-After: 1</code>.
      </p>
      <CodeBlock code={RATE} lang="http" />

      <H2 id="limits">Body limits</H2>
      <p>
        Deposit request, customer, issuer, attachment, and webhook bodies are limited to 64 KiB;
        account-key bodies to 16 KiB; payer verification bodies to 8 KiB. PDF bytes never go to the
        API: they go to the presigned upload URL. A larger body is{" "}
        <code>413 payload_too_large</code>.
      </p>

      <H2 id="cors">Browsers</H2>
      <p>
        The merchant routes answer cross-origin requests from Payday&apos;s own dashboard origin
        only. The payer read routes answer any origin, which is what lets you build a checkout of
        your own; the payer write routes answer the hosted checkout&apos;s origin. Server-side
        integrations are unaffected by any of this.
      </p>

      <Callout title="Getting help">
        Email <a href="mailto:support@payday.sh">support@payday.sh</a> with the request id and, for
        a deposit, the <code>dr_</code> id. Never send an API key, a webhook secret, a verification
        code, or more of a request&apos;s data than the question needs.
      </Callout>
    </DocsPage>
  );
}
