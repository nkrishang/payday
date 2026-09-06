import type { Metadata } from "next";
import { PAYER, PAYER_LOCKED_EXAMPLE } from "@/components/docs/api/payer";
import { CodeBlock } from "@/components/docs/code";
import { MethodBadge } from "@/components/docs/endpoint";
import { DisclosureTable, NeverShown } from "@/components/docs/figures";
import { Callout, DocsPage, Figure, H2 } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "The payer view",
  description:
    "The public, keyless routes behind every deposit link: what they disclose, when, and how a session unlocks them.",
};

export default function PayerOverviewPage() {
  return (
    <DocsPage
      eyebrow="Payer routes"
      title="The payer view"
      lead="What a deposit link resolves to. These routes take no API key, expose no merchant data, and are what the hosted checkout is built on, so a checkout of your own can use them too."
    >
      <p>
        Reads answer any browser origin. A payer session token, obtained by completing verification,
        exchanging a client secret, or (for a permissionless request) requesting a wallet challenge,
        travels in the <code>Payday-Payer-Session</code> header on every read and unlocks exactly
        the request it was minted for. The writes answer the hosted checkout&apos;s origin only and
        limit bodies to 8 KiB. JSON responses are never cached.
      </p>

      <Figure caption="Disclosure by moment. A permissionless request starts in the middle column: everything but the address is visible at once.">
        <DisclosureTable />
      </Figure>
      <Figure caption="Never shown to a payer, whatever the policy.">
        <NeverShown />
      </Figure>

      <H2 id="locked">A gated request, locked</H2>
      <p>
        Without a session that satisfies the policy, the gated fields are <code>null</code>: not
        hidden, absent. The hosted page renders only what it is sent, and so should yours.
      </p>
      <CodeBlock
        code={PAYER_LOCKED_EXAMPLE}
        lang="json"
        title="GET /v1/payer/deposit-requests/{id}, no session"
      />

      <Callout title="Never in a URL">
        The session token goes in the <code>Payday-Payer-Session</code> header, and a client secret
        in a URL fragment. Neither should appear in a path, a query string, or a log.
      </Callout>

      <H2 id="routes">Routes</H2>
      <ul>
        {PAYER.endpoints.map((endpoint) => (
          <li key={endpoint.slug} className="flex items-center gap-2.5">
            <MethodBadge method={endpoint.method} variant="tint" />
            <a href={`/docs/api/payer/${endpoint.slug}`}>{endpoint.title}</a>
            <span className="hidden font-mono text-[12px] text-faint sm:inline">
              {endpoint.path}
            </span>
          </li>
        ))}
      </ul>
    </DocsPage>
  );
}
