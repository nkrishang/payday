import { PaydayClient } from "@payday/sdk";
import { config } from "./config";

/**
 * Merchant dashboard identity.
 *
 * The dashboard signs in with a passwordless email OTP,
 * exchanged straight against the issuer's passwordless endpoints (Auth0 in
 * production, `payday-dev-identity` locally) with the dashboard's own client
 * ID. The resulting access token for the API audience is the session: the API
 * accepts it on the payment, customer, and attachment routes, so the browser
 * never holds or issues an API key.
 *
 * The token lives in memory and is mirrored to `sessionStorage` so a reload
 * within the tab does not force a new code. It is never written to
 * `localStorage`: a session ends with the tab, and Auth0 tokens expire in
 * minutes regardless.
 */

const PASSWORDLESS_OTP_GRANT = "http://auth0.com/oauth/grant-type/passwordless/otp";
const SESSION_KEY = "payday.dashboard.session";

/**
 * How long an emailed code stays usable, which the issuer decides and does not
 * publish: Auth0's passwordless connection is configured for this window in
 * `auth0/passwordless.tf`, and `payday-dev-identity` enforces the same one
 * locally. A page that offers to send another code must not do so while the
 * first still works, so it counts this down rather than guessing.
 */
export const EMAIL_OTP_LIFETIME_MS = 5 * 60 * 1000;

export interface MerchantSession {
  accessToken: string;
  /** Unix milliseconds after which the token is no longer sent. */
  expiresAt: number;
}

export class EmailOtpError extends Error {
  readonly code: string;
  readonly status: number;

  constructor(message: string, code: string, status: number) {
    super(message);
    this.name = "EmailOtpError";
    this.code = code;
    this.status = status;
  }
}

function required(value: string | undefined, name: string): string {
  if (!value) {
    throw new Error(`${name} is not set. Copy web/.env.example to web/.env.local and fill it in.`);
  }
  return value;
}

/**
 * Read lazily so the checkout, which never imports the dashboard, builds
 * without the dashboard variables. Literal `process.env.NEXT_PUBLIC_*` access
 * is what lets Next inline them; do not refactor into a lookup.
 */
function identity(): { issuer: string; clientId: string; audience: string } {
  const domain = required(process.env.NEXT_PUBLIC_AUTH0_DOMAIN, "NEXT_PUBLIC_AUTH0_DOMAIN");
  const issuer = (domain.includes("://") ? domain : `https://${domain}`).replace(/\/+$/, "");
  return {
    issuer,
    clientId: required(process.env.NEXT_PUBLIC_AUTH0_CLIENT_ID, "NEXT_PUBLIC_AUTH0_CLIENT_ID"),
    audience: required(process.env.NEXT_PUBLIC_AUTH0_AUDIENCE, "NEXT_PUBLIC_AUTH0_AUDIENCE"),
  };
}

let current: MerchantSession | null = null;

function storage(): Storage | null {
  try {
    return typeof sessionStorage === "undefined" ? null : sessionStorage;
  } catch {
    return null;
  }
}

function restore(): MerchantSession | null {
  const raw = storage()?.getItem(SESSION_KEY);
  if (!raw) return null;
  try {
    const parsed = JSON.parse(raw) as Partial<MerchantSession>;
    if (typeof parsed.accessToken === "string" && typeof parsed.expiresAt === "number") {
      return { accessToken: parsed.accessToken, expiresAt: parsed.expiresAt };
    }
  } catch {
    // A corrupt entry is treated as signed out.
  }
  return null;
}

export const merchantSession = {
  /** The live session, or null when there is none or it has expired. */
  get(now = Date.now()): MerchantSession | null {
    current ??= restore();
    if (current && current.expiresAt <= now) this.clear();
    return current;
  },
  set(session: MerchantSession): void {
    current = session;
    try {
      storage()?.setItem(SESSION_KEY, JSON.stringify(session));
    } catch {
      // Storage may be unavailable (private mode, quota); memory still holds it.
    }
  },
  clear(): void {
    current = null;
    try {
      storage()?.removeItem(SESSION_KEY);
    } catch {
      // Nothing to remove.
    }
  },
};

