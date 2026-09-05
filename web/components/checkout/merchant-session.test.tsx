import { PaydayError } from "@payday/sdk";
import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { payerClient } from "@/lib/payday";
import { ClientSecretExchange, MerchantSessionGate } from "./merchant-session";

const SECRET = "cs_" + "b".repeat(43);

function renderExchange(payerSession: string | null = null) {
  const onStatus = vi.fn();
  const onSession = vi.fn();
  render(
    <ClientSecretExchange
      paymentId="dr_1"
      payerSession={payerSession}
      onStatus={onStatus}
      onSession={onSession}
    />,
  );
  return { onStatus, onSession };
}

describe("ClientSecretExchange", () => {
  beforeEach(() => {
    window.history.replaceState(null, "", "/pay/dr_1");
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("exchanges the fragment's secret once, hands the session up, and scrubs the URL", async () => {
    window.history.replaceState(null, "", `/pay/dr_1#cs=${SECRET}`);
    const exchange = vi.spyOn(payerClient.verification, "exchangeClientSecret").mockResolvedValue({
      payer_session: "pps_opened",
      expires_at: "2026-09-03T00:00:00Z",
      requirements: { email: "not_required", merchant_session: "approved", complete: true },
    });
    const { onStatus, onSession } = renderExchange();

    expect(onStatus).toHaveBeenCalledWith("exchanging");
    await waitFor(() => expect(onSession).toHaveBeenCalledWith("pps_opened"));
    expect(onStatus).toHaveBeenLastCalledWith("none");
    expect(exchange).toHaveBeenCalledTimes(1);
    expect(exchange.mock.calls[0]?.[0]).toBe("dr_1");
    expect(exchange.mock.calls[0]?.[1]).toBe(SECRET);
    // The secret left the address bar before the request was even answered.
    expect(window.location.hash).toBe("");
    expect(window.location.pathname).toBe("/pay/dr_1");
  });

  it("reports the bare link without calling the API", () => {
    const exchange = vi.spyOn(payerClient.verification, "exchangeClientSecret");
    const { onStatus, onSession } = renderExchange();
    expect(onStatus).toHaveBeenCalledWith("none");
    expect(exchange).not.toHaveBeenCalled();
    expect(onSession).not.toHaveBeenCalled();
  });

  it("does not spend the secret when this tab already holds a session", () => {
    window.history.replaceState(null, "", `/pay/dr_1#cs=${SECRET}`);
    const exchange = vi.spyOn(payerClient.verification, "exchangeClientSecret");
    const { onStatus } = renderExchange("pps_already");
    expect(onStatus).toHaveBeenCalledWith("none");
    expect(exchange).not.toHaveBeenCalled();
    expect(window.location.hash).toBe("");
  });

  it.each([
    ["client_secret_used", 409, "used"],
    ["client_secret_invalid", 401, "invalid"],
    ["deposit_request_not_payable", 410, "closed"],
    ["database_unavailable", 503, "unavailable"],
  ])("maps %s to the %s state", async (code, status, expected) => {
    window.history.replaceState(null, "", `/pay/dr_1#cs=${SECRET}`);
    vi.spyOn(payerClient.verification, "exchangeClientSecret").mockRejectedValue(
      new PaydayError(code, code, status, "req"),
    );
    const { onStatus, onSession } = renderExchange();
    await waitFor(() => expect(onStatus).toHaveBeenLastCalledWith(expected));
    expect(onSession).not.toHaveBeenCalled();
  });

  it("treats a network failure as the API being unavailable", async () => {
    window.history.replaceState(null, "", `/pay/dr_1#cs=${SECRET}`);
    vi.spyOn(payerClient.verification, "exchangeClientSecret").mockRejectedValue(
      new TypeError("fetch failed"),
    );
    const { onStatus } = renderExchange();
    await waitFor(() => expect(onStatus).toHaveBeenLastCalledWith("unavailable"));
  });
});

describe("MerchantSessionGate", () => {
  it("names the app for a bare link and offers no control", () => {
    render(<MerchantSessionGate issuerName="Tandem" status="none" />);
    expect(screen.getByText(/this deposit request opens from tandem/i)).toBeInTheDocument();
    expect(screen.queryByRole("button")).not.toBeInTheDocument();
  });

  it("distinguishes an opened link from an expired one", () => {
    const { rerender } = render(<MerchantSessionGate issuerName="Tandem" status="used" />);
    expect(screen.getByText(/already opened/i)).toBeInTheDocument();
    rerender(<MerchantSessionGate issuerName="Tandem" status="invalid" />);
    expect(screen.getByText(/expired or is not valid/i)).toBeInTheDocument();
    expect(screen.getByText(/go back to tandem/i)).toBeInTheDocument();
    rerender(<MerchantSessionGate issuerName="Tandem" status="closed" />);
    expect(screen.getByText(/closed/i)).toBeInTheDocument();
  });
});
