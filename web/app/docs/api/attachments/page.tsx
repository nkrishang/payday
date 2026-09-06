import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { CodeTabs } from "@/components/docs/code-tabs";
import { Sequence } from "@/components/docs/diagrams";
import { Answers, Endpoint, Params } from "@/components/docs/endpoint";
import { Callout, DocsPage, Figure } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Attachments API",
  description:
    "Attach one PDF to a deposit request: reserve an upload, send the bytes straight to storage, finalize once the malware scan admits it.",
};

const SLOT = `{
  "id": "att_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "upload_url": "https://…",
  "headers": { "Content-Type": "application/pdf", "If-None-Match": "*", "…": "…" },
  "expires_at": "2026-09-06T12:15:00Z"
}`;

const DESCRIPTOR = `{
  "id": "att_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "filename": "INV-1042.pdf",
  "mime_type": "application/pdf",
  "byte_length": "48211",
  "sha256": "0x9f…"
}`;

const TS = `import { readFile } from "node:fs/promises";

// The whole exchange: reserve, PUT, poll finalize with backoff.
const pdf = await payday.attachments.upload(await readFile("INV-1042.pdf"), "INV-1042.pdf");
await payday.depositRequests.create({ ...fields, attachment_id: pdf.id }, "INV-1042");

// Or the pieces:
const slot = await payday.attachments.create({ filename: "INV-1042.pdf" });
await fetch(slot.upload_url, { method: "PUT", headers: slot.headers, body: bytes });
const descriptor = await payday.attachments.finalize(slot.id); // throws attachment_scan_pending while scanning`;

const CURL = `SLOT=$(curl -fsS "$API/v1/attachments" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" -H "Content-Type: application/json" \\
  -d '{ "filename": "INV-1042.pdf" }')

# PUT the bytes with exactly the headers returned in the slot.
curl -fsS -X PUT "$(echo "$SLOT" | jq -r .upload_url)" \\
  $(echo "$SLOT" | jq -r '.headers | to_entries[] | "-H \\"\\(.key): \\(.value)\\""' | xargs) \\
  --data-binary @INV-1042.pdf

# Finalize; repeat with backoff while it answers 409 attachment_scan_pending.
curl -fsS -X POST "$API/v1/attachments/$(echo "$SLOT" | jq -r .id)/finalize" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`;

export default function AttachmentsApiPage() {
  return (
    <DocsPage
      eyebrow="API reference"
      title="Attachments"
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

      <CodeTabs
        tabs={[
          { label: "TypeScript", content: <CodeBlock code={TS} lang="ts" /> },
          { label: "curl", content: <CodeBlock code={CURL} lang="bash" /> },
        ]}
      />

      <Endpoint method="POST" path="/v1/attachments">
        Reserve an upload slot.
      </Endpoint>
      <Params
        title="Body"
        rows={[
          {
            name: "filename",
            type: "string",
            required: true,
            description: "Shown to the payer and on the request.",
          },
        ]}
      />
      <CodeBlock code={SLOT} lang="json" title="201 Created" />
      <p>
        <code>PUT</code> the bytes to <code>upload_url</code> sending exactly the returned{" "}
        <code>headers</code>. They include <code>If-None-Match: *</code>, which makes the object
        write-once: a second <code>PUT</code> fails, so the bytes Payday hashes cannot be swapped
        afterwards. The slot expires; reserve another if it has.
      </p>

      <Endpoint method="POST" path="/v1/attachments/{id}/finalize">
        Ask Payday to admit the uploaded bytes.
      </Endpoint>
      <CodeBlock code={DESCRIPTOR} lang="json" title="200 OK" />
      <p>
        Payday hashes the stored bytes itself and pins the object version it admitted; browser MIME
        types, extensions, and client digests are never trusted. Finalize is idempotent: a finalized
        or attached upload answers with its descriptor again.
      </p>
      <Answers
        rows={[
          { status: 200, when: "A clean PDF within limits. Pass id as attachment_id." },
          {
            status: 409,
            code: "attachment_scan_pending",
            when: "The malware scan has not reported. Retry with backoff.",
          },
          {
            status: 409,
            code: "attachment_not_ready",
            when: "Nothing has reached upload_url yet.",
          },
          {
            status: 422,
            code: "attachment_rejected",
            when: "Not application/pdf, outside 1 byte to 5 MiB, missing the PDF header, or flagged. The object is deleted; upload a new file.",
          },
        ]}
      />

      <Callout title="Lifetime">
        An unattached upload is deleted after seven days. Issuing with an id that expired unused
        fails with <code>409 attachment_not_ready</code> and a message asking for the PDF again. An
        attachment belongs to one request; reusing its id is{" "}
        <code>409 attachment_already_attached</code>.
      </Callout>
      <p>
        Once attached, the payer downloads it from the checkout after verification, you read it from{" "}
        <code>GET /v1/deposit-requests/&#123;id&#125;/attachment</code> with a short-lived signed
        URL, and its SHA-256 appears in the Proof of Payment.
      </p>
    </DocsPage>
  );
}
