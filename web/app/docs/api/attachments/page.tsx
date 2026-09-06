import Link from "next/link";
import type { Metadata } from "next";
import { ATTACHMENTS } from "@/components/docs/api/attachments";
import { CodeBlock } from "@/components/docs/code";
import { Sequence } from "@/components/docs/diagrams";
import { MethodBadge } from "@/components/docs/endpoint";
import { Callout, DocsPage, Figure, H2 } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "The upload flow",
  description:
    "Attach one PDF to a deposit request: reserve an upload, send the bytes straight to storage, finalize once the malware scan admits it.",
};

const TS = `import { readFile } from "node:fs/promises";

// The whole exchange: reserve, PUT, poll finalize with backoff.
const pdf = await payday.attachments.upload(await readFile("INV-1042.pdf"), "INV-1042.pdf");
await payday.depositRequests.create({ ...fields, attachment_id: pdf.id }, "INV-1042");`;

export default function AttachmentsOverviewPage() {
  return (
    <DocsPage
      eyebrow="Attachments"
      title="The upload flow"
      lead="One PDF per deposit request, at most 5 MiB. The bytes go straight to object storage through a presigned URL, are scanned before they can be attached, and their hash is committed into the deposit address."
    >
      <Figure caption="The upload exchange. The API never receives the bytes; it hashes what storage holds, and the write-once headers make sure that is what you sent.">
        <Sequence
          label="Attachment upload sequence: your server reserves a slot, puts the bytes to storage with the presigned headers, the scanner reports, finalize returns the descriptor, and the id goes on the deposit request."
          lanes={["Your server", "Payday API", "Object storage"]}
          messages={[
            { from: 0, to: 1, label: "POST /v1/attachments { filename }" },
            { from: 1, to: 0, label: "201 { id, upload_url, headers }", reply: true },
            { from: 0, to: 2, label: "PUT bytes with the returned headers", accent: "yellow" },
            { from: 2, to: 1, label: "scan result", reply: true, note: "clean, or flagged" },
            { from: 0, to: 1, label: "POST /v1/attachments/{id}/finalize" },
            {
              from: 1,
              to: 0,
              label: "200 descriptor  ·  409 while scanning",
              reply: true,
              accent: "green",
            },
            { from: 0, to: 1, label: "POST /v1/deposit-requests { attachment_id }" },
          ]}
        />
      </Figure>
      <p>The SDK runs the whole exchange in one call:</p>
      <CodeBlock code={TS} lang="ts" />

      <Callout title="Lifetime">
        An unattached upload is deleted after seven days. Issuing with an id that expired unused
        fails with <code>409 attachment_not_ready</code> and a message asking for the PDF again. An
        attachment belongs to one request; reusing its id is{" "}
        <code>409 attachment_already_attached</code>.
      </Callout>
      <p>
        Once attached, the payer downloads it from the checkout after verification, you read it from{" "}
        <Link href="/docs/api/deposit-requests/attachment">
          the request&apos;s attachment route
        </Link>{" "}
        with a short-lived signed URL, and its SHA-256 appears in the Proof of Payment.
      </p>

      <H2 id="routes">Routes</H2>
      <ul>
        {ATTACHMENTS.endpoints.map((endpoint) => (
          <li key={endpoint.slug} className="flex items-center gap-2.5">
            <MethodBadge method={endpoint.method} variant="tint" />
            <a href={`/docs/api/attachments/${endpoint.slug}`}>{endpoint.title}</a>
            <span className="hidden font-mono text-[12px] text-faint sm:inline">
              {endpoint.path}
            </span>
          </li>
        ))}
      </ul>
    </DocsPage>
  );
}
