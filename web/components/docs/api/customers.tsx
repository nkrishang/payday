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
      summary:
        "A reusable payer record: a name, an optional email, optional details. Name it on a deposit request and it supplies the payer party.",
      bodyFields: [
        { name: "name", type: "string", required: true, description: "1 to 255 bytes." },
        {
          name: "email",
          type: "string",
          description: "3 to 254 bytes. Where Payday emails a request issued to this customer.",
        },
        {
          name: "details",
          type: "string",
          description: "Up to 4,000 bytes of free text, shown verbatim on the request.",
        },
      ],
      response: { description: "The customer." },
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
      summary: "Customers newest first, with a cursor.",
      query: [
        { name: "limit", type: "integer", description: "1 to 100, default 20." },
        {
          name: "starting_after",
          type: "cus_ id",
          description: "The previous page's next_cursor.",
        },
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
      summary:
        "One customer, with how much its requests have collected and how much is still open.",
      pathParams: [{ name: "id", type: "cus_ id", required: true, description: "The customer." }],
      response: {
        description: (
          <>
            The customer plus <code>stats</code>, in base units: <code>request_count</code>,{" "}
            <code>collected_base_units</code> (settled across its requests), and{" "}
            <code>pending_base_units</code> (still open). Only the single read carries stats.
          </>
        ),
      },
      answers: [
        {
          status: 404,
          code: "customer_not_found",
          when: "Missing, malformed, or another account's.",
        },
      ],
      examples: {
        curl: `curl -fsS "$API/v1/customers/cus_0198f80c-1111-7dc1-a369-90556a64f700" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const customer = await payday.customers.get(id);
customer.stats.pending_base_units; // "1250000000"`,
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
      summary:
        "Change any subset of name, email, and details. A field left out keeps its value; null clears email or details.",
      body: (
        <p>
          The merged record is validated whole, so a blank name is refused. Editing a customer never
          changes a request already issued.
        </p>
      ),
      pathParams: [{ name: "id", type: "cus_ id", required: true, description: "The customer." }],
      bodyFields: [
        { name: "name", type: "string", description: "1 to 255 bytes." },
        { name: "email", type: "string | null", description: "A new address, or null to clear." },
        { name: "details", type: "string | null", description: "New details, or null to clear." },
      ],
      response: { description: "The updated customer." },
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
