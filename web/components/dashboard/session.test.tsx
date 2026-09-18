import { type GumClient, GumError } from "@gum/sdk";
import { act, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { StrictMode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { MerchantProvider, useInvalidate, useResource } from "./session";

/**
 * The resource cache behind every dashboard read: one request per key
 * however many components ask, results that survive a remount and a token
 * rotation, transparent retries of congestion, and an error only once those
 * are spent.
 */

function Reader({ name, id = "thing" }: { name: string; id?: string }) {
  const thing = useResource(`thing:${id}`, (client) => client.customers.get(id));
  return (
    <div data-testid={name}>
      {thing.loading ? "loading " : ""}
      {thing.error ? `error:${thing.error} ` : ""}
      {thing.data ? `data:${(thing.data as { name: string }).name} ` : ""}
      {thing.detail ? `detail:${thing.detail}` : ""}
      <button type="button" onClick={thing.reload}>
        reload {name}
      </button>
    </div>
  );
}

function Invalidator({ prefix }: { prefix: string }) {
  const invalidate = useInvalidate();
  return (
    <button type="button" onClick={() => invalidate(prefix)}>
      invalidate {prefix}
    </button>
  );
}

function provider(get: (id: string) => Promise<unknown>, signOut = vi.fn()) {
  const client = { customers: { get } } as unknown as GumClient;
  const value = { client, accessToken: "eyJ.dash.token", email: "merchant@example.com", signOut };
  return { value, signOut };
}

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true });
});

afterEach(() => {
  vi.useRealTimers();
});

