import type { RelayQuote, RelayOriginChain } from "@gum/sdk";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";
import { PAYER_WALLET, payment } from "@/test/fixtures";
import { RelayPay } from "./relay-pay";

const { useAccount } = vi.hoisted(() => ({ useAccount: vi.fn() }));
vi.mock("wagmi", () => ({
  useAccount: () => useAccount(),
  useConnect: () => ({ connect: vi.fn(), isPending: false, variables: undefined }),
  useConnectors: () => [],
}));

const { relay } = vi.hoisted(() => ({
  relay: { chains: vi.fn(), quote: vi.fn(), sent: vi.fn() },
}));
vi.mock("@/lib/payday", () => ({ payerClient: { relay } }));

/** Base, the origin chain the fixture quotes route from. */
const BASE: RelayOriginChain = {
  chain_id: "8453",
  name: "Base",
  native_symbol: "ETH",
  tokens: [{ currency: "USDC", symbol: "USDC", address: "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913", decimals: 6 }],
  explorer_url: "https://basescan.example",
  icon_url: null,
  rpc_url: null,
};
/** A second origin, so a test can switch the selection and strand a quote. */
const OPTIMISM: RelayOriginChain = { ...BASE, chain_id: "10", name: "Optimism" };

function quoteFor(origin: RelayOriginChain, amountIn: string): RelayQuote {
  return {
    id: `rli_${origin.chain_id}`,
    request_id: `0x${origin.chain_id.padStart(64, "0")}`,
    origin,
    origin_token: origin.tokens[0]!,
    amount_in: amountIn,
    amount_in_base_units: "1020000",
    amount_out: "1.00",
    amount_out_base_units: "1000000",
    relayer_fee_usd: "0.02",
    time_estimate_seconds: 5,
    expires_at: new Date(Date.now() + 60_000).toISOString(),
    steps: [
      {
        id: "approve",
        transaction: { chain_id: origin.chain_id, to: origin.tokens[0]!.address, data: "0x095ea7b3", value: "0", gas: "80000" },
      },
      {
        id: "deposit",
        transaction: { chain_id: origin.chain_id, to: "0x00000000000000000000000000000000000AbbBb", data: "0xe8017952", value: "0", gas: "80000" },
      },
    ],
  };
}

/** An EIP-1193 provider that behaves like a wallet already on Base. */
function fakeProvider() {
  const sent: Array<Record<string, unknown>> = [];
  const requests: string[] = [];
  let chainId = "0x2105";
  const accounts = [PAYER_WALLET];
  const provider = {
    request: vi.fn(async ({ method, params }: { method: string; params?: unknown }) => {
      requests.push(method);
      switch (method) {
        case "eth_chainId":
          return chainId;
        case "wallet_switchEthereumChain":
          return null;
        case "eth_accounts":
          return accounts;
        case "eth_call":
          return "0x000000000000000000000000000000000000000000000000000000003b9aca00";
        case "eth_sendTransaction":
          sent.push((params as Array<Record<string, unknown>>)[0]!);
          return `0x${"1".repeat(64)}`;
        case "eth_getTransactionReceipt":
          return { status: "0x1" };
        default:
          throw new Error(`unexpected ${method}`);
      }
    }),
  };
  return { provider, sent, requests, setChainId: (next: string) => { chainId = next; } };
}

function connected(provider: ReturnType<typeof fakeProvider>["provider"]) {
  useAccount.mockReturnValue({
    isConnected: true,
    address: PAYER_WALLET,
    connector: { getProvider: () => Promise.resolve(provider) },
  });
}

/** Opens the panel, chooses Base, and waits for its quote to be shown. */
async function chooseBase(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: /Pay from another network/ }));
  await user.click(screen.getByRole("button", { name: /Network/ }));
  await user.click(screen.getByRole("option", { name: /Base/ }));
  await screen.findByTestId("relay-quote");
}

beforeEach(() => {
  useAccount.mockReset();
  relay.chains.mockReset().mockResolvedValue({ chains: [BASE, OPTIMISM] });
  relay.quote.mockReset();
  relay.sent.mockReset().mockResolvedValue(payment());
  connected({ request: async () => null } as never);
});

