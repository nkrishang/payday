import { PaydayClient } from "@payday/sdk";
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
 * A merchant client for the API this deployment is configured against. The
 * optional `fetcher` exists for the attachment uploader, which watches the
 * SDK's presigned PUT go by to report the scanning stage; everything else uses
 * the browser's fetch.
 */
export function createMerchantClient(
  sessionToken: string,
  fetcher?: typeof globalThis.fetch,
): PaydayClient {
  return new PaydayClient({
    accessToken: sessionToken,
    baseUrl: config.apiUrl,
    ...(fetcher === undefined ? {} : { fetch: fetcher }),
  });
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
