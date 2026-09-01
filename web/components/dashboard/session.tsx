"use client";

import { type PaydayClient, PaydayError } from "@payday/sdk";
import { useRouter } from "next/navigation";
import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import { describeError } from "@/lib/attachment-upload";
import { createMerchantClient, merchantSession } from "@/lib/merchant-payday";

export const LOGIN_PATH = "/dashboard/login";
export const HOME_PATH = "/dashboard/invoices";

export interface Merchant {
  client: PaydayClient;
  /** Held only for the attachment uploader, which builds its own observing client. */
  accessToken: string;
  signOut: () => void;
}

const MerchantContext = createContext<Merchant | null>(null);

/** Supplies a merchant to a subtree; tests use it in place of the gate. */
export function MerchantProvider({ value, children }: { value: Merchant; children: ReactNode }) {
  return <MerchantContext.Provider value={value}>{children}</MerchantContext.Provider>;
}

// The session changes only through navigation (login, sign-out), so there is
// nothing to subscribe to; the store is read fresh on every render.
const subscribeNever = () => () => {};
const readToken = () => merchantSession.get()?.accessToken ?? null;
const serverToken = () => null;

/**
 * Renders its children only for a signed-in merchant. The session is read as
 * an external store whose server snapshot is always empty: the server never
 * sees the token and never renders merchant data, so the HTML for every
 * dashboard page is the same empty shell whether or not anyone is signed in.
 */
export function MerchantGate({ children }: { children: ReactNode }) {
  const router = useRouter();
  const accessToken = useSyncExternalStore(subscribeNever, readToken, serverToken);

  useEffect(() => {
    // Re-read rather than trust the rendered value: the first commit after
    // hydration still carries the server snapshot.
    if (merchantSession.get() === null) router.replace(LOGIN_PATH);
  }, [accessToken, router]);

  const signOut = useCallback(() => {
    merchantSession.clear();
    router.replace(LOGIN_PATH);
  }, [router]);

  const value = useMemo<Merchant | null>(
    () =>
      accessToken === null
        ? null
        : { client: createMerchantClient(accessToken), accessToken, signOut },
    [accessToken, signOut],
  );

  if (value === null) {
    return (
      <p role="status" className="px-5 py-10 text-center text-[13px] text-faint">
        Checking your session…
      </p>
    );
  }
  return <MerchantContext.Provider value={value}>{children}</MerchantContext.Provider>;
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
 * expired: the session ends and the login page takes over, rather than every
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
  const request = `${key}\u0000${version}`;
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
