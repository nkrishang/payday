"use client";

import { useCallback, useSyncExternalStore, type ReactNode } from "react";

/**
 * A stand-in for `@privy-io/react-auth`, for the browser suite and the
 * component tests.
 *
 * Privy cannot be driven by a test: a sign-in needs a real mailbox and a real
 * code. So when the suite asks (`PAYDAY_PRIVY_STUB=1`, see next.config.ts)
 * this module takes the SDK's place at bundle time, with the same hooks and
 * the same shapes the app reads — `usePrivy`, `useLoginWithEmail`,
 * `useIdentityToken`, `getIdentityToken`, `useCreateWallet`, `useUser` —
 * backed by a session kept in the tab's `sessionStorage`. The one code it
 * accepts is `123456`, the same one `e2e/stub-api.mjs` accepts everywhere
 * else.
 *
 * The identity token it mints is what the stub API recognizes as a dashboard
 * session: the fixed bearer prefix, then the claims (mailbox, DID, wallet) as
 * base64url JSON, so the stub can keep each mailbox's data apart exactly as
 * the real API keeps accounts apart.
 */

const SESSION_KEY = "payday.privy-stub.session";
const TOKEN_PREFIX = "stub-dashboard-token";
export const STUB_OTP = "123456";

interface StubSession {
  email: string;
  sub: string;
  wallet: string;
  identityToken: string;
}

/**
 * A wallet address that is a pure function of the mailbox, so a test can
 * meet the same address across sign-ins without the stub keeping state
 * between them. Not a real key; nothing ever signs with it.
 */
function walletFor(email: string): string {
  let hex = "";
  let seed = 0x811c9dc5;
  for (let round = 0; hex.length < 40; round += 1) {
    let hash = seed ^ round;
    for (const character of `${email}:${round}`) {
      hash ^= character.charCodeAt(0);
      hash = Math.imul(hash, 0x01000193) >>> 0;
    }
    hex += hash.toString(16).padStart(8, "0");
    seed = hash;
  }
  return `0x${hex.slice(0, 40)}`;
}

function base64url(value: string): string {
  return btoa(value).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function storage(): Storage | null {
  try {
    return typeof sessionStorage === "undefined" ? null : sessionStorage;
  } catch {
    return null;
  }
}

let current: StubSession | null | undefined;
const listeners = new Set<() => void>();

function read(): StubSession | null {
  if (current !== undefined) return current;
  const raw = storage()?.getItem(SESSION_KEY);
  current = raw ? (JSON.parse(raw) as StubSession) : null;
  return current;
}

function write(session: StubSession | null) {
  current = session;
  const store = storage();
  if (session) store?.setItem(SESSION_KEY, JSON.stringify(session));
  else store?.removeItem(SESSION_KEY);
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

function useSession(): StubSession | null {
  return useSyncExternalStore(subscribe, read, () => null);
}

/**
 * Like the real SDK, not `ready` until it has actually looked: during the
 * hydration render the session above is still the server's empty snapshot,
 * and a gate that trusted `ready` then would send a signed-in reload to the
 * landing page. Flips to true on the first client render, together with the
 * restored session.
 */
function useReady(): boolean {
  return useSyncExternalStore(
    subscribe,
    () => true,
    () => false,
  );
}

/** The mailbox a code was last sent to, awaiting that code. */
let pending: string | null = null;

export function PrivyProvider({ children }: { appId: string; config?: unknown; children: ReactNode }) {
  return <>{children}</>;
}

export function usePrivy() {
  const session = useSession();
  const ready = useReady();
  const logout = useCallback(async () => write(null), []);
  return {
    ready,
    authenticated: ready && session !== null,
    user: session
      ? {
          id: session.sub,
          email: { address: session.email },
          linkedAccounts: [
            { type: "email", address: session.email },
            {
              type: "wallet",
              address: session.wallet,
              chainType: "ethereum",
              walletClientType: "privy",
              connectorType: "embedded",
            },
          ],
        }
      : null,
    logout,
    getAccessToken: async () => session?.identityToken ?? null,
  };
}

export function useIdentityToken() {
  const session = useSession();
  return { identityToken: session?.identityToken ?? null };
}

/**
 * The imperative counterpart to useIdentityToken, exported at module scope
 * exactly like the real SDK's — signup-dialog.tsx's waitForWallet reads the
 * account straight after login, before any hook reflecting the new session
 * has necessarily re-rendered.
 */
export async function getIdentityToken(): Promise<string | null> {
  return read()?.identityToken ?? null;
}

export function useLoginWithEmail() {
  const sendCode = useCallback(async ({ email }: { email: string }) => {
    if (!email.includes("@")) throw new Error("Invalid email address");
    pending = email.trim().toLowerCase();
  }, []);
  const loginWithCode = useCallback(async ({ code }: { code: string }) => {
    if (!pending) throw new Error("No code was sent");
    if (code !== STUB_OTP) throw new Error("Invalid verification code");
    const email = pending;
    pending = null;
    const wallet = walletFor(email);
    const sub = `did:privy:stub-${base64url(email)}`;
    const claims = base64url(JSON.stringify({ sub, email, wallet }));
    write({ email, sub, wallet, identityToken: `${TOKEN_PREFIX}.${claims}` });
  }, []);
  return { sendCode, loginWithCode, state: { status: "initial" as const } };
}

export function useCreateWallet() {
  // Every stub session already has its wallet; asking again is a no-op.
  const createWallet = useCallback(async () => {
    const session = read();
    if (!session) throw new Error("Not signed in");
    return { address: session.wallet };
  }, []);
  return { createWallet };
}

export function useUser() {
  // signup-dialog.tsx only reads refreshUser: the stub's identity token
  // already carries the wallet the instant loginWithCode writes it, so
  // there is nothing a refresh would ever change.
  const refreshUser = useCallback(async () => undefined, []);
  return { refreshUser };
}
