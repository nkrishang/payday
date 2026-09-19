import { GumError } from "@gum/sdk";

/**
 * What a failed read means to the merchant looking at the page.
 *
 * The API's own message is written for an integrator reading a response, not
 * for someone whose dashboard did not come up, so the page says what happened
 * in its own words and keeps the technical text — the message and the request
 * id support would ask for — apart from it. `transient` marks the failures
 * that are worth trying again without being asked: congestion, an upstream
 * hiccup, a dropped connection.
 */
export interface LoadFailure {
  /** One sentence for the page. */
  message: string;
  /** The API's message and request id, for the small print. */
  detail: string | null;
  /** Whether trying again a moment later is likely to succeed. */
  transient: boolean;
}

function requestSuffix(error: GumError): string {
  return error.requestId ? ` (request ${error.requestId})` : "";
}

export function describeLoadError(error: unknown): LoadFailure {
  if (error instanceof GumError) {
    const detail = `${error.message}${requestSuffix(error)}`;
    if (error.status === 429) {
      return { message: "Gum is busy right now.", detail, transient: true };
    }
    if (error.status >= 500) {
      return { message: "Gum is temporarily unavailable.", detail, transient: true };
    }
    if (error.status === 404) {
      return { message: "That record is no longer here.", detail, transient: false };
    }
    if (error.status === 403) {
      return { message: "This account can't do that.", detail, transient: false };
    }
    return { message: error.message, detail: error.requestId ? detail : null, transient: false };
  }
  if (error instanceof DOMException && error.name === "AbortError") {
    return { message: "The request was cancelled.", detail: null, transient: false };
  }
  // `fetch` rejects with a TypeError when the network, not the server, failed.
  if (error instanceof TypeError) {
    return {
      message: "Gum can't be reached. Check your connection.",
      detail: error.message || null,
      transient: true,
    };
  }
  if (error instanceof Error && error.message) {
    return { message: "Something went wrong.", detail: error.message, transient: false };
  }
  return { message: "Something went wrong.", detail: null, transient: false };
}
