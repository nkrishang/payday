import type { Metadata } from "next";
import { PAYER, PAYER_LOCKED_EXAMPLE } from "@/components/docs/api/payer";
import { CodeBlock } from "@/components/docs/code";
import { DisclosureTable } from "@/components/docs/figures";
import { DocsPage, Figure, H2 } from "@/components/docs/prose";
import { RouteList } from "@/components/docs/route-list";

export const metadata: Metadata = {
  title: "The payer view",
  description:
    "The unauthenticated routes behind a deposit link: disclosure rules, sessions, and CORS.",
};

export default function PayerOverviewPage() {
  return (
    <DocsPage
      eyebrow="Payer routes"
      title="The payer view"
      lead="Unauthenticated. No API key accepted. No merchant data returned. A payer session in Payday-Payer-Session unlocks gated content for exactly one request."
    >
      <H2 id="sessions">Sessions</H2>
      <ul>
        <li>Opaque token; 24-hour validity; stored hashed; bound to one deposit request.</li>
        <li>
          Minted by: email verification start, client-secret exchange, wallet challenge
          (permissionless only), or the merchant preview route.
        </li>
        <li>Header transport only. Never in a path or query.</li>
      </ul>

      <H2 id="disclosure">Disclosure</H2>
      <Figure caption="Field availability by state. Permissionless requests begin in the second column.">
        <DisclosureTable />
      </Figure>
      <p>
        Never present: <code>payout_address</code>, <code>recovery_address</code>,{" "}
        <code>metadata</code>, <code>customer_id</code>, <code>issuer_id</code>, policy assertions.{" "}
        <code>expected_email</code> is masked (<code>a****@c***.example</code>); absent under{" "}
        <code>merchant_session</code>. Gated fields are <code>null</code>, not omitted.
      </p>
      <CodeBlock code={PAYER_LOCKED_EXAMPLE} lang="json" title="Locked" />

      <H2 id="cors">CORS</H2>
      <ul>
        <li>
          Reads (<code>GET</code>): any origin. <code>Payday-Payer-Session</code> allowed.
        </li>
        <li>
          Writes (verification, session exchange, wallet): hosted checkout origin only. Body limit 8
          KiB.
        </li>
        <li>
          JSON responses: <code>Cache-Control: no-store</code>.
        </li>
      </ul>

      <H2 id="routes">Routes</H2>
      <RouteList groups={[PAYER]} />
    </DocsPage>
  );
}
