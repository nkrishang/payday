import type { EndpointGroup } from "./types";

const CUSTOMER = `{
  "id": "cus_0198f80c-1111-7dc1-a369-90556a64f700",
  "name": "Globex",
  "email": "ap@globex.example",
  "details": "Attn: Accounts Payable",
  "created_at": "2026-09-06T12:00:00Z",
  "updated_at": "2026-09-06T12:00:00Z"
}`;

export const CUSTOMERS: EndpointGroup = {
  slug: "customers",
  title: "Customers",
  endpoints: [
    {
      slug: "create",
      title: "Create a customer",
      method: "POST",
      path: "/v1/customers",
      auth: "key",
      summary: "Creates a customer.",
      bodyFields: [
        { name: "name", type: "string", required: true, description: "1–255 bytes." },
        { name: "email", type: "string", description: "3–254 bytes." },
        { name: "details", type: "string", description: "≤4,000 bytes." },
      ],
      response: { description: "Customer." },
      examples: {
        curl: `curl -fsS "$API/v1/customers" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{ "name": "Globex", "email": "ap@globex.example", "details": "Attn: Accounts Payable" }'`,
        ts: `const globex = await payday.customers.create({
  name: "Globex",
  email: "ap@globex.example",
  details: "Attn: Accounts Payable",
});`,
        response: CUSTOMER,
        responseTitle: "201 Created",
      },
    },
    {
      slug: "list",
      title: "List customers",
      method: "GET",
      path: "/v1/customers",
      auth: "key",
      summary: "Lists customers, newest first.",
      query: [
        { name: "limit", type: "integer", description: "1–100. Default 20." },
        { name: "starting_after", type: "cus_ id", description: "Cursor from next_cursor." },
      ],
      response: { description: "{ customers: Customer[], next_cursor: cus_ id | null }." },
      examples: {
        curl: `curl -fsS "$API/v1/customers?limit=50" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const page = await payday.customers.list({ limit: 50 });`,
        response: `{
  "customers": [ ${CUSTOMER.replace(/\n/g, "\n    ")} ],
  "next_cursor": null
}`,
      },
    },
    {
      slug: "get",
      title: "Get a customer",
      method: "GET",
      path: "/v1/customers/{id}",
      auth: "key",
      summary: "Retrieves a customer with aggregate statistics.",
      pathParams: [{ name: "id", type: "cus_ id", required: true, description: "" }],
      response: {
        description: (
          <>
            <code>Customer</code> plus <code>stats</code>: <code>request_count</code>,{" "}
            <code>collected_base_units</code>, <code>pending_base_units</code>. This route only.
          </>
        ),
      },
      answers: [{ status: 404, code: "customer_not_found", when: "" }],
      examples: {
        curl: `curl -fsS "$API/v1/customers/cus_0198f80c-1111-7dc1-a369-90556a64f700" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const customer = await payday.customers.get(id);`,
        response: CUSTOMER.replace(
          '"updated_at": "2026-09-06T12:00:00Z"',
          '"updated_at": "2026-09-06T12:00:00Z",\n  "stats": { "request_count": 3, "collected_base_units": "2500000000", "pending_base_units": "1250000000" }',
        ),
      },
    },
    {
      slug: "update",
      title: "Update a customer",
      method: "PATCH",
      path: "/v1/customers/{id}",
      auth: "key",
      summary: "Updates a customer. Partial.",
      body: (
        <p>
          Omitted fields are unchanged. <code>null</code> clears <code>email</code> or{" "}
          <code>details</code>. The merged record is validated whole. Issued requests are
          unaffected.
        </p>
      ),
      pathParams: [{ name: "id", type: "cus_ id", required: true, description: "" }],
      bodyFields: [
        { name: "name", type: "string", description: "1–255 bytes." },
        { name: "email", type: "string | null", description: "" },
        { name: "details", type: "string | null", description: "" },
      ],
      response: { description: "Customer." },
      examples: {
        curl: `curl -fsS -X PATCH "$API/v1/customers/cus_0198f80c-…" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{ "email": null }'`,
        ts: `await payday.customers.update(id, { email: null });`,
        response: CUSTOMER.replace('"email": "ap@globex.example"', '"email": null'),
      },
    },
  ],
};
