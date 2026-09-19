"use client";

import { type GumClient, GumError } from "@gum/sdk";
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
  useSyncExternalStore,
  type ReactNode,
} from "react";
import { describeLoadError, type LoadFailure } from "@/lib/load-error";
import { createMerchantClient } from "@/lib/merchant-gum";
import { SigningOutScreen } from "./session-screens";

export const HOME_PATH = "/dashboard";
/**
 * Where anyone without a session goes. There is no dashboard login page: the
 * landing page's "Get Started" is the only way in, and it is the same
 * exchange, so an expired or absent session lands where a new merchant does.
 */
export const SIGNED_OUT_PATH = "/";

export interface Merchant {
  client: GumClient;
  /**
   * The session token itself, held only for the attachment uploader, which
   * builds its own observing client.
   */
  accessToken: string;
  /** The mailbox the merchant signed in with, as Privy reports it. */
  email: string | null;
  /** The stable Privy identity id: what the resource cache is owned by. */
  subject: string;
  signOut: () => void;
}

const MerchantContext = createContext<Merchant | null>(null);
const ResourceCacheContext = createContext<ResourceCache | null>(null);

/** Supplies a merchant to a subtree; tests use it in place of the gate. */
export function MerchantProvider({ value, children }: { value: Merchant; children: ReactNode }) {
  const cache = useResourceCacheFor(value.signOut, value.subject);
  return (
    <MerchantContext.Provider value={value}>
      <ResourceCacheContext.Provider value={cache}>{children}</ResourceCacheContext.Provider>
    </MerchantContext.Provider>
  );
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
  const subject = user?.id ?? "";
  const value = useMemo<Merchant | null>(
    () =>
      authenticated && identityToken && subject
        ? {
            client: createMerchantClient(identityToken),
            accessToken: identityToken,
            email,
            subject,
            signOut,
          }
        : null,
    [authenticated, identityToken, email, subject, signOut],
  );
  // Keyed by the identity id, not the token and not the mailbox: Privy
  // rotates the identity token while the merchant is signed in, and changing
  // the sign-in email must not empty the page — the same person keeps their
  // loaded data either way.
  const cache = useResourceCacheFor(signOut, subject);

  if (signingOut) {
    return <SigningOutScreen />;
  }
  if (value === null) {
    return <>{checking ?? <CheckingSession />}</>;
  }
  return (
    <MerchantContext.Provider value={value}>
      <ResourceCacheContext.Provider value={cache}>{children}</ResourceCacheContext.Provider>
    </MerchantContext.Provider>
  );
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

/*
 * Loaded data, shared across the page.
 *
 * A dashboard page is many components that each need something from the API,
 * and several need the same thing: the identities, the customers, the account.
 * Each `useResource` used to be its own request, so one paint of the home
 * page was seven requests — fourteen in development, where React mounts
 * everything twice — and every token rotation and hot reload sent them all
 * again, which is how a merchant refreshing their own dashboard met the
 * API's per-account limit. Now the page shares one cache: one request per
 * key however many components ask, a result that outlives the component that
 * asked for it, and a background refresh rather than a blank page when the
 * data is old enough to doubt.
 *
 * A read that fails with congestion (429), an upstream error, or a dropped
 * connection is not an error to the page yet: the client has already
 * retried it (`retryingFetch`), and the cache tries again a few more times
 * on a growing backoff while the page keeps its skeleton. Only when that is
 * exhausted, or the failure is one that repeating cannot fix, does the page
 * hear about it.
 */

/** Data younger than this is served as is; older is shown and refreshed behind it. */
const FRESH_MS = 10_000;
/** Automatic retries of a transient failure before it is shown, after the client's own. */
const AUTO_RETRIES = 3;
const AUTO_RETRY_BASE_MS = 1_000;

export interface Resource<T> {
  data: T | null;
  /** What to tell the merchant, or null while there is nothing to tell. */
  error: string | null;
  /** The technical detail behind `error`, for the small print. */
  detail: string | null;
  loading: boolean;
  /** Fetch again; every component reading this key sees the new result. */
  reload: () => void;
}

interface Snapshot {
  data: unknown;
  failure: LoadFailure | null;
  loading: boolean;
}

const EMPTY: Snapshot = { data: null, failure: null, loading: true };

type Loader = () => Promise<unknown>;

interface Entry {
  snapshot: Snapshot;
  fetchedAt: number;
  /** The most recent loader a subscriber gave, so a refresh needs no component. */
  loader: Loader | null;
  inflight: Promise<void> | null;
  /** A reload asked for while a fetch was in flight: go again when it lands. */
  queued: boolean;
  /** Automatic retries spent on the current failure. */
  attempt: number;
  retryTimer: ReturnType<typeof setTimeout> | null;
  listeners: Set<() => void>;
}

class ResourceCache {
  private readonly entries = new Map<string, Entry>();

  constructor(
    private readonly signOut: () => void,
    /** The identity this cache belongs to; a different one gets a new cache. */
    readonly owner: string | null,
  ) {}

  private entry(key: string): Entry {
    let entry = this.entries.get(key);
    if (!entry) {
      entry = {
        snapshot: EMPTY,
        fetchedAt: 0,
        loader: null,
        inflight: null,
        queued: false,
        attempt: 0,
        retryTimer: null,
        listeners: new Set(),
      };
      this.entries.set(key, entry);
    }
    return entry;
  }

  snapshot(key: string): Snapshot {
    return this.entries.get(key)?.snapshot ?? EMPTY;
  }

  /**
   * Reads `key` on behalf of one component: fetches when nothing is known,
   * refreshes behind the data when it is old, and otherwise serves what is
   * there. The subscriber is told of every change until it unsubscribes.
   */
  subscribe(key: string, loader: Loader, listener: () => void): () => void {
    const entry = this.entry(key);
    entry.loader = loader;
    entry.listeners.add(listener);
    const settled = entry.snapshot.data !== null || entry.snapshot.failure !== null;
    if (!settled || Date.now() - entry.fetchedAt > FRESH_MS) {
      this.fetch(key);
    }
    return () => {
      entry.listeners.delete(listener);
      // Nobody is looking any more: a pending automatic retry has no page to
      // fill, and the data itself stays for whoever looks next.
      if (entry.listeners.size === 0 && entry.retryTimer !== null) {
        clearTimeout(entry.retryTimer);
        entry.retryTimer = null;
        entry.attempt = 0;
        entry.snapshot = { ...entry.snapshot, loading: false };
      }
    };
  }

  /** Fetch `key` again now; a fetch already in flight is followed by another. */
  refresh(key: string): void {
    const entry = this.entries.get(key);
    if (!entry || entry.loader === null) return;
    if (entry.inflight !== null) {
      entry.queued = true;
      return;
    }
    if (entry.retryTimer !== null) {
      clearTimeout(entry.retryTimer);
      entry.retryTimer = null;
    }
    entry.attempt = 0;
    this.fetch(key);
  }

  /**
   * Something changed on the server: every key under `prefix` is refreshed
   * where a component is showing it and forgotten where none is, so the next
   * look fetches afresh.
   */
  invalidate(prefix: string): void {
    for (const [key, entry] of this.entries) {
      if (!key.startsWith(prefix)) continue;
      if (entry.listeners.size > 0) {
        this.refresh(key);
      } else {
        if (entry.retryTimer !== null) clearTimeout(entry.retryTimer);
        this.entries.delete(key);
      }
    }
  }

  private fetch(key: string): void {
    const entry = this.entry(key);
    if (entry.inflight !== null || entry.loader === null) return;
    const loader = entry.loader;
    if (!entry.snapshot.loading) this.publish(entry, { ...entry.snapshot, loading: true });
    entry.inflight = loader()
      .then((data) => {
        entry.fetchedAt = Date.now();
        entry.attempt = 0;
        this.publish(entry, { data, failure: null, loading: false });
      })
      .catch((cause: unknown) => {
        if (cause instanceof GumError && cause.status === 401) {
          // The token expired: the session ends and the landing page takes
          // over, rather than every page handling it.
          this.signOut();
          return;
        }
        const failure = describeLoadError(cause);
        if (failure.transient && entry.attempt < AUTO_RETRIES && entry.listeners.size > 0) {
          const delay = AUTO_RETRY_BASE_MS * 2 ** entry.attempt;
          entry.attempt += 1;
          entry.retryTimer = setTimeout(() => {
            entry.retryTimer = null;
            this.fetch(key);
          }, delay);
          return;
        }
        entry.attempt = 0;
        this.publish(entry, { data: entry.snapshot.data, failure, loading: false });
      })
      .finally(() => {
        entry.inflight = null;
        if (entry.queued) {
          entry.queued = false;
          this.fetch(key);
        }
      });
  }

  private publish(entry: Entry, snapshot: Snapshot): void {
    entry.snapshot = snapshot;
    for (const listener of entry.listeners) listener();
  }
}

/** One cache per signed-in merchant, emptied when the identity changes. */
function useResourceCacheFor(signOut: () => void, owner: string | null): ResourceCache {
  return useMemo(() => new ResourceCache(signOut, owner), [signOut, owner]);
}

function useResourceCache(): ResourceCache {
  const cache = useContext(ResourceCacheContext);
  if (cache === null) throw new Error("useResource must be used inside MerchantGate");
  return cache;
}

/**
 * Loads one thing with the merchant client, shared with every other component
 * reading the same `key` (see the cache above). `key` names what is being
 * loaded — a kind, an id, a filter, a cursor — and a change re-fetches; keys
 * are one namespace across the page, so name the kind (`customer:${id}`),
 * never the id alone.
 */
export function useResource<T>(
  key: string,
  load: (client: GumClient) => Promise<T>,
): Resource<T> {
  const cache = useResourceCache();
  const { client } = useMerchant();
  // The latest loader and client, read when a fetch actually starts: a token
  // rotation swaps the client without re-fetching anything.
  const loader = useRef(load);
  const current = useRef(client);
  useEffect(() => {
    loader.current = load;
    current.current = client;
  });

  const subscribe = useCallback(
    (listener: () => void) => cache.subscribe(key, () => loader.current(current.current), listener),
    [cache, key],
  );
  const snapshot = useSyncExternalStore(
    subscribe,
    () => cache.snapshot(key),
    () => EMPTY,
  );
  const reload = useCallback(() => cache.refresh(key), [cache, key]);

  return {
    data: snapshot.data as T | null,
    error: snapshot.failure?.message ?? null,
    detail: snapshot.failure?.detail ?? null,
    loading: snapshot.loading,
    reload,
  };
}

/**
 * Tells the page that something under `prefix` changed on the server —
 * `"deposit-requests"` after issuing one, say — so every component showing
 * such a thing fetches it again, and nothing stale is served to the next.
 */
export function useInvalidate(): (prefix: string) => void {
  const cache = useResourceCache();
  return useCallback((prefix: string) => cache.invalidate(prefix), [cache]);
}