/**
 * The mailbox the session's token was issued for, from the identity claim both
 * Auth0's merchant action and `payday-dev-identity` set. Read straight off the
 * token rather than verified: nothing is authorized on it here — it only
 * greets the merchant and pre-fills their own address — and the API checks the
 * signature on every call it is sent with. Anything that is not a JWT (an e2e
 * stub token, say) simply has no mailbox.
 */
export function sessionEmail(accessToken: string): string | null {
  const payload = accessToken.split(".")[1];
  if (!payload) return null;
  try {
    const bytes = Uint8Array.from(
      atob(payload.replace(/-/g, "+").replace(/_/g, "/")),
      (character) => character.charCodeAt(0),
    );
    const claims = JSON.parse(new TextDecoder().decode(bytes)) as Record<string, unknown>;
    const email = claims["https://api.payday.sh/auth/email"];
    return typeof email === "string" && email ? email : null;
  } catch {
    // A token we cannot read is still a perfectly good bearer credential.
    return null;
  }
}

/**
 * A merchant client for the API this deployment is configured against. The
 * optional `fetcher` exists for the attachment uploader, which watches the
 * SDK's presigned PUT go by to report the scanning stage; everything else uses
 * the browser's fetch.
 */
export function createMerchantClient(
  accessToken: string,
  fetcher?: typeof globalThis.fetch,
): PaydayClient {
  return new PaydayClient({
    accessToken,
    baseUrl: config.apiUrl,
    ...(fetcher === undefined ? {} : { fetch: fetcher }),
  });
}

async function passwordless(
  fetcher: typeof globalThis.fetch,
  path: string,
  body: Record<string, string>,
): Promise<unknown> {
  const response = await fetcher(`${identity().issuer}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "application/json" },
    body: JSON.stringify(body),
  });
  const text = await response.text();
  let data: { error?: string; error_description?: string } | undefined;
  try {
    data = text ? JSON.parse(text) : undefined;
  } catch {
    data = undefined;
  }
  if (!response.ok) {
    throw new EmailOtpError(
      data?.error_description ?? data?.error ?? "Sign-in request failed",
      data?.error ??
        (response.status === 401 || response.status === 403 ? "invalid_grant" : "http_error"),
      response.status,
    );
  }
  return data;
}

/** Asks the issuer to email a one-time code to `email`. */
export async function startEmailOtp(
  email: string,
  fetcher: typeof globalThis.fetch = globalThis.fetch,
): Promise<void> {
  await passwordless(fetcher, "/passwordless/start", {
    client_id: identity().clientId,
    connection: "email",
    email,
    send: "code",
  });
}

/**
 * Exchanges the emailed code for an API access token and stores it as the
 * session. Throws `EmailOtpError` with code `invalid_grant` for a wrong,
 * expired, or reused code.
 */
export async function confirmEmailOtp(
  email: string,
  otp: string,
  fetcher: typeof globalThis.fetch = globalThis.fetch,
): Promise<MerchantSession> {
  const { clientId, audience } = identity();
  const data = (await passwordless(fetcher, "/oauth/token", {
    grant_type: PASSWORDLESS_OTP_GRANT,
    client_id: clientId,
    username: email,
    otp,
    realm: "email",
    audience,
    scope: "openid",
  })) as { access_token?: unknown; expires_in?: unknown } | undefined;
  if (typeof data?.access_token !== "string" || !data.access_token) {
    throw new EmailOtpError("Issuer returned no access token", "invalid_response", 200);
  }
  const expiresIn =
    typeof data.expires_in === "number" && data.expires_in > 0 ? data.expires_in : 300;
  const session = { accessToken: data.access_token, expiresAt: Date.now() + expiresIn * 1000 };
  merchantSession.set(session);
  return session;
}
