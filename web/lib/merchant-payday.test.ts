import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  EmailOtpError,
  confirmEmailOtp,
  createMerchantClient,
  merchantSession,
  sessionEmail,
  startEmailOtp,
} from "./merchant-payday";

type Call = { url: string; init: RequestInit };

function mockFetch(handler: (url: string, init: RequestInit) => Response) {
  const calls: Call[] = [];
  const fetcher = vi.fn(async (input: string | URL | Request, init?: RequestInit) => {
    const url = String(input);
    calls.push({ url, init: init ?? {} });
    return handler(url, init ?? {});
  }) as unknown as typeof fetch;
  return { fetcher, calls };
}

const json = (body: unknown, status = 200) =>
  new Response(JSON.stringify(body), { status, headers: { "content-type": "application/json" } });

beforeEach(() => {
  vi.stubEnv("NEXT_PUBLIC_AUTH0_DOMAIN", "http://127.0.0.1:3001");
  vi.stubEnv("NEXT_PUBLIC_AUTH0_CLIENT_ID", "payday-dashboard-local");
  vi.stubEnv("NEXT_PUBLIC_AUTH0_AUDIENCE", "payday-api-local");
  merchantSession.clear();
});

afterEach(() => {
  vi.unstubAllEnvs();
  merchantSession.clear();
});

describe("startEmailOtp", () => {
  it("asks the issuer for a code with the dashboard client id", async () => {
    const mock = mockFetch(() => json({}));

    await startEmailOtp("merchant@example.com", mock.fetcher);

    expect(mock.calls[0]?.url).toBe("http://127.0.0.1:3001/passwordless/start");
    expect(mock.calls[0]?.init.method).toBe("POST");
    expect(JSON.parse(String(mock.calls[0]?.init.body))).toEqual({
      client_id: "payday-dashboard-local",
      connection: "email",
      email: "merchant@example.com",
      send: "code",
    });
  });

  it("accepts a bare Auth0 tenant domain and prefixes https", async () => {
    vi.stubEnv("NEXT_PUBLIC_AUTH0_DOMAIN", "payday.us.auth0.com/");
    const mock = mockFetch(() => json({}));

    await startEmailOtp("merchant@example.com", mock.fetcher);

    expect(mock.calls[0]?.url).toBe("https://payday.us.auth0.com/passwordless/start");
  });

  it("surfaces the issuer's error code and description", async () => {
    const mock = mockFetch(() =>
      json({ error: "bad.connection", error_description: "Connection is disabled" }, 400),
    );

    await expect(startEmailOtp("merchant@example.com", mock.fetcher)).rejects.toMatchObject({
      name: "EmailOtpError",
      code: "bad.connection",
      message: "Connection is disabled",
      status: 400,
    });
  });
});

describe("confirmEmailOtp", () => {
  it("exchanges the code for an API-audience token and stores the session", async () => {
    const mock = mockFetch(() =>
      json({ access_token: "eyJ.dash.token", token_type: "Bearer", expires_in: 300 }),
    );
    const before = Date.now();

    const session = await confirmEmailOtp("merchant@example.com", "123456", mock.fetcher);

    expect(mock.calls[0]?.url).toBe("http://127.0.0.1:3001/oauth/token");
    expect(JSON.parse(String(mock.calls[0]?.init.body))).toEqual({
      grant_type: "http://auth0.com/oauth/grant-type/passwordless/otp",
      client_id: "payday-dashboard-local",
      username: "merchant@example.com",
      otp: "123456",
      realm: "email",
      audience: "payday-api-local",
      scope: "openid",
    });
    expect(session.accessToken).toBe("eyJ.dash.token");
    expect(session.expiresAt).toBeGreaterThanOrEqual(before + 300_000);
    expect(merchantSession.get()).toEqual(session);
  });

  it("reports a wrong code as invalid_grant even when the issuer sends no body", async () => {
    const mock = mockFetch(() => new Response(null, { status: 401 }));

    await expect(
      confirmEmailOtp("merchant@example.com", "000000", mock.fetcher),
    ).rejects.toMatchObject({
      code: "invalid_grant",
      status: 401,
    });
    expect(merchantSession.get()).toBeNull();
  });

  it("refuses a success response without a token", async () => {
    const mock = mockFetch(() => json({ token_type: "Bearer" }));

    await expect(
      confirmEmailOtp("merchant@example.com", "123456", mock.fetcher),
    ).rejects.toBeInstanceOf(EmailOtpError);
  });
});

describe("merchantSession", () => {
  it("survives a reload within the tab but never touches localStorage", () => {
    merchantSession.set({ accessToken: "eyJ.dash.token", expiresAt: Date.now() + 60_000 });

    expect(sessionStorage.getItem("payday.dashboard.session")).toContain("eyJ.dash.token");
    expect(localStorage.length).toBe(0);
  });

  it("drops an expired token on read", () => {
    merchantSession.set({ accessToken: "eyJ.old.token", expiresAt: 1_000 });

    expect(merchantSession.get(2_000)).toBeNull();
    expect(sessionStorage.getItem("payday.dashboard.session")).toBeNull();
  });

  it("clears both memory and the tab mirror", () => {
    merchantSession.set({ accessToken: "eyJ.dash.token", expiresAt: Date.now() + 60_000 });
    merchantSession.clear();

    expect(merchantSession.get()).toBeNull();
    expect(sessionStorage.getItem("payday.dashboard.session")).toBeNull();
  });
});

describe("createMerchantClient", () => {
  it("binds the configured API origin and sends the access token as the bearer", async () => {
    const original = globalThis.fetch;
    const mock = mockFetch(() => json({ payments: [], next_cursor: null }));
    globalThis.fetch = mock.fetcher;
    try {
      await createMerchantClient("eyJ.dash.token").payments.list();
    } finally {
      globalThis.fetch = original;
    }

    expect(mock.calls[0]?.url).toBe("https://api.example.test/v1/payments");
    expect((mock.calls[0]?.init.headers as Record<string, string>).Authorization).toBe(
      "Bearer eyJ.dash.token",
    );
  });
});

describe("sessionEmail", () => {
  /** A JWT is three base64url segments; only the middle one is read. */
  const token = (claims: Record<string, unknown>) =>
    `header.${btoa(JSON.stringify(claims)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "")}.signature`;

  it("reads the mailbox out of the identity claim", () => {
    expect(sessionEmail(token({ "https://api.payday.sh/auth/email": "founder@acme.test" }))).toBe(
      "founder@acme.test",
    );
  });

  it("has no mailbox for a token without the claim, or for one that is not a JWT", () => {
    expect(sessionEmail(token({ sub: "email|abc" }))).toBeNull();
    expect(sessionEmail("stub-dashboard-token")).toBeNull();
    expect(sessionEmail("header.not-base64-json.signature")).toBeNull();
  });
});
