import { useCallback, useSyncExternalStore } from "react";

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
