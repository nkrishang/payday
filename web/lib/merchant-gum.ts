import { GumClient } from "@gum/sdk";
import { config } from "./config";

/**
 * Merchant dashboard identity.
 *
 * Merchants sign in through Privy with an emailed code. What Privy hands the
 * browser back is an identity token — a signed statement of who the user is,
 * the mailbox they proved, and the embedded wallet Privy holds for them — and
 * that token is the session: the API accepts it as the bearer credential on
 * every merchant route, so the browser never holds or issues an API key.
 *
 * Privy owns the token's lifetime. Its SDK refreshes it while the merchant is
 * signed in and clears it on sign-out; nothing here stores a credential of its
 * own. (The API-key step-up the old flow needed is gone with it: the session
 * is the credential, and an API key cannot mint another key.)
 */

function required(value: string | undefined, name: string): string {
  if (!value) {
    throw new Error(`${name} is not set. Copy web/.env.example to web/.env.local and fill it in.`);
  }
  return value;
}

/**
 * The Privy app the dashboard signs in to. Read lazily so the checkout, which
 * never imports the dashboard, builds without it. Literal
 * `process.env.NEXT_PUBLIC_*` access is what lets Next inline it; do not
 * refactor into a lookup.
 */
export function privyAppId(): string {
  return required(process.env.NEXT_PUBLIC_PRIVY_APP_ID, "NEXT_PUBLIC_PRIVY_APP_ID");
}

/**
 * How long the sign-in dialog waits before offering to send another code.
 * Privy rate-limits the endpoint itself; this keeps a merchant from running
 * into that limit, and from holding two live codes and guessing which one the
 * page wants.
 */
export const RESEND_COOLDOWN_MS = 60 * 1000;

/**
 * How many times one request is sent before its failure is reported, and the
 * longest a single wait between sends may be. The API's per-account limit
 * refills one request a second and answers `Retry-After: 1`, so a burst that
 * overran it clears within a couple of retries; anything still failing after
 * these is a real outage, not congestion.
 */
export const RETRY_ATTEMPTS = 3;
const RETRY_BASE_MS = 400;
const RETRY_MAX_MS = 4_000;

/** Statuses that say "not now" rather than "no". */
const TRANSIENT_STATUSES = new Set([429, 502, 503, 504]);

/**
 * Whether a request can be sent again without doing something twice: reads
 * always, writes only when the caller pinned them with an idempotency key.
 */
function retriable(init: RequestInit | undefined): boolean {
  const method = (init?.method ?? "GET").toUpperCase();
  if (method === "GET" || method === "HEAD") return true;
  return new Headers(init?.headers).has("Idempotency-Key");
}

/** The wait the API asked for, when it named one, else exponential with jitter. */
function retryDelay(response: Response | null, attempt: number): number {
  const asked = response?.headers.get("Retry-After");
  const seconds = asked === null || asked === undefined ? Number.NaN : Number(asked);
  if (Number.isFinite(seconds) && seconds >= 0) {
    return Math.min(seconds * 1000, RETRY_MAX_MS);
  }
  const backoff = RETRY_BASE_MS * 2 ** attempt;
  return Math.min(backoff + Math.random() * backoff * 0.5, RETRY_MAX_MS);
}

function wait(ms: number, signal: AbortSignal | null | undefined): Promise<void> {
  return new Promise((resolve, reject) => {
    if (signal?.aborted) {
      reject(signal.reason);
      return;
    }
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort);
      resolve();
    }, ms);
    function onAbort() {
      clearTimeout(timer);
      reject(signal?.reason);
    }
    signal?.addEventListener("abort", onAbort, { once: true });
  });
}

/**
 * `fetch` that absorbs congestion. A `429` or a `502`–`504`, or a network
 * failure, on a request that is safe to repeat is sent again after the wait
 * the API asked for (`Retry-After`) or a short backoff, up to
 * `RETRY_ATTEMPTS` sends in all. Everything else — every other status, a
 * request that cannot be repeated, an abort — passes straight through, so
 * the caller sees exactly what the API said. The dashboard's page load is a
 * burst of reads from one account; this is what keeps a burst that overran
 * the per-account limit from ever reaching the page as an error.
 */
export function retryingFetch(base: typeof globalThis.fetch): typeof globalThis.fetch {
  return async (input, init) => {
    const again = retriable(init);
    let attempt = 0;
    for (;;) {
      let response: Response | null = null;
      try {
        response = await base(input, init);
      } catch (cause) {
        if (!again || attempt + 1 >= RETRY_ATTEMPTS || init?.signal?.aborted) throw cause;
      }
      if (response !== null && (!again || !TRANSIENT_STATUSES.has(response.status))) {
        return response;
      }
      if (response !== null && attempt + 1 >= RETRY_ATTEMPTS) return response;
      attempt += 1;
      await wait(retryDelay(response, attempt - 1), init?.signal);
    }
  };
}

/**
 * A merchant client for the API this deployment is configured against. The
 * optional `fetcher` exists for the attachment uploader, which watches the
 * SDK's presigned PUT go by to report the scanning stage; everything else uses
 * the browser's fetch. Either way the client retries congestion
 * (`retryingFetch`).
 */
export function createMerchantClient(
  sessionToken: string,
  fetcher?: typeof globalThis.fetch,
): GumClient {
  return new GumClient({
    accessToken: sessionToken,
    baseUrl: config.apiUrl,
    fetch: retryingFetch(fetcher ?? ((...args) => globalThis.fetch(...args))),
  });
}

/**
 * Asks gatewayd to pregenerate this email's embedded wallet with Privy right
 * away, in parallel with Privy sending the sign-in code
 * (crates/gatewayd/src/pregenerated_wallet.rs), so it is often already there
 * by the time the code comes back. Fire-and-forget: sign-in's own explicit
 * wallet creation is the correctness guarantee regardless, so a failure here
 * — including a deployment with no `GUM_PRIVY_APP_SECRET` configured — is
 * never worth surfacing.
 */
export function pregenerateWallet(email: string): void {
  fetch(`${config.apiUrl}/v1/wallets/pregenerate`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ email }),
  }).catch(() => {});
}

/**
 * What to tell a merchant when Privy refuses a step of the sign-in. Privy's
 * errors are not typed for this, so the message is read for the one case
 * worth naming — a code that was wrong, expired, or already used — and
 * everything else is treated as transient.
 */
export function describeLoginError(cause: unknown): string {
  const message = cause instanceof Error ? cause.message : String(cause ?? "");
  if (/invalid|incorrect|expired|wrong|already used|no longer valid/i.test(message)) {
    return "That code is not valid. Check the email and try again.";
  }
  if (/too many|rate limit|try again later/i.test(message)) {
    return "Too many attempts. Wait a minute, then try again.";
  }
  return "Sign-in failed. Try again in a moment.";
}