describe("RelayPay", () => {
  it("ignores a quote that answers after the payer moved to another network", async () => {
    const user = userEvent.setup();
    // Base's quote is slow; Optimism's answers at once.
    let late: (value: RelayQuote) => void = () => {};
    const pending = new Promise<RelayQuote>((resolve) => { late = resolve; });
    relay.quote.mockImplementation((_id: string, chainId: string) =>
      chainId === "8453" ? pending : Promise.resolve(quoteFor(OPTIMISM, "1.03")),
    );
    render(<RelayPay payment={payment()} payerSession={null} onSent={vi.fn()} />);

    await user.click(screen.getByRole("button", { name: /Pay from another network/ }));
    await user.click(screen.getByRole("button", { name: /Network/ }));
    await user.click(screen.getByRole("option", { name: /Base/ }));
    // Switching to Optimism supersedes Base's still-pending request and its
    // quote is what shows…
    await user.click(screen.getByRole("button", { name: /Network/ }));
    await user.click(screen.getByRole("option", { name: /Optimism/ }));
    await waitFor(() => expect(screen.getByTestId("relay-quote")).toHaveTextContent("1.03"));

    // …and the slow Base quote, arriving afterwards, must not take the panel over.
    late(quoteFor(BASE, "1.02"));
    await waitFor(() => expect(screen.getByTestId("relay-quote")).toHaveTextContent("1.03"));
    expect(screen.getByTestId("relay-quote")).not.toHaveTextContent("1.02");
  });

  it("sends the quote's transactions once no matter how often the button is clicked", async () => {
    const user = userEvent.setup();
    const { provider, sent } = fakeProvider();
    connected(provider);
    relay.quote.mockResolvedValue(quoteFor(BASE, "1.02"));
    const onSent = vi.fn();
    render(<RelayPay payment={payment()} payerSession={null} onSent={onSent} />);

    await chooseBase(user);
    const pay = screen.getByRole("button", { name: /Pay USDC from Base/ });
    // Two clicks in the same tick: the second must meet the execution lock,
    // not a second run of the whole route.
    fireEvent.click(pay);
    fireEvent.click(pay);

    await waitFor(() => expect(onSent).toHaveBeenCalledTimes(1));
    // Approve and deposit — the second run would send each again.
    expect(sent).toHaveLength(2);
    expect(relay.sent).toHaveBeenCalledTimes(1);
  });

  it("asks the wallet for every transaction on the origin chain", async () => {
    const user = userEvent.setup();
    const { provider, sent } = fakeProvider();
    connected(provider);
    relay.quote.mockResolvedValue(quoteFor(BASE, "1.02"));
    render(<RelayPay payment={payment()} payerSession={null} onSent={vi.fn()} />);

    await chooseBase(user);
    fireEvent.click(screen.getByRole("button", { name: /Pay USDC from Base/ }));
    await waitFor(() => expect(sent).toHaveLength(2));
    // 8453 in hex: the origin chain, not whatever network the wallet happens
    // to be on after the switch.
    for (const transaction of sent) {
      expect(transaction.chainId).toBe("0x2105");
    }
  });

  it("stops before sending when the wallet has moved off the origin chain", async () => {
    const user = userEvent.setup();
    const { provider, sent, setChainId } = fakeProvider();
    connected(provider);
    relay.quote.mockResolvedValue(quoteFor(BASE, "1.02"));
    render(<RelayPay payment={payment()} payerSession={null} onSent={vi.fn()} />);

    await chooseBase(user);
    // The switch request "succeeds" but the wallet reports another chain
    // afterwards — a guard must catch it before any transaction is built.
    setChainId("0x1");
    fireEvent.click(screen.getByRole("button", { name: /Pay USDC from Base/ }));

    await waitFor(() => expect(screen.getByRole("alert")).toHaveTextContent(/different network/i));
    expect(sent).toHaveLength(0);
  });
});
