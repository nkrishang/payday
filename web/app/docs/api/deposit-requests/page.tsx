import type { Metadata } from "next";
import {
  DEPOSIT_REQUEST_EXAMPLE,
  DEPOSIT_REQUEST_FIELDS,
  DEPOSIT_REQUESTS,
} from "@/components/docs/api/deposit-requests";
import { CodeBlock } from "@/components/docs/code";
import { Params } from "@/components/docs/endpoint";
import { DocsPage, H2 } from "@/components/docs/prose";
import { RouteList } from "@/components/docs/route-list";

export const metadata: Metadata = {
  title: "The deposit request object",
  description: "Fields of the DepositRequest object and the routes that operate on it.",
};

export default function DepositRequestObjectPage() {
  return (
    <DocsPage
      eyebrow="Deposit requests"
      title="The deposit request object"
      lead="The primary resource. Immutable once issued. Deposit state is read from it."
    >
      <H2 id="fields">Fields</H2>
      <Params rows={DEPOSIT_REQUEST_FIELDS} />

      <H2 id="example">Example</H2>
      <CodeBlock code={DEPOSIT_REQUEST_EXAMPLE} lang="json" title="DepositRequest" />

      <H2 id="routes">Routes</H2>
      <RouteList groups={[DEPOSIT_REQUESTS]} />
    </DocsPage>
  );
}
