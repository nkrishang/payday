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
      summary:
        "Register an HTTPS endpoint. The signing secret is returned once; later reads omit it. Events, signatures, payloads, and retries are in the Webhooks guide.",
      body: (
        <p>
          The URL must be a public HTTPS destination with a DNS hostname and no credentials in it.
          Private, loopback, link-local, and reserved destinations are rejected at registration and
          again before every delivery. Redirects are never followed. See{" "}
          <a href="/docs/webhooks">Webhooks</a>.
        </p>
      ),
      bodyFields: [
        {
          name: "url",
          type: "string",
          required: true,
          description: "A credential-free public HTTPS URL.",
        },
      ],
      response: {
        description: (
          <>
            The endpoint with <code>secret</code>, on this response only.
          </>
        ),
      },
      answers: [
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
      ],
      examples: {
        curl: `curl -fsS "$API/v1/webhooks" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{ "url": "https://example.com/payday/webhook" }'`,
        ts: `const endpoint = await payday.webhooks.add("https://example.com/payday/webhook");
// endpoint.secret is returned exactly once. Store it beside the API key.`,
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
      summary: "Active endpoints, without secrets.",
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
      summary: "One endpoint, disabled or not, without its secret.",
      pathParams: [{ name: "id", type: "wh_ id", required: true, description: "The endpoint." }],
      response: { description: "The endpoint." },
      answers: [
        {
          status: 404,
          code: "webhook_not_found",
          when: "Missing, malformed, or another account's.",
        },
      ],
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
      summary:
        "Disable the endpoint. Idempotent; its delivery history stays readable, and the same URL can be registered again with a new secret.",
      pathParams: [{ name: "id", type: "wh_ id", required: true, description: "The endpoint." }],
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
      summary: "Queue a webhook.test event to this endpoint only. It carries no deposit request.",
      pathParams: [
        { name: "id", type: "wh_ id", required: true, description: "An active endpoint." },
      ],
      response: {
        fields: [
          {
            name: "delivery_id",
            type: "whd_ id",
            description: "Follow it on the deliveries route.",
          },
        ],
      },
      answers: [
        { status: 202, when: "Queued." },
        {
          status: 404,
          code: "webhook_not_found",
          when: "Missing, another account's, or disabled.",
        },
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
      summary:
        "Deliveries newest first, each with its immutable attempt history, so a missed event can be found and reconciled.",
      query: [
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
      ],
      response: {
        fields: [
          { name: "id", type: "whd_ id", description: "The delivery." },
          {
            name: "event_id",
            type: "evt_ id",
            description: "The event, also sent as Payday-Event-Id.",
          },
          { name: "endpoint_id", type: "wh_ id", description: "The endpoint." },
          {
            name: "state",
            type: "string",
            description: (
              <>
                <code>pending</code>, <code>delivered</code>, or <code>failed</code> after the
                twelfth failed attempt.
              </>
            ),
          },
          {
            name: "attempt_count, next_attempt_at, delivered_at, created_at",
            type: "…",
            description: "Scheduling and outcome.",
          },
          {
            name: "attempts",
            type: "object[]",
            description: "{ number, attempted_at, duration_ms, status, error } each.",
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
