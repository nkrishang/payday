import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  createMerchantClient,
  describeLoginError,
  privyAppId,
  RETRY_ATTEMPTS,
  retryingFetch,
} from "./merchant-gum";

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
  vi.stubEnv("NEXT_PUBLIC_PRIVY_APP_ID", "privy-test-app");
});

afterEach(() => {
  vi.unstubAllEnvs();
});

describe("privyAppId", () => {
  it("reads the configured app and fails loudly without one", () => {
    expect(privyAppId()).toBe("privy-test-app");
    vi.stubEnv("NEXT_PUBLIC_PRIVY_APP_ID", "");
    expect(() => privyAppId()).toThrow(/NEXT_PUBLIC_PRIVY_APP_ID/);
  });
});

describe("createMerchantClient", () => {
  it("sends the session token as the bearer credential to the configured API", async () => {
    const mock = mockFetch(() =>
      json({
        account_id: "acct",
        email: "merchant@example.com",
        wallet_address: "0x70997970C51812dc3A010C7d01b50e0d17dc79C8",
        key_hint: null,
        generation: 1,
        created_at: "2026-09-01T00:00:00Z",
        rotated_at: null,
        previous_key_expires_at: null,
        revoked_at: null,
      }),
    );
    const account = await createMerchantClient("privy.identity.token", mock.fetcher).account.get();
    expect(account.wallet_address).toBe("0x70997970C51812dc3A010C7d01b50e0d17dc79C8");
    expect(mock.calls[0]?.url).toBe("https://api.example.test/v1/account");
    expect(new Headers(mock.calls[0]?.init.headers).get("authorization")).toBe(
      "Bearer privy.identity.token",
    );
  });
});

describe("retryingFetch", () => {
  beforeEach(() => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  const limited = () =>
    new Response(
      JSON.stringify({
        error: { code: "rate_limited", message: "Per-account request limit exceeded" },
      }),
      { status: 429, headers: { "content-type": "application/json", "Retry-After": "1" } },
    );

  /** Answers each call in turn; the last answer repeats. */
  function sequence(...answers: (() => Response)[]) {
    let call = 0;
    return mockFetch(() => {
      const answer = answers[Math.min(call, answers.length - 1)] as () => Response;
      call += 1;
      return answer();
    });
  }

  it("sends a refused read again after the wait the API asked for", async () => {
    const mock = sequence(limited, () => json({ ok: true }));
    const response = retryingFetch(mock.fetcher)("https://api.example.test/v1/account", {
      method: "GET",
    });
    await vi.advanceTimersByTimeAsync(1_000);
    expect((await response).status).toBe(200);
    expect(mock.calls).toHaveLength(2);
  });

  it("gives up after the last attempt and hands back what the API said", async () => {
    const mock = mockFetch(() => limited());
    const response = retryingFetch(mock.fetcher)("https://api.example.test/v1/account", {});
    await vi.advanceTimersByTimeAsync(RETRY_ATTEMPTS * 1_000);
    expect((await response).status).toBe(429);
    expect(mock.calls).toHaveLength(RETRY_ATTEMPTS);
  });

  it("retries a dropped connection on a read, but never a write it cannot pin", async () => {
    const dropped = sequence(
      () => {
        throw new TypeError("Failed to fetch");
      },
      () => json({ ok: true }),
    );
    const read = retryingFetch(dropped.fetcher)("https://api.example.test/v1/issuers", {
      method: "GET",
    });
    await vi.advanceTimersByTimeAsync(2_000);
    expect((await read).status).toBe(200);
    expect(dropped.calls).toHaveLength(2);

    const refused = mockFetch(() => limited());
    const write = retryingFetch(refused.fetcher)("https://api.example.test/v1/customers", {
      method: "POST",
      body: "{}",
    });
    expect((await write).status).toBe(429);
    expect(refused.calls).toHaveLength(1);

    const pinned = sequence(limited, () => json({}, 201));
    const idempotent = retryingFetch(pinned.fetcher)(
      "https://api.example.test/v1/deposit-requests",
      { method: "POST", headers: { "Idempotency-Key": "k1" }, body: "{}" },
    );
    await vi.advanceTimersByTimeAsync(1_000);
    expect((await idempotent).status).toBe(201);
    expect(pinned.calls).toHaveLength(2);
  });

  it("passes every other refusal straight through", async () => {
    const mock = mockFetch(() => json({ error: { code: "invalid_request", message: "no" } }, 400));
    const response = await retryingFetch(mock.fetcher)("https://api.example.test/v1/account", {});
    expect(response.status).toBe(400);
    expect(mock.calls).toHaveLength(1);
  });
});

describe("describeLoginError", () => {
  it("names a bad code, a rate limit, and nothing else", () => {
    expect(describeLoginError(new Error("Invalid verification code"))).toMatch(/not valid/);
    expect(describeLoginError(new Error("Code has expired"))).toMatch(/not valid/);
    expect(describeLoginError(new Error("Too many requests"))).toMatch(/Too many attempts/);
    expect(describeLoginError(new Error("Failed to fetch"))).toMatch(/Try again in a moment/);
    expect(describeLoginError(undefined)).toMatch(/Try again in a moment/);
  });
});