describe("useResource", () => {
  it("sends one request for a key however many components read it, even under StrictMode", async () => {
    const get = vi.fn().mockResolvedValue({ name: "Globex" });
    const { value } = provider(get);
    render(
      <StrictMode>
        <MerchantProvider value={value}>
          <Reader name="a" />
          <Reader name="b" />
          <Reader name="c" id="other" />
        </MerchantProvider>
      </StrictMode>,
    );

    await waitFor(() => expect(screen.getByTestId("a")).toHaveTextContent("data:Globex"));
    expect(screen.getByTestId("b")).toHaveTextContent("data:Globex");
    expect(screen.getByTestId("c")).toHaveTextContent("data:Globex");
    // Two keys, two requests: the double mount and the second reader join in.
    expect(get).toHaveBeenCalledTimes(2);
    expect(get).toHaveBeenCalledWith("thing");
    expect(get).toHaveBeenCalledWith("other");
  });

  it("keeps a result across a token rotation instead of fetching again", async () => {
    const get = vi.fn().mockResolvedValue({ name: "Globex" });
    const first = provider(get);
    const { rerender } = render(
      <MerchantProvider value={first.value}>
        <Reader name="a" />
      </MerchantProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("a")).toHaveTextContent("data:Globex"));

    // Same merchant, new client bearing the rotated token.
    const rotated = provider(get, first.signOut);
    rerender(
      <MerchantProvider value={{ ...rotated.value, accessToken: "eyJ.rotated.token" }}>
        <Reader name="a" />
      </MerchantProvider>,
    );
    expect(screen.getByTestId("a")).toHaveTextContent("data:Globex");
    expect(get).toHaveBeenCalledTimes(1);
  });

  it("serves a fresh result to a later reader and refreshes an old one behind it", async () => {
    const get = vi
      .fn()
      .mockResolvedValueOnce({ name: "Globex" })
      .mockResolvedValueOnce({ name: "Globex Corporation" });
    const { value } = provider(get);
    const { rerender } = render(
      <MerchantProvider value={value}>
        <Reader key="a" name="a" />
      </MerchantProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("a")).toHaveTextContent("data:Globex"));

    // Back within the freshness window: the cached result, no request.
    rerender(
      <MerchantProvider value={value}>
        <Reader key="b" name="b" />
      </MerchantProvider>,
    );
    expect(screen.getByTestId("b")).toHaveTextContent("data:Globex");
    expect(screen.getByTestId("b")).not.toHaveTextContent("loading");
    expect(get).toHaveBeenCalledTimes(1);

    // Long after: the old result shows at once while a fresh one is fetched.
    await act(async () => {
      await vi.advanceTimersByTimeAsync(11_000);
    });
    rerender(
      <MerchantProvider value={value}>
        <Reader key="c" name="c" />
      </MerchantProvider>,
    );
    expect(screen.getByTestId("c")).toHaveTextContent("data:Globex");
    await waitFor(() =>
      expect(screen.getByTestId("c")).toHaveTextContent("data:Globex Corporation"),
    );
    expect(get).toHaveBeenCalledTimes(2);
  });

  it("retries congestion quietly and settles once the API answers", async () => {
    const get = vi
      .fn()
      .mockRejectedValueOnce(
        new GumError("Per-account request limit exceeded", "rate_limited", 429, "req-1"),
      )
      .mockRejectedValueOnce(
        new GumError("Per-account request limit exceeded", "rate_limited", 429, "req-2"),
      )
      .mockResolvedValueOnce({ name: "Globex" });
    const { value } = provider(get);
    render(
      <MerchantProvider value={value}>
        <Reader name="a" />
      </MerchantProvider>,
    );

    // Still loading after the first refusal: nothing red reaches the page.
    await waitFor(() => expect(get).toHaveBeenCalledTimes(1));
    expect(screen.getByTestId("a")).toHaveTextContent("loading");
    expect(screen.getByTestId("a")).not.toHaveTextContent("error");

    await act(async () => {
      await vi.advanceTimersByTimeAsync(1_000);
    });
    await waitFor(() => expect(get).toHaveBeenCalledTimes(2));
    expect(screen.getByTestId("a")).not.toHaveTextContent("error");

    await act(async () => {
      await vi.advanceTimersByTimeAsync(2_000);
    });
    await waitFor(() => expect(screen.getByTestId("a")).toHaveTextContent("data:Globex"));
    expect(get).toHaveBeenCalledTimes(3);
  });

  it("reports a failure in the merchant's words once every retry is spent, and tries again on demand", async () => {
    const get = vi
      .fn()
      .mockRejectedValue(new GumError("upstream timeout", "internal_error", 503, "req-9"));
    const { value } = provider(get);
    render(
      <MerchantProvider value={value}>
        <Reader name="a" />
      </MerchantProvider>,
    );

    // The first send, then three automatic retries at 1s, 2s, 4s.
    for (const wait of [1_000, 2_000, 4_000]) {
      await waitFor(() => expect(screen.getByTestId("a")).toHaveTextContent("loading"));
      await act(async () => {
        await vi.advanceTimersByTimeAsync(wait);
      });
    }
    await waitFor(() =>
      expect(screen.getByTestId("a")).toHaveTextContent("error:Gum is temporarily unavailable."),
    );
    expect(screen.getByTestId("a")).toHaveTextContent("detail:upstream timeout (request req-9)");
    expect(screen.getByTestId("a")).not.toHaveTextContent("loading");
    expect(get).toHaveBeenCalledTimes(4);

    get.mockResolvedValueOnce({ name: "Globex" });
    await userEvent.click(screen.getByRole("button", { name: "reload a" }));
    await waitFor(() => expect(screen.getByTestId("a")).toHaveTextContent("data:Globex"));
    expect(screen.getByTestId("a")).not.toHaveTextContent("error");
  });

  it("does not retry a refusal that repeating cannot fix", async () => {
    const get = vi
      .fn()
      .mockRejectedValue(new GumError("Customer not found", "customer_not_found", 404, "req-4"));
    const { value } = provider(get);
    render(
      <MerchantProvider value={value}>
        <Reader name="a" />
      </MerchantProvider>,
    );
    await waitFor(() =>
      expect(screen.getByTestId("a")).toHaveTextContent("error:That record is no longer here."),
    );
    expect(get).toHaveBeenCalledTimes(1);
  });

  it("ends the session on a 401 rather than showing it", async () => {
    const get = vi.fn().mockRejectedValue(new GumError("expired", "identity_unauthorized", 401));
    const { value, signOut } = provider(get);
    render(
      <MerchantProvider value={value}>
        <Reader name="a" />
      </MerchantProvider>,
    );
    await waitFor(() => expect(signOut).toHaveBeenCalled());
    expect(screen.getByTestId("a")).not.toHaveTextContent("error");
  });

  it("refreshes every reader under a prefix when it is invalidated, and forgets the rest", async () => {
    const get = vi
      .fn()
      .mockImplementation((id: string) =>
        Promise.resolve({ name: `${id}#${get.mock.calls.length}` }),
      );
    const { value } = provider(get);
    const { rerender } = render(
      <MerchantProvider value={value}>
        <Reader name="a" />
        <Reader name="b" id="other" />
        <Invalidator prefix="thing:" />
      </MerchantProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("a")).toHaveTextContent("data:thing#1"));
    await waitFor(() => expect(screen.getByTestId("b")).toHaveTextContent("data:other#2"));

    // "b" leaves; its result is cached but nobody is showing it.
    rerender(
      <MerchantProvider value={value}>
        <Reader name="a" />
        <Invalidator prefix="thing:" />
      </MerchantProvider>,
    );
    await userEvent.click(screen.getByRole("button", { name: "invalidate thing:" }));
    // The reader still mounted fetches again; the one that left is forgotten.
    await waitFor(() => expect(screen.getByTestId("a")).toHaveTextContent("data:thing#3"));
    expect(get).toHaveBeenCalledTimes(3);

    // The reader that left was forgotten: coming back is a fresh fetch, not a stale hit.
    rerender(
      <MerchantProvider value={value}>
        <Reader name="a" />
        <Reader name="b" id="other" />
        <Invalidator prefix="thing:" />
      </MerchantProvider>,
    );
    await waitFor(() => expect(screen.getByTestId("b")).toHaveTextContent("data:other#4"));
  });
});
