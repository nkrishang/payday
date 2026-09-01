import { type AttachmentDescriptor, PaydayError } from "@payday/sdk";
import { createMerchantClient } from "./merchant-payday";

/** The API's limit; checked here only so the form can answer before a wasted upload. */
export const MAX_ATTACHMENT_BYTES = 5 * 1024 * 1024;

/**
 * Where one file is in the exchange. `pending_upload` and `scanning` are the
 * attachment's own statuses at the API; `ready` carries the descriptor whose
 * id goes on the invoice; `rejected` is the API's verdict and `failed` is
 * anything else (network, expired slot, scan timeout).
 */
export type UploadState =
  | { status: "idle" }
  | { status: "pending_upload"; filename: string }
  | { status: "scanning"; filename: string }
  | { status: "ready"; attachment: AttachmentDescriptor }
  | { status: "rejected"; filename: string; message: string }
  | { status: "failed"; filename: string; message: string };

/**
 * Immediate feedback before any request is made. The API is the validator —
 * it hashes and sniffs the stored bytes itself — so this only saves a round
 * trip for the obvious cases.
 */
export function fileProblem(file: { name: string; size: number; type: string }): string | null {
  const pdf = file.type === "application/pdf" || /\.pdf$/i.test(file.name);
  if (!pdf) return "Only a PDF can be attached.";
  if (file.size === 0) return "The file is empty.";
  if (file.size > MAX_ATTACHMENT_BYTES) return "The file is larger than 5 MiB.";
  return null;
}

/**
 * Runs the SDK's upload — reserve a slot, PUT the bytes with the presigned
 * headers, poll finalize until the scan reports — and narrates it through
 * `onState`. The SDK has no progress hook, so the client is built with a
 * fetch that notices the presigned PUT completing: that is the moment the
 * bytes are with the scanner and the wait becomes the scan, not the upload.
 */
export async function uploadAttachment(
  accessToken: string,
  file: File,
  onState: (state: UploadState) => void,
  signal?: AbortSignal,
): Promise<AttachmentDescriptor | null> {
  const filename = file.name;
  const client = createMerchantClient(accessToken, async (input, init) => {
    const response = await globalThis.fetch(input, init);
    if (init?.method === "PUT" && response.ok) onState({ status: "scanning", filename });
    return response;
  });

  onState({ status: "pending_upload", filename });
  try {
    const attachment = await client.attachments.upload(
      file,
      filename,
      signal === undefined ? {} : { signal },
    );
    onState({ status: "ready", attachment });
    return attachment;
  } catch (error) {
    if (signal?.aborted) {
      onState({ status: "idle" });
      return null;
    }
    if (error instanceof PaydayError && error.code === "attachment_rejected") {
      onState({ status: "rejected", filename, message: error.message });
    } else {
      onState({ status: "failed", filename, message: describeError(error) });
    }
    return null;
  }
}

/** A sentence for the merchant; never the raw object. */
export function describeError(error: unknown): string {
  if (error instanceof PaydayError) {
    return error.requestId ? `${error.message} (request ${error.requestId})` : error.message;
  }
  if (error instanceof Error && error.message) return error.message;
  return "Something went wrong. Try again.";
}
