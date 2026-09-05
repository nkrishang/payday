import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  fileProblem,
  MAX_ATTACHMENT_BYTES,
  uploadAttachment,
  type UploadState,
} from "./attachment-upload";

const SLOT = {
  id: "0198f80c-8d2f-7dc1-a369-90556a64f7aa",
  upload_url: "https://bucket.example.test/uploads/acct/0198f80c.pdf?X-Amz-Signature=abc",
  headers: { "content-type": "application/pdf", "x-amz-tagging": "payday-upload=pending" },
  expires_at: "2026-09-01T00:15:00Z",
};

const DESCRIPTOR = {
  id: SLOT.id,
  filename: "request.pdf",
  mime_type: "application/pdf",
  byte_length: "9",
  sha256: "0x9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
};

type Call = { url: string; init: RequestInit };

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

const apiError = (code: string, status: number) =>
  json({ error: { code, message: `stub ${code}` }, request_id: "req_1" }, status);

/** Answers the SDK's exchange in order: create, PUT, then each finalize. */
function stubExchange(finalizeAnswers: Array<() => Response>) {
  const calls: Call[] = [];
  let finalizeCount = 0;
  const fetcher = vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
    const url = String(input);
    calls.push({ url, init: init ?? {} });
    if (url.endsWith("/v1/attachments") && init?.method === "POST") return json(SLOT, 201);
    if (url === SLOT.upload_url && init?.method === "PUT")
      return new Response(null, { status: 200 });
    if (url.endsWith(`/v1/attachments/${SLOT.id}/finalize`)) {
      const answer = finalizeAnswers[finalizeCount] ?? finalizeAnswers[finalizeAnswers.length - 1];
      finalizeCount += 1;
      return answer ? answer() : json(DESCRIPTOR);
    }
    return new Response("unexpected", { status: 500 });
  });
  vi.stubGlobal("fetch", fetcher);
  return { calls, fetcher };
}

const file = () => new File(["%PDF-1.4\n"], "request.pdf", { type: "application/pdf" });

describe("uploadAttachment", () => {
  beforeEach(() => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  it("narrates pending_upload, then scanning once the bytes are stored, then ready", async () => {
    stubExchange([() => apiError("attachment_scan_pending", 409), () => json(DESCRIPTOR)]);
    const states: UploadState["status"][] = [];

    const pending = uploadAttachment("eyJ.dash.token", file(), (state) =>
      states.push(state.status),
    );
    // The first finalize answered "scan pending"; the SDK backs off a second.
    await vi.advanceTimersByTimeAsync(1_000);
    const attachment = await pending;

    expect(states).toEqual(["pending_upload", "scanning", "ready"]);
    expect(attachment).toEqual(DESCRIPTOR);
  });

  it("PUTs with the presigned headers verbatim and never the session token", async () => {
    const { calls } = stubExchange([() => json(DESCRIPTOR)]);

    await uploadAttachment("eyJ.dash.token", file(), () => {});

    const put = calls.find((call) => call.init.method === "PUT");
    expect(put?.url).toBe(SLOT.upload_url);
    expect(put?.init.headers).toEqual(SLOT.headers);
    const create = calls.find((call) => call.url.endsWith("/v1/attachments"));
    expect((create?.init.headers as Record<string, string>).Authorization).toBe(
      "Bearer eyJ.dash.token",
    );
  });

  it("reports the API's rejection as rejected, not as a failure", async () => {
    stubExchange([() => apiError("attachment_rejected", 422)]);
    const states: UploadState[] = [];

    const result = await uploadAttachment("eyJ.dash.token", file(), (state) => states.push(state));

    expect(result).toBeNull();
    expect(states.map((state) => state.status)).toEqual(["pending_upload", "scanning", "rejected"]);
    expect(states.at(-1)).toMatchObject({ status: "rejected", filename: "request.pdf" });
  });

  it("treats a refused PUT as a failure before any scan", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
        if (init?.method === "POST") return json(SLOT, 201);
        return new Response("SignatureDoesNotMatch", { status: 403 });
      }),
    );
    const states: UploadState["status"][] = [];

    const result = await uploadAttachment("eyJ.dash.token", file(), (state) =>
      states.push(state.status),
    );

    expect(result).toBeNull();
    expect(states).toEqual(["pending_upload", "failed"]);
  });
});

describe("fileProblem", () => {
  it("answers the obvious cases before any request is made", () => {
    expect(fileProblem({ name: "request.pdf", size: 10, type: "application/pdf" })).toBeNull();
    expect(fileProblem({ name: "INVOICE.PDF", size: 10, type: "" })).toBeNull();
    expect(fileProblem({ name: "photo.png", size: 10, type: "image/png" })).toMatch(/PDF/);
    expect(fileProblem({ name: "empty.pdf", size: 0, type: "application/pdf" })).toMatch(/empty/);
    expect(
      fileProblem({ name: "big.pdf", size: MAX_ATTACHMENT_BYTES + 1, type: "application/pdf" }),
    ).toMatch(/5 MiB/);
  });
});
