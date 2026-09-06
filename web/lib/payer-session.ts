import { PREVIEW_SESSION_FRAGMENT_KEY, previewUrl, type PaydayClient } from "@payday/sdk";
import { useCallback, useEffect, useState, useSyncExternalStore } from "react";

/**
 * The opaque payer session for one payment, kept in this tab's
 * `sessionStorage` and nowhere else: never in server-rendered HTML, local
 * storage, a URL, or anything that reports. It lets the payer resume after a
 * reload or a hosted verification flow, and ends with the tab.
 */

const PREFIX = "payday:payer-session:";

export function payerSessionKey(paymentId: string): string {
  return `${PREFIX}${paymentId}`;
}

function storage(): Storage | null {
  try {
    return typeof window === "undefined" ? null : window.sessionStorage;
  } catch {
    // Storage access can throw (privacy modes, sandboxed frames); the payer
    // then verifies again next time, which is safe.
    return null;
  }
}

export function loadPayerSession(paymentId: string): string | null {
  try {
    return storage()?.getItem(payerSessionKey(paymentId)) || null;
  } catch {
    return null;
  }
}

export function storePayerSession(paymentId: string, token: string): void {
  try {
    storage()?.setItem(payerSessionKey(paymentId), token);
  } catch {
    // The session still works for this page load.
  }
}

export function clearPayerSession(paymentId: string): void {
  try {
    storage()?.removeItem(payerSessionKey(paymentId));
  } catch {
    // Nothing to clear.
  }
}

/**
 * The preview session a fragment carries, if this page load was opened with
 * one. Read once and removed from the address bar in the same breath, the
 * same way a merchant-session client secret is (`lib/client-secret.ts`) — the
 * fragment is the one part of a URL a browser never sends anywhere.
 */
export function takePreviewSession(): string | null {
  if (typeof window === "undefined") return null;
  const token = new URLSearchParams(window.location.hash.replace(/^#/, "")).get(
    PREVIEW_SESSION_FRAGMENT_KEY,
  );
  if (window.location.hash) {
    try {
      window.history.replaceState(
        window.history.state,
        "",
        window.location.pathname + window.location.search,
      );
    } catch {
      // A sandboxed frame may refuse; harmless — the token is still handed
      // up to the caller either way.
    }
  }
  return token;
}

/**
 * The deposit request's own payer view, unlocked for its issuing merchant —
 * mints a preview session up front (`client.depositRequests.previewSession`,
 * not verification: it records no attempt and never marks the deposit
 * request's own verification complete) and carries it in the URL fragment
 * (`previewUrl`), exactly as a merchant-session client secret is. Doing this
 * eagerly rather than on click means the link is a real, synchronously-known
 * `href` by the time anyone clicks it — opening it in a new tab is never at
 * risk of a popup blocker, and there is no reliance on this tab's storage
 * reaching the new one before it navigates.
 *
 * Falls back to the bare, locked `deposit_url` while minting or if it fails:
 * this is a convenience, not something sign-in or the link itself depends on.
 */
export function usePayerPreviewUrl(
  client: PaydayClient,
  payment: { id: string; deposit_url: string },
): string {
  const [url, setUrl] = useState(payment.deposit_url);
  const depositUrl = payment.deposit_url;
  useEffect(() => {
    let disposed = false;
    client.depositRequests.previewSession(payment.id).then(
      ({ payer_session }) => {
        if (!disposed) setUrl(previewUrl({ deposit_url: depositUrl }, payer_session));
      },
      () => {
        // Best-effort — see doc comment above.
      },
    );
    return () => {
      disposed = true;
    };
  }, [client, payment.id, depositUrl]);
  return url;
}

/** Components reading a session through the hook, told when one changes. */
const listeners = new Set<() => void>();

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function serverSnapshot(): string | null {
  return null;
}

/**
 * The stored session as React state. Server rendering and hydration see no
 * session, so the markup agrees on both sides and the token never enters
 * server output; the client then reads the tab's storage.
 */
export function usePayerSession(
  paymentId: string,
): [string | null, (token: string | null) => void] {
  const session = useSyncExternalStore(
    subscribe,
    () => loadPayerSession(paymentId),
    serverSnapshot,
  );

  const update = useCallback(
    (token: string | null) => {
      if (token) storePayerSession(paymentId, token);
      else clearPayerSession(paymentId);
      for (const listener of listeners) listener();
    },
    [paymentId],
  );

  return [session, update];
}
