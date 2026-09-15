"use client";

import type { RelayOriginChain, RelayOriginToken, RelayQuote } from "@payday/sdk";
import type { PendingPayment, ReadyPayerDepositRequest } from "@/lib/checkout-state";
import { ArrowLeftRight, ChevronDown, Loader2 } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { encodeFunctionData, erc20Abi, hexToBigInt, numberToHex, type Hex } from "viem";
import { useAccount } from "wagmi";
import { Button } from "@/components/ui/button";
import { MenuSelect } from "@/components/ui/menu-select";
import { cn } from "@/lib/cn";
import { formatBaseUnits, formatDisplayAmount, truncateAddress } from "@/lib/format";
import { payerClient } from "@/lib/payday";
import { clearRelayReport, recordRelayBroadcast } from "@/lib/relay-send";
import { ConnectSheet } from "./connect-sheet";
import { walletErrorMessage } from "./wallet-errors";

/** A quote older than this is refreshed before it is sent. */
const QUOTE_FRESH_MS = 60_000;
/** How long an origin transaction is waited for before the page gives up watching it. */
const RECEIPT_TIMEOUT_MS = 5 * 60_000;
const RECEIPT_POLL_MS = 2_000;

/**
 * Paying from another network through Relay.
 *
 * The payer's stablecoin may sit on a chain the request is not on, and may
 * not even be the request's currency: USDC on Base can pay a USDT request on
 * Monad, with Relay swapping on the way and the payer carrying the spread.
 * Payday quotes
 * the route (the quote is pinned to the attested wallet, the payment
 * address, and exactly the amount still due), and this component sends the
 * quote's transactions from the connected wallet on the origin chain, then
 * reports the deposit so the indexer follows it.
 *
 * The origin chain is whichever of Relay's the payer picks, so it is not in
 * this app's wagmi configuration; the wallet is driven through its EIP-1193
 * provider directly: switch (or add, from the chain record Payday relays)
 * to the origin chain, send each step, wait for the receipt between steps.
 * The connected wallet must be the attested one: the quote was made for it,
 * and the origin transaction it sends is what the proof will name.
 */
