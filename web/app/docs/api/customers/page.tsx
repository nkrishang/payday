import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { CodeTabs } from "@/components/docs/code-tabs";
import { Answers, Endpoint, Params } from "@/components/docs/endpoint";
import { DocsPage } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Customers API",
  description:
    "Reusable payer records that supply a deposit request's payer party and let you filter by who owes you.",
};

const CREATE_CURL = `curl -fsS "$API/v1/customers" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{ "name": "Globex", "email": "ap@globex.example", "details": "Attn: Accounts Payable" }'`;

const CREATE_TS = `const globex = await payday.customers.create({
  name: "Globex",
  email: "ap@globex.example",
  details: "Attn: Accounts Payable",
});`;

const CUSTOMER = `{
  "id": "cus_0198f80c-1111-7dc1-a369-90556a64f700",
  "name": "Globex",
  "email": "ap@globex.example",
  "details": "Attn: Accounts Payable",
  "created_at": "2026-09-06T12:00:00Z",
  "updated_at": "2026-09-06T12:00:00Z"
}`;

const DETAIL = `{
  "id": "cus_0198f80c-1111-7dc1-a369-90556a64f700",
  "name": "Globex",
  "email": "ap@globex.example",
  "details": "Attn: Accounts Payable",
  "created_at": "2026-09-06T12:00:00Z",
  "updated_at": "2026-09-06T12:00:00Z",
  "stats": { "request_count": 3, "collected_base_units": "2500000000", "pending_base_units": "1250000000" }
}`;

const PATCH = `PATCH /v1/customers/cus_0198f80c-…
{ "email": null }             # clears the email; name and details are untouched`;

export default function CustomersApiPage() {
  return (
    <DocsPage
      eyebrow="API reference"
      title="Customers"
      lead="A customer is a reusable counterparty record: a name, an optional email, optional details. Name one on a deposit request and it supplies the payer party; the request still stores its own snapshot."
    >
      <Endpoint method="POST" path="/v1/customers">
        Create a customer.
      </Endpoint>
      <Params
        title="Body"
        rows={[
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
        ]}
      />
      <CodeTabs
        tabs={[
          { label: "curl", content: <CodeBlock code={CREATE_CURL} lang="bash" /> },
          { label: "TypeScript", content: <CodeBlock code={CREATE_TS} lang="ts" /> },
        ]}
      />
      <CodeBlock code={CUSTOMER} lang="json" title="201 Created" />

      <Endpoint method="GET" path="/v1/customers">
        List customers as <code>{`{ customers, next_cursor }`}</code>.
      </Endpoint>
      <Params
        title="Query"
        rows={[
          { name: "limit", type: "integer", description: "1 to 100, default 20." },
          {
            name: "starting_after",
            type: "cus_ id",
            description: "The previous page's next_cursor.",
          },
        ]}
      />

      <Endpoint method="GET" path="/v1/customers/{id}">
        Read one customer, with what it owes and has paid.
      </Endpoint>
      <CodeBlock code={DETAIL} lang="json" title="200 OK" />
      <p>
        <code>stats</code> is in base units: <code>collected_base_units</code> is what settled
        across the customer&apos;s requests, <code>pending_base_units</code> what is still open.
        Only the single read carries it.
      </p>
      <Answers
        rows={[
          {
            status: 404,
            code: "customer_not_found",
            when: "Missing, malformed, or another account's.",
          },
        ]}
      />

      <Endpoint method="PATCH" path="/v1/customers/{id}">
        Update any subset of <code>name</code>, <code>email</code>, and <code>details</code>.
      </Endpoint>
      <p>
        Partial: a field left out keeps its value, and <code>null</code> clears <code>email</code>{" "}
        or <code>details</code>. The merged record is validated whole, so a blank name is refused.
        Editing a customer never changes a request already issued.
      </p>
      <CodeBlock code={PATCH} lang="http" />
    </DocsPage>
  );
}
