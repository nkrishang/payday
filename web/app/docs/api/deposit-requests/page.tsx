import Link from "next/link";
import type { Metadata } from "next";
import {
  DEPOSIT_REQUESTS,
  DEPOSIT_REQUEST_EXAMPLE,
  DEPOSIT_REQUEST_FIELDS,
} from "@/components/docs/api/deposit-requests";
import { CodeBlock } from "@/components/docs/code";
import { MethodBadge, Params } from "@/components/docs/endpoint";
import { Callout, DocsPage, H2 } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "The deposit request object",
  description:
    "Every field of a deposit request, and the routes that create, read, and act on one.",
};

export default function DepositRequestObjectPage() {
  return (
    <DocsPage
      eyebrow="Deposit requests"
      title="The deposit request object"
      lead="The central resource. A deposit request is the document you issue; the state of its deposit is read from it. Every route in this group returns or acts on this object."
    >
      <Callout title="The address comes later">
        <code>address</code>, <code>payer_wallet</code>, <code>recovery_address</code>,{" "}
        <code>wallet_bound_at</code>, and <code>self_settlement</code> are <code>null</code> from
        creation until the payer signs the wallet attestation on the hosted page. Wait for{" "}
        <code>deposit_request.ready</code>, or poll until <code>address</code> is set, before
        quoting an address anywhere.
      </Callout>

      <H2 id="fields">Fields</H2>
      <Params rows={DEPOSIT_REQUEST_FIELDS} />
      <p>
        Statuses are <code>awaiting_deposit</code>, <code>partially_deposited</code>,{" "}
        <code>deposited</code>, <code>settled</code>, <code>expired</code>, <code>returned</code>,
        and <code>needs_attention</code>; see the{" "}
        <Link href="/docs/concepts#lifecycle">lifecycle</Link>. Clients must tolerate new fields and
        branch only on documented values.
      </p>

      <H2 id="example">Example</H2>
      <CodeBlock
        code={DEPOSIT_REQUEST_EXAMPLE}
        lang="json"
        title="A deposit request, just issued"
      />

      <H2 id="routes">Routes</H2>
      <ul>
        {DEPOSIT_REQUESTS.endpoints.map((endpoint) => (
          <li key={endpoint.slug} className="flex items-center gap-2.5">
            <MethodBadge method={endpoint.method} variant="tint" />
            <a href={`/docs/api/deposit-requests/${endpoint.slug}`}>{endpoint.title}</a>
            <span className="hidden font-mono text-[12px] text-faint sm:inline">
              {endpoint.path}
            </span>
          </li>
        ))}
      </ul>
    </DocsPage>
  );
}
