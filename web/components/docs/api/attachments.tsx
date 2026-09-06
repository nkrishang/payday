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
      summary:
        "Reserve a presigned slot for one PDF. PUT the bytes to upload_url with exactly the returned headers, then finalize.",
      body: (
        <p>
          The headers include <code>If-None-Match: *</code>, which makes the object write-once: a
          second <code>PUT</code> fails, so the bytes Payday hashes cannot be swapped afterwards.
          The slot expires; reserve another if it has. The API never receives the bytes.
        </p>
      ),
      bodyFields: [
        {
          name: "filename",
          type: "string",
          required: true,
          description: "Shown to the payer and on the request.",
        },
      ],
      response: {
        fields: [
          { name: "id", type: "att_ id", description: "The attachment." },
          { name: "upload_url", type: "string", description: "Where to PUT the bytes." },
          { name: "headers", type: "object", description: "Send these verbatim with the PUT." },
          {
            name: "expires_at",
            type: "timestamp",
            description: "When the slot stops accepting the PUT.",
          },
        ],
      },
      examples: {
        curl: `SLOT=$(curl -fsS "$API/v1/attachments" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" -H "Content-Type: application/json" \\
  -d '{ "filename": "INV-1042.pdf" }')

# PUT the bytes with exactly the returned headers.
curl -fsS -X PUT "$(echo "$SLOT" | jq -r .upload_url)" \\
  $(echo "$SLOT" | jq -r '.headers | to_entries[] | "-H \\"\\(.key): \\(.value)\\""' | xargs) \\
  --data-binary @INV-1042.pdf`,
        ts: `const slot = await payday.attachments.create({ filename: "INV-1042.pdf" });
await fetch(slot.upload_url, { method: "PUT", headers: slot.headers, body: bytes });

// Or let the SDK run the whole exchange, including the finalize polling:
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
      summary:
        "Ask Payday to admit the uploaded bytes. Answers 409 while the malware scan is running; retry with backoff. Idempotent afterwards.",
      body: (
        <p>
          Payday hashes the stored bytes itself and pins the object version it admitted; browser
          MIME types, extensions, and client digests are never trusted. A rejected object is
          deleted. An unattached upload is deleted after seven days.
        </p>
      ),
      pathParams: [{ name: "id", type: "att_ id", required: true, description: "The attachment." }],
      response: {
        fields: [
          {
            name: "id",
            type: "att_ id",
            description: "Pass as attachment_id when creating the request.",
          },
          { name: "filename, mime_type", type: "string", description: "Always application/pdf." },
          {
            name: "byte_length",
            type: "string",
            description: "Decimal byte count, 1 to 5,242,880.",
          },
          {
            name: "sha256",
            type: "string",
            description: "Hash of the stored bytes; committed into the address.",
          },
        ],
      },
      answers: [
        { status: 200, when: "A clean PDF within limits." },
        {
          status: 409,
          code: "attachment_scan_pending",
          when: "The malware scan has not reported. Retry with backoff.",
        },
        { status: 409, code: "attachment_not_ready", when: "Nothing has reached upload_url yet." },
        {
          status: 422,
          code: "attachment_rejected",
          when: "Not application/pdf, outside 1 byte to 5 MiB, missing the PDF header, or flagged. Upload a new file.",
        },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/attachments/att_0198f80c-…/finalize" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const descriptor = await payday.attachments.finalize(slot.id);
// throws PaydayError attachment_scan_pending while scanning`,
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