export function RelayPay({
  payment,
  payerSession,
  onSent,
}: {
  payment: ReadyPayerDepositRequest;
  payerSession: string | null;
  onSent: (payment: PendingPayment) => void;
}) {
  const { address, isConnected, connector } = useAccount();
  const [open, setOpen] = useState(false);
  const [chains, setChains] = useState<RelayOriginChain[] | null>(null);
  // The origin is one (network, token) pair, keyed `chainId:address`.
  const [origin, setOrigin] = useState<string>("");
  const [quote, setQuote] = useState<{ quote: RelayQuote; at: number } | null>(null);
  const [quoting, setQuoting] = useState(false);
  const [stage, setStage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [connectOpen, setConnectOpen] = useState(false);
  const quoteGeneration = useRef(0);
  const originRef = useRef(origin);
  const paymentIdRef = useRef(payment.id);
  const executing = useRef(false);
  useEffect(() => {
    originRef.current = origin;
    paymentIdRef.current = payment.id;
  }, [origin, payment.id]);
  const following = payment.relay;
  const walletMatches =
    !isConnected || !address || address.toLowerCase() === payment.payer_wallet.toLowerCase();

  // The chains are read when the panel opens, once per page.
  useEffect(() => {
    if (!open || chains !== null) return;
    const controller = new AbortController();
    payerClient.relay
      .chains(payment.id, { signal: controller.signal, ...sessionOption(payerSession) })
      .then((listed) => setChains(listed.chains))
      .catch((cause) => {
        if (!controller.signal.aborted) setError(describe(cause));
      });
    return () => controller.abort();
  }, [open, chains, payment.id, payerSession]);

  const [originChainId, originToken] = splitOrigin(origin);
  const selected = chains?.find((chain) => chain.chain_id === originChainId) ?? null;
  const selectedToken: RelayOriginToken | null =
    selected?.tokens.find((token) => token.address.toLowerCase() === originToken) ?? null;

  const requestQuote = useCallback(
    async (originKey: string, signal?: AbortSignal) => {
      const [chainId, tokenAddress] = splitOrigin(originKey);
      const generation = ++quoteGeneration.current;
      const requestPaymentId = payment.id;
      setQuoting(true);
      setError(null);
      try {
        const fresh = await payerClient.relay.quote(payment.id, chainId, {
          originToken: tokenAddress,
          ...(signal ? { signal } : {}),
          ...sessionOption(payerSession),
        });
        if (generation !== quoteGeneration.current || signal?.aborted ||
          paymentIdRef.current !== requestPaymentId || originRef.current !== originKey ||
          fresh.origin.chain_id !== chainId ||
          fresh.origin_token.address.toLowerCase() !== tokenAddress) return null;
        setQuote({ quote: fresh, at: Date.now() });
        return fresh;
      } catch (cause) {
        if (generation === quoteGeneration.current && !signal?.aborted) {
          setError(describe(cause));
          setQuote(null);
        }
        return null;
      } finally {
        if (generation === quoteGeneration.current) setQuoting(false);
      }
    },
    [payment.id, payerSession],
  );

  // A new origin gets a quote at once, so the payer sees what it costs.
  useEffect(() => {
    if (!origin) return;
    const controller = new AbortController();
    void Promise.resolve().then(() => requestQuote(origin, controller.signal));
    return () => controller.abort();
  }, [origin, requestQuote]);

  const pay = useCallback(async () => {
    if (executing.current) return;
    executing.current = true;
    try {
      setError(null);
      if (!selected || !selectedToken) return;
      if (!isConnected || !connector) {
        setConnectOpen(true);
        return;
      }
      if (!walletMatches || !address) return;
      const provider = (await connector.getProvider()) as Eip1193;
      setStage(`Switching to ${selected.name}…`);
      await switchToChain(provider, selected);
      // The quote must be fresh at sending time: a stale one may no longer
      // be fillable, and the amount due may have moved.
      let current = quote && Date.now() - quote.at < QUOTE_FRESH_MS ? quote.quote : null;
      if (current === null) {
        setStage("Refreshing the quote…");
        current = await requestQuote(origin);
        if (current === null) return;
      }
      if (current.origin.chain_id !== selected.chain_id ||
        current.origin_token.address.toLowerCase() !== selectedToken.address.toLowerCase() ||
        current.steps.some((step) => step.transaction.chain_id !== selected.chain_id)) {
        throw new Error("The Relay quote does not match the selected origin network and token.");
      }
      setStage("Checking your balance…");
      const balance = await tokenBalance(provider, selectedToken.address, address);
      if (balance < BigInt(current.amount_in_base_units)) {
        setError(
          `This wallet holds ${formatBaseUnits(balance, selectedToken.decimals)} ${selectedToken.symbol} on ${selected.name}, less than the ${formatDisplayAmount(current.amount_in)} the route costs.`,
        );
        setStage(null);
        return;
      }
      let depositHash: Hex | null = null;
      for (const [index, step] of current.steps.entries()) {
        const last = index === current.steps.length - 1;
        setStage(
          step.id === "approve"
            ? `Approve ${selectedToken.symbol} in your wallet…`
            : `Confirm the deposit in your wallet…`,
        );
        await assertWalletContext(provider, selected.chain_id, address);
        const hash = await sendStep(provider, address, step.transaction);
        if (last) {
          depositHash = hash;
          break;
        }
        setStage("Waiting for the approval to confirm…");
        await waitForReceipt(provider, hash);
      }
      if (depositHash === null) return;
      // Reported at once: the indexer follows the intent from here, and the
      // page starts polling fast for the delivery on the request's chain.
      setStage("Relay is delivering your payment…");
      const pending: PendingPayment = {
        kind: "relay",
        intentId: current.id,
        originChainId: selected.chain_id,
        hash: depositHash,
      };
      recordRelayBroadcast(payment.id, {
        intentId: current.id,
        originChainId: selected.chain_id,
        hash: depositHash,
        at: Date.now(),
      });
      onSent(pending);
      // Best effort only: the session journal retries this idempotent report.
      void payerClient.relay.sent(payment.id, current.id, depositHash, sessionOption(payerSession))
        .then(() => clearRelayReport(payment.id))
        .catch(() => undefined);
    } catch (cause) {
      setError(walletErrorMessage(cause));
    } finally {
      setStage(null);
      executing.current = false;
    }
  }, [
    address,
    connector,
    isConnected,
    onSent,
    payerSession,
    payment.id,
    origin,
    quote,
    requestQuote,
    selected,
    selectedToken,
    walletMatches,
  ]);

  if (following && (following.status === "sent" || following.status === "filled")) {
    const from = chains?.find((chain) => chain.chain_id === following.origin_chain_id)?.name;
    return (
      <p
        role="status"
        className="mt-4 rounded-[10px] border border-line bg-raised px-3.5 py-3 text-[13px] leading-relaxed text-muted"
      >
        {following.status === "filled"
          ? `Relay delivered your payment on ${payment.chain.name}.`
          : `Relay is delivering your payment${from ? ` from ${from}` : ""} to ${payment.chain.name}; this usually takes a few seconds.`}
      </p>
    );
  }

  const busy = stage !== null || quoting;

  return (
    <div className="mt-4">
      <ConnectSheet open={connectOpen} onOpenChange={setConnectOpen} />
      <button
        type="button"
        onClick={() => setOpen((value) => !value)}
        aria-expanded={open}
        className="flex w-full items-center justify-between gap-3 rounded-[10px] border border-line px-3.5 py-3 text-left transition-colors hover:border-muted"
      >
        <span className="flex items-center gap-2.5">
          <ArrowLeftRight className="size-4 text-faint" />
          <span>
            <span className="block text-[14px] font-medium">Pay from another network</span>
            <span className="mt-0.5 block text-[12px] text-faint">
              Send USDC or USDT from a chain you hold it on; Relay delivers {payment.token.symbol}{" "}
              to {payment.chain.name}.
            </span>
          </span>
        </span>
        <ChevronDown className={cn("size-4 shrink-0 text-faint transition-transform", open && "rotate-180")} />
      </button>

      {open ? (
        <div className="mt-3 grid gap-3 rounded-[10px] border border-line bg-surface p-3.5">
          {chains === null && !error ? (
            <p className="text-[13px] text-muted">Loading networks…</p>
          ) : null}
          {chains !== null ? (
            <MenuSelect
              label="Network"
              value={origin}
              onChange={setOrigin}
              options={[
                { value: "", label: "Choose a network" },
                ...chains.flatMap((chain) =>
                  chain.tokens.map((token) => ({
                    value: originKey(chain.chain_id, token.address),
                    label: chain.name,
                    badge: chain.icon_url ? (
                      // eslint-disable-next-line @next/next/no-img-element
                      <img src={chain.icon_url} alt="" className="size-4 rounded-full" />
                    ) : undefined,
                    detail: token.symbol,
                  })),
                ),
              ]}
              // Locked only while transactions are in flight: a quote still
              // loading must not trap the payer on that network.
              disabled={stage !== null}
            />
          ) : null}

          {selected && selectedToken && quote && quote.quote.origin.chain_id === selected.chain_id &&
          quote.quote.origin_token.address.toLowerCase() === selectedToken.address.toLowerCase() ? (
            <p className="text-[13px] leading-relaxed text-muted" data-testid="relay-quote">
              You send{" "}
              <span className="tabular font-medium text-ink">
                {formatDisplayAmount(quote.quote.amount_in)} {selectedToken.symbol}
              </span>{" "}
              on {selected.name}; exactly {formatDisplayAmount(quote.quote.amount_out)}{" "}
              {payment.token.symbol} lands on{" "}
              {payment.chain.name}
              {quote.quote.relayer_fee_usd ? ` · about $${quote.quote.relayer_fee_usd} in fees` : ""}
              {quote.quote.time_estimate_seconds
                ? ` · about ${quote.quote.time_estimate_seconds}s`
                : ""}
              .
            </p>
          ) : selected && quoting ? (
            <p className="text-[13px] text-muted">Getting a quote…</p>
          ) : null}

          <Button size="lg" onClick={pay} disabled={!selected || busy || !walletMatches}>
            {busy ? <Loader2 className="size-4 animate-spin" /> : <ArrowLeftRight className="size-4" />}
            {stage ??
              (quoting
                ? "Getting a quote…"
                : !isConnected
                  ? "Connect wallet"
                  : selected && selectedToken
                    ? `Pay ${selectedToken.symbol} from ${selected.name}`
                    : "Choose a network")}
          </Button>

          {isConnected && !walletMatches ? (
            <p role="alert" className="text-center text-[13px] text-warning">
              This request is bound to{" "}
              <span className="font-mono">{truncateAddress(payment.payer_wallet)}</span>. Switch to
              that wallet: the route was quoted for it and only its payment counts.
            </p>
          ) : null}

          {error ? (
            <p role="alert" className="text-center text-[13px] text-danger">
              {error}
            </p>
          ) : null}

          <p className="text-[12px] leading-relaxed text-faint">
            Exactly the amount due arrives on {payment.chain.name}; the route&apos;s fee is added to
            what you send. If the request expires before Relay delivers, the funds return to your
            wallet on {payment.chain.name}.
          </p>
        </div>
      ) : null}
    </div>
  );
}

interface Eip1193 {
  request: (args: { method: string; params?: unknown[] }) => Promise<unknown>;
}

function sessionOption(payerSession: string | null): { payerSession?: string } {
  return payerSession ? { payerSession } : {};
}

function describe(cause: unknown): string {
  if (typeof cause === "object" && cause !== null && "message" in cause) {
    const message = (cause as { message: unknown }).message;
    if (typeof message === "string" && message) return message;
  }
  return "Something went wrong. Try again.";
}

/** Switch the wallet to `chain`, adding it from Relay's record when the wallet lacks it. */
async function switchToChain(provider: Eip1193, chain: RelayOriginChain): Promise<void> {
  const chainId = numberToHex(Number(chain.chain_id));
  const current = (await provider.request({ method: "eth_chainId" })) as string;
  if (hexToBigInt(current as Hex) === BigInt(chain.chain_id)) return;
  try {
    await provider.request({ method: "wallet_switchEthereumChain", params: [{ chainId }] });
  } catch (cause) {
    const code = (cause as { code?: number } | null)?.code;
    if (code !== 4902 || !chain.rpc_url) throw cause;
    await provider.request({
      method: "wallet_addEthereumChain",
      params: [
        {
          chainId,
          chainName: chain.name,
          rpcUrls: [chain.rpc_url],
          nativeCurrency: {
            name: chain.native_symbol ?? "ETH",
            symbol: chain.native_symbol ?? "ETH",
            decimals: 18,
          },
          ...(chain.explorer_url ? { blockExplorerUrls: [chain.explorer_url] } : {}),
        },
      ],
    });
    await provider.request({ method: "wallet_switchEthereumChain", params: [{ chainId }] });
  }
}

/** One `chainId:address` key per origin option, lowercased so quotes compare exactly. */
function originKey(chainId: string, tokenAddress: string): string {
  return `${chainId}:${tokenAddress.toLowerCase()}`;
}

function splitOrigin(key: string): [string, string] {
  const separator = key.indexOf(":");
  return separator === -1 ? [key, ""] : [key.slice(0, separator), key.slice(separator + 1)];
}

async function tokenBalance(provider: Eip1193, token: string, holder: string): Promise<bigint> {
  const data = encodeFunctionData({
    abi: erc20Abi,
    functionName: "balanceOf",
    args: [holder as Hex],
  });
  const result = (await provider.request({
    method: "eth_call",
    params: [{ to: token, data }, "latest"],
  })) as string;
  return result && result !== "0x" ? hexToBigInt(result as Hex) : 0n;
}

async function sendStep(
  provider: Eip1193,
  from: string,
  transaction: RelayQuote["steps"][number]["transaction"],
): Promise<Hex> {
  const hash = await provider.request({
    method: "eth_sendTransaction",
    params: [
      {
        from,
        chainId: numberToHex(BigInt(transaction.chain_id)),
        to: transaction.to,
        data: transaction.data,
        value: numberToHex(BigInt(transaction.value)),
        ...(transaction.gas ? { gas: numberToHex(BigInt(transaction.gas)) } : {}),
      },
    ],
  });
  return hash as Hex;
}

async function assertWalletContext(provider: Eip1193, chainId: string, account: string): Promise<void> {
  const currentChain = await provider.request({ method: "eth_chainId" }) as string;
  if (hexToBigInt(currentChain as Hex) !== BigInt(chainId)) {
    // "chain mismatch" is the phrase walletErrorMessage maps to the
    // switch-back instruction the payer needs.
    throw new Error("chain mismatch: the wallet is no longer on the origin network");
  }
  const accounts = await provider.request({ method: "eth_accounts" }) as string[];
  if (!accounts.some((candidate) => candidate.toLowerCase() === account.toLowerCase())) {
    throw new Error("account changed: the wallet is no longer holding the bound account");
  }
}

async function waitForReceipt(provider: Eip1193, hash: Hex): Promise<void> {
  const deadline = Date.now() + RECEIPT_TIMEOUT_MS;
  while (Date.now() < deadline) {
    const receipt = (await provider.request({
      method: "eth_getTransactionReceipt",
      params: [hash],
    })) as { status?: string } | null;
    if (receipt) {
      if (receipt.status === "0x0") throw new Error("The approval was reverted on-chain.");
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, RECEIPT_POLL_MS));
  }
  throw new Error("timed out waiting for the approval to confirm");
}
