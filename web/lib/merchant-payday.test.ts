import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createMerchantClient, describeLoginError, privyAppId } from "./merchant-payday";

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

describe("describeLoginError", () => {
  it("names a bad code, a rate limit, and nothing else", () => {
    expect(describeLoginError(new Error("Invalid verification code"))).toMatch(/not valid/);
    expect(describeLoginError(new Error("Code has expired"))).toMatch(/not valid/);
    expect(describeLoginError(new Error("Too many requests"))).toMatch(/Too many attempts/);
    expect(describeLoginError(new Error("Failed to fetch"))).toMatch(/Try again in a moment/);
    expect(describeLoginError(undefined)).toMatch(/Try again in a moment/);
  });
});
