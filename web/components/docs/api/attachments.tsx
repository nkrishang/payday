import type { EndpointGroup } from "./types";

export const ATTACHMENTS: EndpointGroup = {
  slug: "attachments",
  title: "Attachments",
  overview: { title: "The upload flow" },
  endpoints: [
    {
      slug: "create",
      title: "Reserve an upload",
      method: "POST",
      path: "/v1/attachments",
      auth: "key",
      summary: "Reserves a presigned upload slot for one PDF.",
      body: (
        <p>
          <code>PUT</code> the bytes to <code>upload_url</code> with <code>headers</code> verbatim.
          The headers include <code>If-None-Match: *</code>: the object is write-once. The API never
          receives the bytes. Slots expire; reserve again.
        </p>
      ),
      bodyFields: [{ name: "filename", type: "string", required: true, description: "" }],
      response: {
        fields: [
          { name: "id", type: "att_ id", description: "" },
          { name: "upload_url", type: "string", description: "Presigned PUT target." },
          { name: "headers", type: "object", description: "Send verbatim." },
          { name: "expires_at", type: "timestamp", description: "" },
        ],
      },
      examples: {
        curl: `SLOT=$(curl -fsS "$API/v1/attachments" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" -H "Content-Type: application/json" \\
  -d '{ "filename": "INV-1042.pdf" }')

curl -fsS -X PUT "$(echo "$SLOT" | jq -r .upload_url)" \\
  $(echo "$SLOT" | jq -r '.headers | to_entries[] | "-H \\"\\(.key): \\(.value)\\""' | xargs) \\
  --data-binary @INV-1042.pdf`,
        ts: `const slot = await payday.attachments.create({ filename: "INV-1042.pdf" });
await fetch(slot.upload_url, { method: "PUT", headers: slot.headers, body: bytes });

// Reserve + PUT + finalize with backoff:
const pdf = await payday.attachments.upload(bytes, "INV-1042.pdf");`,
        response: `{
  "id": "att_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "upload_url": "https://…",
  "headers": { "Content-Type": "application/pdf", "If-None-Match": "*", "…": "…" },
  "expires_at": "2026-09-06T12:15:00Z"
}`,
        responseTitle: "201 Created",
      },
    },
    {
      slug: "finalize",
      title: "Finalize an upload",
      method: "POST",
      path: "/v1/attachments/{id}/finalize",
      auth: "key",
      summary: "Admits the uploaded object once scanned. Idempotent.",
      body: (
        <p>
          Payday hashes the stored bytes and pins the admitted object version. Client-supplied MIME
          types, extensions, and digests are ignored. Rejected objects are deleted. Unattached
          uploads are deleted after seven days.
        </p>
      ),
      pathParams: [{ name: "id", type: "att_ id", required: true, description: "" }],
      response: {
        fields: [
          { name: "id", type: "att_ id", description: "Use as attachment_id." },
          {
            name: "filename, mime_type",
            type: "string",
            description: "mime_type is application/pdf.",
          },
          { name: "byte_length", type: "string", description: "Decimal. 1 to 5,242,880." },
          { name: "sha256", type: "string", description: "0x hex." },
        ],
      },
      answers: [
        { status: 200, when: "" },
        {
          status: 409,
          code: "attachment_scan_pending",
          when: "Scan not reported. Retry with backoff.",
        },
        { status: 409, code: "attachment_not_ready", when: "No object at upload_url." },
        {
          status: 422,
          code: "attachment_rejected",
          when: "Not application/pdf, outside 1 B–5 MiB, missing %PDF- header, or flagged. Object deleted.",
        },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/attachments/att_0198f80c-…/finalize" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const descriptor = await payday.attachments.finalize(slot.id);`,
        response: `{
  "id": "att_0198f80c-8d2f-7dc1-a369-90556a64f700",
  "filename": "INV-1042.pdf",
  "mime_type": "application/pdf",
  "byte_length": "48211",
  "sha256": "0x9f…"
}`,
      },
    },
  ],
};
