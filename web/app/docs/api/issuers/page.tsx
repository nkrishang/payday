import type { Metadata } from "next";
import { ISSUERS, PAYOUT_ADDRESSES } from "@/components/docs/api/issuers";
import { CodeBlock } from "@/components/docs/code";
import { Params } from "@/components/docs/endpoint";
import { DocsPage, H2 } from "@/components/docs/prose";
import { RouteList } from "@/components/docs/route-list";

export const metadata: Metadata = {
  title: "Issuer identities",
  description: "The Issuer and PayoutAddress objects and the routes that operate on them.",
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

export default function IssuersOverviewPage() {
  return (
    <DocsPage
      eyebrow="Issuers"
      title="Issuer identities"
      lead="A saved issuing party with a verified contact mailbox and an ordered set of payout addresses. Referenced from a deposit request by issuer_id; the request snapshots the party at issuance."
    >
      <H2 id="issuer">Issuer</H2>
      <Params
        rows={[
          { name: "id", type: "iss_ id", description: "" },
          {
            name: "name",
            type: "string",
            description: "Unique per account, case- and whitespace-insensitive.",
          },
          { name: "contact_email", type: "string", description: "Lowercased." },
          { name: "details", type: "string | null", description: "" },
          {
            name: "email_verified, email_verified_at",
            type: "boolean, timestamp | null",
            description: "Set by the confirm route. Reset by a contact_email change.",
          },
          {
            name: "payout_addresses",
            type: "PayoutAddress[]",
            description: "Association order. First is the default for issuer_id-only creates.",
          },
          { name: "created_at, updated_at", type: "timestamp", description: "" },
        ]}
      />
      <CodeBlock code={ISSUER} lang="json" title="Issuer" />

      <H2 id="payout-address">PayoutAddress</H2>
      <Params
        rows={[
          { name: "id", type: "pa_ id", description: "" },
          { name: "address", type: "string", description: "EIP-55. Unique per account." },
          { name: "label", type: "string | null", description: "≤20 characters." },
          { name: "created_at", type: "timestamp", description: "" },
        ]}
      />

      <H2 id="constraints">Constraints</H2>
      <ul>
        <li>A request cannot carry an identity whose mailbox is unverified.</li>
        <li>Editing an identity does not touch issued requests.</li>
        <li>An identity with issued requests cannot be deleted.</li>
        <li>Identities with no payout addresses settle to the account wallet.</li>
      </ul>

      <H2 id="routes">Routes</H2>
      <RouteList groups={[ISSUERS, PAYOUT_ADDRESSES]} />
    </DocsPage>
  );
}
