import Link from "next/link";
import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { DocsPage, H2, H3, Table } from "@/components/docs/prose";
import { config } from "@/lib/config";

export const metadata: Metadata = {
  title: "API reference",
  description:
    "Base URLs, authentication, ids, error shape, pagination, idempotency, formats, rate limits, and body limits.",
};

const AUTH = `Authorization: Bearer payday_live_…
Content-Type: application/json`;

const ERROR = `HTTP/1.1 400 Bad Request
X-Request-Id: 0198f80c-5555-7dc1-a369-90556a64f700

{
  "error": { "code": "invalid_request", "message": "missing field \`amount\`" },
  "request_id": "0198f80c-5555-7dc1-a369-90556a64f700"
}`;

const PAGE = `GET /v1/deposit-requests?status=partially_deposited&limit=50
GET /v1/deposit-requests?limit=50&starting_after=dr_0198f80c-…`;

const RATE = `X-RateLimit-Limit: 60
X-RateLimit-Remaining: 42
X-RateLimit-Reset: 1756728061`;

export default function ApiIntroductionPage() {
  return (
    <DocsPage
      eyebrow="API reference"
      title="Introduction"
      lead="REST over HTTPS. JSON request and response bodies. Bearer authentication. Prefixed ids. Cursor pagination. Idempotent creates."
    >
      <H2 id="base-urls">Base URLs</H2>
      <Table>
        <thead>
          <tr>
            <th>Environment</th>
            <th>Base URL</th>
            <th>Key prefix</th>
            <th>Networks</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>Production</td>
            <td>
              <code>https://api.payday.sh</code>
            </td>
            <td>
              <code>payday_live_</code>
            </td>
            <td>Monad 143 · Base 8453 · Arbitrum One 42161</td>
          </tr>
          <tr>
            <td>Sandbox</td>
            <td>
              <code>https://api.sandbox.payday.sh</code>
            </td>
            <td>
              <code>payday_test_</code>
            </td>
            <td>Monad testnet 10143 · Base Sepolia 84532 · Arbitrum Sepolia 421614</td>
          </tr>
        </tbody>
      </Table>
      <p>
        All routes are under <code>/v1</code>. OpenAPI 3.1: <code>/openapi.json</code> on each
        origin; rendered at{" "}
        <a href={`${config.apiUrl}/docs`} target="_blank" rel="noopener noreferrer">
          /docs
        </a>
        .
      </p>

      <H2 id="authentication">Authentication</H2>
      <CodeBlock code={AUTH} lang="http" />
      <ul>
        <li>
          Merchant routes accept an API key or a dashboard session token in the same header. Full
          account authority; not scoped.
        </li>
        <li>One active key per account. Issuance replaces it.</li>
        <li>
          Rotation: previous key valid 24 hours. Revocation: immediate, including the grace key.
        </li>
        <li>
          Key management (<code>/v1/account/api-key</code>) accepts sessions only. A key is refused
          with <code>401 identity_unauthorized</code>.
        </li>
        <li>Keys are returned once and stored as SHA-256.</li>
        <li>
          <Link href="/docs/api/payer">Payer routes</Link> take no credential.
        </li>
      </ul>

      <H2 id="ids">Ids</H2>
      <p>
        UUIDs behind a resource prefix. Only the canonical form is accepted: a bare UUID or wrong
        prefix in a path is <code>404</code>; in a body or query, <code>400 invalid_request</code>.
      </p>
      <Table>
        <thead>
          <tr>
            <th>Prefix</th>
            <th>Resource</th>
          </tr>
        </thead>
        <tbody>
          {[
            ["dr_", "deposit request"],
            ["cus_", "customer"],
            ["iss_", "issuer identity"],
            ["pa_", "payout address"],
            ["att_", "attachment"],
            ["wh_", "webhook endpoint"],
            ["whd_", "webhook delivery"],
            ["evt_", "webhook event"],
            ["va_", "verification attempt"],
            ["rec_", "recovery ledger entry"],
            ["acct_", "account"],
          ].map(([prefix, resource]) => (
            <tr key={prefix}>
              <td>{prefix}</td>
              <td>{resource}</td>
            </tr>
          ))}
        </tbody>
      </Table>

      <H2 id="errors">Errors</H2>
      <CodeBlock code={ERROR} lang="http" />
      <ul>
        <li>
          <code>X-Request-Id</code> on every response. A caller-supplied value (≤128 printable
          bytes) is echoed; otherwise a UUIDv7 is generated. JSON errors repeat it as{" "}
          <code>request_id</code>.
        </li>
        <li>
          <code>400 invalid_request</code>: malformed JSON, missing or unknown field, wrong type,
          missing JSON content type, control character. The message names the field. Unknown fields
          are rejected on every body.
        </li>
        <li>
          <code>404 &lt;resource&gt;_not_found</code>: missing, malformed, or another account&apos;s
          id.
        </li>
        <li>
          Full code table: <Link href="/docs/api/errors">Errors</Link>.
        </li>
      </ul>

      <H2 id="conventions">Conventions</H2>
      <ul>
        <li>
          Creates: <code>201</code> with the resource. Deletes and disables: <code>204</code>.
        </li>
        <li>
          <code>PATCH</code> is partial. Omitted fields are unchanged; explicit <code>null</code>{" "}
          clears. The merged record is validated whole.
        </li>
        <li>
          Lists are enveloped under the plural (<code>deposit_requests</code>,{" "}
          <code>customers</code>, <code>issuers</code>, <code>payout_addresses</code>,{" "}
          <code>webhooks</code>, <code>deliveries</code>, <code>transfers</code>).
        </li>
        <li>Clients must tolerate unknown fields and branch only on documented values.</li>
      </ul>

      <H3 id="pagination">Pagination</H3>
      <p>
        <code>limit</code> 1–100, default 20. <code>next_cursor</code> is the last item&apos;s id or{" "}
        <code>null</code>; pass it as <code>starting_after</code>. Newest first.
      </p>
      <CodeBlock code={PAGE} lang="http" />

      <H3 id="idempotency">Idempotency</H3>
      <p>
        <code>POST /v1/deposit-requests</code> requires <code>Idempotency-Key</code> (1–255 bytes).
        Same key, same document: <code>200</code> with the original and{" "}
        <code>Idempotency-Replayed: true</code>. Same key, different immutable field:{" "}
        <code>409 idempotency_conflict</code>.
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
              Decimal string, six fractional digits (<code>&quot;25.000000&quot;</code>), with an
              integer <code>*_base_units</code> counterpart (<code>&quot;25000000&quot;</code>).
            </td>
          </tr>
          <tr>
            <td>Timestamps</td>
            <td>
              RFC 3339, UTC, second precision, <code>Z</code> suffix. Unix seconds only where
              stated.
            </td>
          </tr>
          <tr>
            <td>Addresses</td>
            <td>EIP-55 on output. Any case on input.</td>
          </tr>
          <tr>
            <td>Block numbers, log indexes</td>
            <td>Decimal strings.</td>
          </tr>
          <tr>
            <td>Hashes, signatures</td>
            <td>
              <code>0x</code>-prefixed lowercase hex.
            </td>
          </tr>
          <tr>
            <td>Text</td>
            <td>UTF-8. No C0 control characters. Line breaks and tabs preserved.</td>
          </tr>
        </tbody>
      </Table>

      <H2 id="rate-limits">Rate limits</H2>
      <p>
        Per account, API-key traffic: token bucket of 60, refill 1/s. Rejection:{" "}
        <code>429 rate_limited</code>, <code>Retry-After: 1</code>.
      </p>
      <CodeBlock code={RATE} lang="http" />

      <H2 id="limits">Body limits</H2>
      <Table>
        <thead>
          <tr>
            <th>Routes</th>
            <th>Limit</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>Deposit requests, customers, issuers, attachments, webhooks</td>
            <td>64 KiB</td>
          </tr>
          <tr>
            <td>Account key</td>
            <td>16 KiB</td>
          </tr>
          <tr>
            <td>Payer verification writes</td>
            <td>8 KiB</td>
          </tr>
        </tbody>
      </Table>
      <p>
        Exceeded: <code>413 payload_too_large</code>. PDF bytes go to the presigned upload URL,
        never to the API.
      </p>

      <H2 id="cors">CORS</H2>
      <ul>
        <li>Merchant routes: the dashboard origin only.</li>
        <li>
          Payer reads: <code>Access-Control-Allow-Origin: *</code>.
        </li>
        <li>Payer writes: the hosted checkout origin only.</li>
      </ul>
    </DocsPage>
  );
}
