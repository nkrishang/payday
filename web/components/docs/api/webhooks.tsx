import type { EndpointGroup } from "./types";

const WEBHOOK = `{
  "id": "wh_0198f80c-3333-7dc1-a369-90556a64f700",
  "url": "https://example.com/payday/webhook",
  "created_at": "2026-09-06T12:00:00Z",
  "disabled_at": null
}`;

export const WEBHOOKS: EndpointGroup = {
  slug: "webhooks",
  title: "Webhooks",
  endpoints: [
    {
      slug: "create",
      title: "Register an endpoint",
      method: "POST",
      path: "/v1/webhooks",
      auth: "key",
      summary: "Registers a webhook endpoint. The signing secret is returned once.",
      body: (
        <p>
          HTTPS; DNS hostname; no credentials. Private, loopback, link-local, and reserved
          destinations are rejected at registration and before each delivery. Redirects are not
          followed. Event types, payloads, signatures, retry policy:{" "}
          <a href="/docs/webhooks">Webhooks</a>.
        </p>
      ),
      bodyFields: [{ name: "url", type: "string", required: true, description: "" }],
      response: {
        description: (
          <>
            <code>Webhook</code> with <code>secret</code>. This response only.
          </>
        ),
      },
      answers: [
        { status: 400, code: "invalid_request", when: "URL rejected. Message states why." },
        { status: 503, code: "webhooks_unavailable", when: "No encryption key configured." },
      ],
      examples: {
        curl: `curl -fsS "$API/v1/webhooks" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{ "url": "https://example.com/payday/webhook" }'`,
        ts: `const endpoint = await payday.webhooks.add("https://example.com/payday/webhook");`,
        response: WEBHOOK.replace('"created_at"', '"secret": "whsec_…",\n  "created_at"'),
        responseTitle: "201 Created",
      },
    },
    {
      slug: "list",
      title: "List endpoints",
      method: "GET",
      path: "/v1/webhooks",
      auth: "key",
      summary: "Lists active endpoints. Secrets omitted.",
      response: { description: "{ webhooks: Webhook[] }." },
      examples: {
        curl: `curl -fsS "$API/v1/webhooks" -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const { webhooks } = await payday.webhooks.list();`,
        response: `{
  "webhooks": [ ${WEBHOOK.replace(/\n/g, "\n    ")} ]
}`,
      },
    },
    {
      slug: "get",
      title: "Get an endpoint",
      method: "GET",
      path: "/v1/webhooks/{id}",
      auth: "key",
      summary: "Retrieves an endpoint, active or disabled. Secret omitted.",
      pathParams: [{ name: "id", type: "wh_ id", required: true, description: "" }],
      response: { description: "Webhook." },
      answers: [{ status: 404, code: "webhook_not_found", when: "" }],
      examples: {
        curl: `curl -fsS "$API/v1/webhooks/wh_0198f80c-…" -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const endpoint = await payday.webhooks.get(id);`,
        response: WEBHOOK,
      },
    },
    {
      slug: "delete",
      title: "Disable an endpoint",
      method: "DELETE",
      path: "/v1/webhooks/{id}",
      auth: "key",
      summary: "Disables an endpoint. Idempotent. Delivery history remains readable.",
      pathParams: [{ name: "id", type: "wh_ id", required: true, description: "" }],
      response: { description: "204 No Content." },
      examples: {
        curl: `curl -fsS -X DELETE "$API/v1/webhooks/wh_0198f80c-…" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `await payday.webhooks.remove(id);`,
        response: `HTTP/1.1 204 No Content`,
        responseLang: "http",
      },
    },
    {
      slug: "test",
      title: "Send a test event",
      method: "POST",
      path: "/v1/webhooks/{id}/test",
      auth: "key",
      summary: "Queues a webhook.test event to one endpoint.",
      pathParams: [{ name: "id", type: "wh_ id", required: true, description: "Active endpoint." }],
      response: { fields: [{ name: "delivery_id", type: "whd_ id", description: "" }] },
      answers: [
        { status: 202, when: "" },
        { status: 404, code: "webhook_not_found", when: "Unknown or disabled." },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/webhooks/wh_0198f80c-…/test" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const { delivery_id } = await payday.webhooks.test(id);`,
        response: `{
  "delivery_id": "whd_0198f80c-5555-7dc1-a369-90556a64f700"
}`,
        responseTitle: "202 Accepted",
      },
    },
    {
      slug: "deliveries",
      title: "List deliveries",
      method: "GET",
      path: "/v1/webhook-deliveries",
      auth: "key",
      summary: "Lists deliveries, newest first, with attempt history.",
      query: [
        { name: "endpoint_id", type: "wh_ id", description: "Disabled endpoints included." },
        { name: "limit", type: "integer", description: "1–100. Default 20." },
        { name: "starting_after", type: "whd_ id", description: "Cursor from next_cursor." },
      ],
      response: {
        fields: [
          { name: "id", type: "whd_ id", description: "" },
          { name: "event_id", type: "evt_ id", description: "Also sent as Payday-Event-Id." },
          { name: "endpoint_id", type: "wh_ id", description: "" },
          {
            name: "state",
            type: "enum",
            description: (
              <>
                <code>pending</code> · <code>delivered</code> · <code>failed</code> (after twelve
                attempts).
              </>
            ),
          },
          {
            name: "attempt_count, next_attempt_at, delivered_at, created_at",
            type: "",
            description: "",
          },
          {
            name: "attempts",
            type: "object[]",
            description: "{ number, attempted_at, duration_ms, status, error }. Immutable.",
          },
        ],
      },
      examples: {
        curl: `curl -fsS "$API/v1/webhook-deliveries?endpoint_id=wh_0198f80c-…&limit=20" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const page = await payday.webhooks.deliveries({ endpoint_id: id, limit: 20 });`,
        response: `{
  "deliveries": [
    {
      "id": "whd_0198f80c-5555-7dc1-a369-90556a64f700",
      "event_id": "evt_0198f80c-4444-7dc1-a369-90556a64f700",
      "endpoint_id": "wh_0198f80c-3333-7dc1-a369-90556a64f700",
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
}`,
      },
    },
  ],
};
