import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { Answers, Endpoint, Params } from "@/components/docs/endpoint";
import { Callout, DocsPage } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Account and status API",
  description:
    "Read the account and its key state, mint or revoke a key from a dashboard session, and check service health.",
};

const ACCOUNT = `{
  "account_id": "acct_0198f80c-6666-7dc1-a369-90556a64f700",
  "email": "ops@acme.example",
  "wallet_address": "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC",
  "key_hint": "…aB3f9Q",
  "generation": 3,
  "created_at": "2026-08-01T09:00:00Z",
  "rotated_at": "2026-09-06T12:00:00Z",
  "previous_key_expires_at": "2026-09-07T12:00:00Z",
  "revoked_at": null
}`;

const ISSUED = `{
  "api_key": "payday_live_…",
  "generation": 4,
  "replaced_previous_key": true
}`;

const STATUS = `{
  "chain": { "id": "143", "name": "Monad", "finalized_block": "98765432", "finalized_at": "2026-09-06T12:00:00Z" },
  "indexer": { "cursor_block": "98765430", "cursor_at": "2026-09-06T12:00:01Z", "lag_blocks": 2 },
  "sweeper": { "state": "idle", "queued": 0 }
}`;

export default function AccountApiPage() {
  return (
    <DocsPage
      eyebrow="API reference"
      title="Account and status"
      lead="Who you are, where you settle by default, the state of your key, and whether the service is keeping up with the chain."
    >
      <Endpoint method="GET" path="/v1/account">
        The account, with either credential.
      </Endpoint>
      <CodeBlock code={ACCOUNT} lang="json" title="200 OK" />
      <ul>
        <li>
          <code>wallet_address</code> is the account&apos;s own EVM wallet, created at first
          sign-in, where deposits settle by default. <code>null</code> only until the first
          dashboard session that carried one.
        </li>
        <li>
          <code>key_hint</code> is the key&apos;s last six characters, for telling keys apart. The
          key itself is never returned here.
        </li>
        <li>
          <code>generation</code> increments on every issuance or rotation and guards the two routes
          below.
        </li>
      </ul>

      <Endpoint method="POST" path="/v1/account/api-key">
        Issue a first key, or rotate the current one. Dashboard session only.
      </Endpoint>
      <Params
        title="Body"
        rows={[
          {
            name: "expected_generation",
            type: "integer",
            required: true,
            description:
              "The generation the account currently reports. A mismatch means something else changed the key first.",
          },
        ]}
      />
      <CodeBlock code={ISSUED} lang="json" title="201 Created (first key) or 200 OK (rotation)" />
      <Answers
        rows={[
          { status: 201, when: "The first key, returned once." },
          { status: 200, when: "A rotation. The previous key keeps working for 24 hours." },
          {
            status: 401,
            code: "identity_unauthorized",
            when: "An API key was presented. Only a dashboard session may mint.",
          },
          {
            status: 409,
            code: "api_key_generation_conflict",
            when: "The generation moved. Re-read the account and decide again.",
          },
        ]}
      />

      <Endpoint method="DELETE" path="/v1/account/api-key">
        Revoke the current key and any key still in its grace window. Dashboard session only.{" "}
        <code>204</code>.
      </Endpoint>
      <p>
        Takes the same <code>{`{ "expected_generation": N }`}</code> body. A later issuance starts a
        new generation.
      </p>

      <Callout tone="danger" title="Why a key cannot mint a key">
        Whoever holds a key must not be able to widen or prolong their own access. Minting and
        revoking take a signed-in merchant, which is why these two routes refuse the API-key form of
        the same bearer header.
      </Callout>

      <Endpoint method="GET" path="/v1/status">
        Chain, indexer, and settlement-queue health. API key.
      </Endpoint>
      <CodeBlock code={STATUS} lang="json" title="200 OK" />
      <p>
        Use it when a payer can see a transaction that a request does not yet reflect:{" "}
        <code>lag_blocks</code> says how far behind the finalized head the indexer is. Answers{" "}
        <code>503</code> when the state cannot be read.
      </p>

      <Endpoint method="GET" path="/health">
        Unauthenticated readiness: <code>ok</code> on <code>200</code>, or{" "}
        <code>database unavailable</code> on <code>503</code>.
      </Endpoint>
    </DocsPage>
  );
}
