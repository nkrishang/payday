import type { Metadata } from "next";
import { ATTACHMENTS } from "@/components/docs/api/attachments";
import { CodeBlock } from "@/components/docs/code";
import { Sequence } from "@/components/docs/diagrams";
import { DocsPage, Figure, H2 } from "@/components/docs/prose";
import { RouteList } from "@/components/docs/route-list";

export const metadata: Metadata = {
  title: "The upload flow",
  description: "Presigned, write-once, scanned PDF upload. One attachment per deposit request.",
};

const TS = `const pdf = await payday.attachments.upload(bytes, "INV-1042.pdf"); // reserve + PUT + finalize
await payday.depositRequests.create({ ...fields, attachment_id: pdf.id }, "INV-1042");`;

export default function AttachmentsOverviewPage() {
  return (
    <DocsPage
      eyebrow="Attachments"
      title="The upload flow"
      lead="One PDF per deposit request, ≤5 MiB. Bytes go to object storage through a presigned, write-once PUT; the object is scanned, hashed by Payday, and admitted by finalize. The hash is committed into the deposit address."
    >
      <Figure caption="Reserve, PUT, finalize, attach. The API never receives the bytes.">
        <Sequence
          label="Attachment upload sequence: reserve a slot, PUT the bytes to storage with the presigned headers, the scanner reports, finalize returns the descriptor, the id goes on the deposit request."
          lanes={["Client", "Payday API", "Object storage"]}
          messages={[
            { from: 0, to: 1, label: "POST /v1/attachments { filename }" },
            { from: 1, to: 0, label: "201 { id, upload_url, headers }", reply: true },
            { from: 0, to: 2, label: "PUT bytes, headers verbatim", accent: "yellow" },
            { from: 2, to: 1, label: "scan result", reply: true },
            { from: 0, to: 1, label: "POST /v1/attachments/{id}/finalize" },
            {
              from: 1,
              to: 0,
              label: "200 descriptor · 409 attachment_scan_pending",
              reply: true,
              accent: "green",
            },
            { from: 0, to: 1, label: "POST /v1/deposit-requests { attachment_id }" },
          ]}
        />
      </Figure>
      <CodeBlock code={TS} lang="ts" title="SDK" />

      <H2 id="lifecycle">Lifecycle</H2>
      <ul>
        <li>Unattached uploads are deleted after seven days.</li>
        <li>
          An expired or rejected id on create: <code>409 attachment_not_ready</code>.
        </li>
        <li>
          One request per attachment: <code>409 attachment_already_attached</code>.
        </li>
        <li>
          Once attached: readable by the merchant at{" "}
          <code>GET /v1/deposit-requests/{`{id}`}/attachment</code>, by an unlocked payer at the
          payer route, and hashed into the Proof of Payment.
        </li>
      </ul>

      <H2 id="routes">Routes</H2>
      <RouteList groups={[ATTACHMENTS]} />
    </DocsPage>
  );
}
