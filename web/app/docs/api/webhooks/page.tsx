import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { Answers, Endpoint, Params } from "@/components/docs/endpoint";
import { DocsPage } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Webhooks API",
  description:
    "Register, list, test, and disable webhook endpoints, and page through their delivery history.",
};

const WEBHOOK = `{
  "id": "wh_0198f80c-3333-7dc1-a369-90556a64f700",
  "url": "https://example.com/payday/webhook",
  "secret": "whsec_…",
  "created_at": "2026-09-06T12:00:00Z",
  "disabled_at": null
}`;

const DELIVERIES = `{
  "deliveries": [
    {
      "id": "whd_0198f80c-…",
      "event_id": "evt_0198f80c-…",
      "endpoint_id": "wh_0198f80c-…",
      "state": "delivered",
      "attempt_count": 2,
      "next_attempt_at": "2026-09-06T12:01:00Z",
      "delivered_at": "2026-09-06T12:01:03Z",
      "created_at": "2026-09-06T12:00:00Z",
      "attempts": [
        { "number": 1, "attempted_at": "2026-09-06T12:00:00Z", "duration_ms": 3012, "status": null, "error": "connect timeout" },
        { "number": 2, "attempted_at": "2026-09-06T12:01:03Z", "duration_ms": 84, "status": 200, "error": null }
      ]
    }
  ],
  "next_cursor": null
}`;

export default function WebhooksApiPage() {
  return (
    <DocsPage
      eyebrow="API reference"
      title="Webhooks"
      lead="Endpoints and their delivery history. For the events, the delivery format, signatures, and retries, see the Webhooks guide."
    >
      <p>
        Everything about what a delivery contains and how to verify it is in{" "}
        <a href="/docs/webhooks">Webhooks</a>. This page is the routes.
      </p>

      <Endpoint method="POST" path="/v1/webhooks">
        Register an endpoint. The signing secret is returned once.
      </Endpoint>
      <Params
        title="Body"
        rows={[
          {
            name: "url",
            type: "string",
            required: true,
            description:
              "A credential-free HTTPS URL with a DNS hostname. Private, loopback, link-local, and reserved destinations are rejected, at registration and before every delivery. Redirects are never followed.",
          },
        ]}
      />
      <CodeBlock code={WEBHOOK} lang="json" title="201 Created" />
      <Answers
        rows={[
          {
            status: 400,
            code: "invalid_request",
            when: "The URL is not acceptable; the message says why.",
          },
          {
            status: 503,
            code: "webhooks_unavailable",
            when: "The environment cannot store secrets.",
          },
        ]}
      />

      <Endpoint method="GET" path="/v1/webhooks">
        Active endpoints as <code>{`{ webhooks }`}</code>, without secrets.
      </Endpoint>

      <Endpoint method="GET" path="/v1/webhooks/{id}">
        One endpoint, disabled or not, without its secret.
      </Endpoint>

      <Endpoint method="DELETE" path="/v1/webhooks/{id}">
        Disable the endpoint. <code>204</code>, idempotent. Its delivery history stays readable, and
        the same URL can be registered again with a new secret.
      </Endpoint>

      <Endpoint method="POST" path="/v1/webhooks/{id}/test">
        Queue a <code>webhook.test</code> event to this endpoint only. Answers{" "}
        <code>202 {`{ delivery_id }`}</code>.
      </Endpoint>
      <Answers
        rows={[
          {
            status: 404,
            code: "webhook_not_found",
            when: "Missing, another account's, or disabled.",
          },
        ]}
      />

      <Endpoint method="GET" path="/v1/webhook-deliveries">
        Deliveries newest first, each with its immutable attempt history.
      </Endpoint>
      <Params
        title="Query"
        rows={[
          {
            name: "endpoint_id",
            type: "wh_ id",
            description: "Only this endpoint's deliveries, disabled or not.",
          },
          { name: "limit", type: "integer", description: "1 to 100, default 20." },
          {
            name: "starting_after",
            type: "whd_ id",
            description: "The previous page's next_cursor.",
          },
        ]}
      />
      <CodeBlock code={DELIVERIES} lang="json" title="200 OK" />
      <p>
        <code>state</code> is <code>pending</code>, <code>delivered</code>, or <code>failed</code>{" "}
        (after the twelfth failed attempt). Each attempt records the HTTP status or the transport
        error, when it happened, and how long it took, so a missed event can be found and reconciled
        from the event id.
      </p>
    </DocsPage>
  );
}
