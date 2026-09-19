/**
 * SDK adapters with an instrumented transport: one merchant client (bearer
 * key) and one payer client (no key, checkout origin). The transport records
 * logical-call completion — after response parsing, not at header receipt —
 * plus HTTP attempts, status codes, and Retry-After signals.
 */

import { GumClient, GumPayerClient } from "@gum/sdk";
import type { EventSink } from "./model.js";

export interface InstrumentedFetchOptions {
  sink: EventSink;
  routeLabel: string;
  extraHeaders?: Record<string, string>;
}

export function instrumentedFetch(sink: EventSink, extraHeaders: Record<string, string> = {}): { fetch: typeof globalThis.fetch } {
  let attempt = 0;
  const wrapped: typeof globalThis.fetch = async (input, init) => {
    const route = routeOf(input);
    attempt += 1;
    const started = performance.now();
    try {
      const headers = new Headers(init?.headers);
      for (const [name, value] of Object.entries(extraHeaders)) headers.set(name, value);
      headers.set("x-request-id", crypto.randomUUID());
      const response = await fetch(input, { ...init, headers });
      const ms = performance.now() - started;
      sink({ type: "api.call", payload: { route, status: response.status, ms, attempt } });
      if (response.status === 429) {
        const retryAfter = Number(response.headers.get("retry-after") ?? "") || undefined;
        sink({ type: "api.rate_limited", payload: { route, ...(retryAfter !== undefined ? { retryAfter } : {}) } });
      }
      return response;
    } catch (error) {
      const ms = performance.now() - started;
      sink({ type: "api.call", payload: { route, status: 0, ms, attempt } });
      throw error;
    }
  };
  return { fetch: wrapped };
}

function routeOf(input: Parameters<typeof fetch>[0]): string {
  const url = typeof input === "string" ? input : input instanceof URL ? input : input.url;
  try {
    const parsed = new URL(url);
    return `${parsed.host}${parsed.pathname}`;
  } catch {
    return "unknown";
  }
}

export interface Clients {
  merchant: GumClient;
  payer: GumPayerClient;
}

export function createClients(options: { apiOrigin: string; apiKey: string; sink: EventSink; checkoutOrigin: string }): Clients {
  const merchantFetch = instrumentedFetch(options.sink);
  const payerFetch = instrumentedFetch(options.sink, { origin: options.checkoutOrigin });
  return {
    merchant: new GumClient({ baseUrl: options.apiOrigin, apiKey: options.apiKey, fetch: merchantFetch.fetch }),
    payer: new GumPayerClient({ baseUrl: options.apiOrigin, fetch: payerFetch.fetch }),
  };
}
