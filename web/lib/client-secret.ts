import { CLIENT_SECRET_FRAGMENT_KEY } from "@payday/sdk";

/**
 * A merchant-session payment arrives with its single-use client secret in the
 * URL fragment: `/pay/{id}#cs=cs_…`. The fragment is the one part of a URL a
 * browser never sends anywhere — not to this app's server, not in a Referer,
 * not to an analytics beacon — which is exactly why the merchant put it
 * there. This module reads it once and removes it from the address bar in the
 * same breath, so a reload, a bookmark, or a shared tab never carries it.
 */

const SHAPE = /^cs_[A-Za-z0-9_-]{43}$/;

/** The secret a fragment carries, if it has the shape of one. */
export function clientSecretFromFragment(fragment: string): string | null {
  const params = new URLSearchParams(fragment.replace(/^#/, ""));
  const secret = params.get(CLIENT_SECRET_FRAGMENT_KEY);
  return secret !== null && SHAPE.test(secret) ? secret : null;
}

/**
 * Take the client secret out of this tab's URL: read it, then rewrite the
 * address bar without the fragment. Returns null when the page was opened
 * without one — the bare link — or outside a browser.
 */
export function takeClientSecret(): string | null {
  if (typeof window === "undefined") return null;
  const secret = clientSecretFromFragment(window.location.hash);
  if (window.location.hash) {
    try {
      window.history.replaceState(
        window.history.state,
        "",
        window.location.pathname + window.location.search,
      );
    } catch {
      // A sandboxed frame may refuse; the secret is spent by the exchange
      // anyway, so a lingering fragment opens nothing twice.
    }
  }
  return secret;
}
