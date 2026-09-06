import type { Metadata } from "next";
import { ISSUERS, PAYOUT_ADDRESSES } from "@/components/docs/api/issuers";
import { CodeBlock } from "@/components/docs/code";
import { MethodBadge } from "@/components/docs/endpoint";
import { Callout, DocsPage, H2, Step, Steps } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Issuer identities",
  description:
    "Save the party you issue under, prove its contact mailbox, and keep the wallets you settle to.",
};

const ISSUER = `{
  "id": "iss_0198f80c-2222-7dc1-a369-90556a64f700",
  "name": "Acme LLC",
  "contact_email": "billing@acme.example",
  "details": "Acme LLC, 1 Market St, Springfield",
  "email_verified": true,
  "email_verified_at": "2026-09-06T12:05:00Z",
  "payout_addresses": [
    { "id": "pa_0198f80c-…", "address": "0x1111111111111111111111111111111111111111", "label": "Treasury", "created_at": "2026-09-06T12:00:00Z" }
  ],
  "created_at": "2026-09-06T12:00:00Z",
  "updated_at": "2026-09-06T12:05:00Z"
}`;

const SETUP = `const acme = await payday.issuers.create({ name: "Acme LLC", contact_email: "billing@acme.example" });

await payday.issuers.startEmailVerification(acme.id);   // a code goes to billing@acme.example
await payday.issuers.confirmEmailVerification(acme.id, "123456");

const treasury = await payday.payoutAddresses.create({ address: "0x1111…", label: "Treasury" });
await payday.issuers.setPayoutAddresses(acme.id, [treasury.id]);`;

export default function IssuersOverviewPage() {
  return (
    <DocsPage
      eyebrow="Issuers"
      title="Issuer identities"
      lead="An issuer identity is your own side of a deposit request, saved once instead of retyped: the party it is issued under, the mailbox payers write to, and the wallets it settles to."
    >
      <p>
        Issuance is unchanged by any of this. A request still carries its own <code>issuer</code>{" "}
        and <code>payout_address</code> snapshot, so editing an identity never reaches a request
        already issued. What the identity does is stop the retyping, prove the mailbox, and leave a
        durable <code>issuer_id</code> on every request issued under it, so &quot;issued under
        Acme&quot; stays answerable after a rename.
      </p>
      <CodeBlock code={ISSUER} lang="json" title="The issuer object" />

      <H2 id="setting-one-up">Setting one up</H2>
      <Steps>
        <Step title="Create it">
          A name and a contact address. Names are unique among your identities, ignoring case and
          surrounding space; contact addresses may repeat.
        </Step>
        <Step title="Prove the mailbox">
          Start verification and a six-digit code goes to the stored address; confirm it. Payers are
          told to write there, so a request cannot carry an unproven mailbox.
        </Step>
        <Step title="Save the wallets it settles to (optional)">
          Payout addresses are account-owned and shared between identities. An identity with none
          settles to the account&apos;s own Payday wallet.
        </Step>
      </Steps>
      <CodeBlock code={SETUP} lang="ts" />

      <Callout title="Where a request settles">
        <code>POST /v1/deposit-requests</code> with an <code>issuer_id</code> and no{" "}
        <code>payout_address</code> settles to the identity&apos;s first saved wallet. An explicit{" "}
        <code>payout_address</code> always wins. Before accepting deposits, confirm your
        organisation controls whichever wallet that is.
      </Callout>

      <H2 id="routes">Routes</H2>
      <ul>
        {[
          ...ISSUERS.endpoints.map((e) => [ISSUERS.slug, e] as const),
          ...PAYOUT_ADDRESSES.endpoints.map((e) => [PAYOUT_ADDRESSES.slug, e] as const),
        ].map(([group, endpoint]) => (
          <li key={`${group}/${endpoint.slug}`} className="flex items-center gap-2.5">
            <MethodBadge method={endpoint.method} variant="tint" />
            <a href={`/docs/api/${group}/${endpoint.slug}`}>{endpoint.title}</a>
            <span className="hidden font-mono text-[12px] text-faint sm:inline">
              {endpoint.path}
            </span>
          </li>
        ))}
      </ul>
    </DocsPage>
  );
}
