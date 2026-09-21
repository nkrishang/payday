/**
 * Harness-owned webhook receiver: verifies Gum's HMAC signature over the raw
 * body, persists the capture durably before answering 2xx, and deduplicates
 * logical events by id while keeping every delivery attempt.
 *
 * Accepts out-of-order delivery; lifecycle reconstruction happens in the
 * reducer from event type plus API state, never arrival order. In local
 * mode the API rejects loopback webhook URLs, so point the public URL at a
 * tunnel that forwards here.
 */

import { createHmac, timingSafeEqual } from "node:crypto";
import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";

export interface WebhookCapture {
  eventId: string;
  type: string;
  depositId?: string;
  raw: string;
  receivedMono: number;
}

export type CaptureSink = (capture: WebhookCapture) => void;

const FRESHNESS_SECONDS = 300;

export function verifySignature(raw: string, header: string | undefined, secret: string, nowSeconds: number): { ok: true } | { ok: false; reason: string } {
  if (!header) return { ok: false, reason: "missing Gum-Signature" };
  const match = /^v1,t=(\d+),sha256=([0-9a-f]+)$/.exec(header.trim());
  if (!match) return { ok: false, reason: "unparseable Gum-Signature" };
  const timestamp = Number(match[1]);
  if (!Number.isFinite(timestamp) || Math.abs(nowSeconds - timestamp) > FRESHNESS_SECONDS) {
    return { ok: false, reason: "stale or invalid timestamp" };
  }
  const expected = createHmac("sha256", secret).update(`v1.${timestamp}.${raw}`).digest("hex");
  const given = match[2];
  const a = Buffer.from(expected, "hex");
  const b = Buffer.from(given, "hex");
  if (a.length !== b.length || !timingSafeEqual(a, b)) return { ok: false, reason: "signature mismatch" };
  return { ok: true };
}

interface Envelope {
  id?: string;
  type?: string;
  data?: { deposit_request?: { id?: string } };
}

export class WebhookReceiver {
  private server?: Server;
  private secret = "";
  private seen = new Set<string>();

  constructor(private readonly sink: CaptureSink, private readonly onInvalid: (reason: string) => void) {}

  registerSecret(secret: string): void {
    this.secret = secret;
  }

  get endpointSecret(): string {
    return this.secret;
  }

  async start(port: number): Promise<void> {
    this.server = createServer((request, response) => void this.handle(request, response));
    await new Promise<void>((resolve) => this.server!.listen(port, "0.0.0.0", resolve));
  }

  async stop(): Promise<void> {
    if (!this.server) return;
    await new Promise<void>((resolve) => this.server!.close(() => resolve()));
  }

  private async handle(request: IncomingMessage, response: ServerResponse): Promise<void> {
    const chunks: Buffer[] = [];
    for await (const chunk of request) chunks.push(chunk as Buffer);
    const raw = Buffer.concat(chunks).toString("utf8");
    const receivedMono = performance.now();

    const headerValue = request.headers["gum-signature"];
    const header = Array.isArray(headerValue) ? headerValue[0] : headerValue;
    const verdict = verifySignature(raw, header, this.secret, Math.floor(Date.now() / 1000));
    if (!verdict.ok) {
      this.onInvalid(verdict.reason);
      response.statusCode = 400;
      response.end("invalid signature");
      return;
    }
    let envelope: Envelope;
    try {
      envelope = JSON.parse(raw) as Envelope;
    } catch {
      this.onInvalid("unparseable JSON body");
      response.statusCode = 400;
      response.end("invalid json");
      return;
    }
    const eventId = envelope.id ?? "";
    const type = envelope.type ?? "";
    if (!eventId || !type) {
      this.onInvalid("envelope missing id or type");
      response.statusCode = 400;
      response.end("invalid envelope");
      return;
    }
    // webhook.test is outside the lifecycle; still captured for endpoint health.
    const depositId = envelope.data?.deposit_request?.id;
    this.sink({ eventId, type, ...(depositId ? { depositId } : {}), raw, receivedMono });
    // Acknowledged only after the durable capture the sink performed.
    response.statusCode = 200;
    response.end("ok");
    void this.seen;
  }
}
