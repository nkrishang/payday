"use client";

import { type PaydayClient, PaydayError } from "@payday/sdk";
import { useCreateWallet, useIdentityToken, usePrivy } from "@privy-io/react-auth";
import { useRouter } from "next/navigation";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { describeError } from "@/lib/attachment-upload";
import { createMerchantClient } from "@/lib/merchant-payday";
import { SigningOutScreen } from "./session-screens";

export const HOME_PATH = "/dashboard";
/**
 * Where anyone without a session goes. There is no dashboard login page: the
 * landing page's "Get Started" is the only way in, and it is the same
 * exchange, so an expired or absent session lands where a new merchant does.
 */
export const SIGNED_OUT_PATH = "/";

export interface Merchant {
  client: PaydayClient;
  /**
   * The session token itself, held only for the attachment uploader, which
   * builds its own observing client.
   */
  accessToken: string;
  /** The mailbox the merchant signed in with, as Privy reports it. */
  email: string | null;
  signOut: () => void;
}

const MerchantContext = createContext<Merchant | null>(null);

/** Supplies a merchant to a subtree; tests use it in place of the gate. */
export function MerchantProvider({ value, children }: { value: Merchant; children: ReactNode }) {
  return <MerchantContext.Provider value={value}>{children}</MerchantContext.Provider>;
}

/** The merchant when there is one, for chrome that outlives the session check. */
export function useOptionalMerchant(): Merchant | null {
  return useContext(MerchantContext);
}

/**
 * Renders its children only for a signed-in merchant.
 *
 * Privy holds the session. Until it has restored one (`ready`) the page shows
 * `checking` — a skeleton of the page it is about to be — so the HTML for
 * every dashboard page is one empty shell whether or not anyone is signed
 * in; once it has, a visitor without a session is sent to the landing page,
 * and a merchant gets a client bearing their identity token.
 *
 * Signing out is its own state, not a return to checking: the moment the
 * merchant asks, the whole page becomes a sign-out screen that holds until
 * the landing page replaces it, so a slow `logout` round trip never shows
 * the dashboard half-alive or a bare "checking" flash on the way out.
 */
export function MerchantGate({
  children,
  checking,
}: {
  children: ReactNode;
  /** Shown while Privy restores the session, in place of `children`. */
  checking?: ReactNode;
}) {
  const router = useRouter();
  const { ready, authenticated, user, logout } = usePrivy();
  const { identityToken } = useIdentityToken();
  const { createWallet } = useCreateWallet();
  const [signingOut, setSigningOut] = useState(false);

  useEffect(() => {
    if (ready && !authenticated && !signingOut) router.replace(SIGNED_OUT_PATH);
  }, [ready, authenticated, signingOut, router]);

  // Every account gets an embedded wallet. Privy creates it on login when it
  // can; a login through our own dialog rather than Privy's modal does not
  // always qualify, so the gate asks for one itself whenever a signed-in
  // user has none. Asked once per mount: a second ask while the first is in
  // flight would be refused as a duplicate, and a failure shows up as the
  // account section's "still being created" state rather than a loop.
  const askedForWallet = useRef(false);
  useEffect(() => {
    if (!ready || !authenticated || !user || askedForWallet.current) return;
    const hasWallet = user.linkedAccounts.some(
      (account) =>
        account.type === "wallet" &&
        account.walletClientType === "privy" &&
        account.chainType === "ethereum",
    );
    if (hasWallet) return;
    askedForWallet.current = true;
    createWallet().catch(() => {
      // Already there (created between the check and the call), or a
      // transient failure; the next sign-in asks again.
    });
  }, [ready, authenticated, user, createWallet]);

  const signOut = useCallback(() => {
    // First paint of signing out, before anything async: the screen replaces
    // the whole page in the same commit, so nothing of the dashboard lingers
    // while Privy's logout round trip runs.
    setSigningOut(true);
    void logout()
      .finally(() => {
        // The landing page takes over from here. Until the navigation
        // completes, the sign-out screen below is still what renders, so
        // there is no blank frame between the two pages.
        router.replace(SIGNED_OUT_PATH);
      })
      // A failed logout must not strand the merchant on a page whose session
      // is half gone; the landing page and its sign-in dialog can recover.
      .catch(() => {});
  }, [logout, router]);

  const email = user?.email?.address ?? null;
  const value = useMemo<Merchant | null>(
    () =>
      authenticated && identityToken
        ? {
            client: createMerchantClient(identityToken),
            accessToken: identityToken,
            email,
            signOut,
          }
        : null,
    [authenticated, identityToken, email, signOut],
  );

  if (signingOut) {
    return <SigningOutScreen />;
  }
  if (value === null) {
    return <>{checking ?? <CheckingSession />}</>;
  }
  return <MerchantContext.Provider value={value}>{children}</MerchantContext.Provider>;
}

/** The bare fallback for embedders that pass no `checking` state. */
function CheckingSession() {
  return (
    <p role="status" className="px-5 py-10 text-center text-[13px] text-faint">
      Checking your session…
    </p>
  );
}

export function useMerchant(): Merchant {
  const merchant = useContext(MerchantContext);
  if (merchant === null) throw new Error("useMerchant must be used inside MerchantGate");
  return merchant;
}

export interface Resource<T> {
  data: T | null;
  error: string | null;
  loading: boolean;
  reload: () => void;
}

/**
 * Loads one thing with the merchant client. `key` names what is being loaded
 * (an id, a filter, a cursor) and a change re-fetches. A 401 means the token
 * expired: the session ends and the landing page takes over, rather than every
 * page handling it.
 */
export function useResource<T>(
  key: string,
  load: (client: PaydayClient) => Promise<T>,
): Resource<T> {
  const { client, signOut } = useMerchant();
  const loader = useRef(load);
  useEffect(() => {
    loader.current = load;
  });

  const [version, setVersion] = useState(0);
  // Identifies one request; a result is current only if it answers this one.
  const request = `${key}\x00${version}`;
  const [settled, setSettled] = useState<{
    request: string;
    data: T | null;
    error: string | null;
  } | null>(null);

  useEffect(() => {
    let disposed = false;
    loader
      .current(client)
      .then((data) => {
        if (!disposed) setSettled({ request, data, error: null });
      })
      .catch((error: unknown) => {
        if (disposed) return;
        if (error instanceof PaydayError && error.status === 401) {
          signOut();
          return;
        }
        setSettled({ request, data: null, error: describeError(error) });
      });
    return () => {
      disposed = true;
    };
  }, [client, signOut, request]);

  const loading = settled?.request !== request;
  const reload = useCallback(() => setVersion((current) => current + 1), []);
  return {
    data: settled?.data ?? null,
    error: loading ? null : (settled?.error ?? null),
    loading,
    reload,
  };
}
