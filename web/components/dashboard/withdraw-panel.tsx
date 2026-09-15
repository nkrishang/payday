"use client";

import type { AccountMetadata, Withdrawal, WithdrawalLeg } from "@payday/sdk";
import { PaydayError } from "@payday/sdk";
import { useSignTypedData } from "@privy-io/react-auth";
import { ArrowRight, ArrowUpRight, Check, Loader2 } from "lucide-react";
import Link from "next/link";
import { useCallback, useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input, Problem } from "@/components/ui/field";
import { StatusDot } from "@/components/ui/status-dot";
import { describeError } from "@/lib/attachment-upload";
import type { CheckoutTone } from "@/lib/checkout-state";
import { cn } from "@/lib/cn";
import {
  CCTP_BURN_LIMIT_USDC,
  bridges,
  chainById,
  chainsFor,
  currencies,
  tokenOn,
  type PublicChain,
} from "@/lib/config";
import { explorerTxUrl, formatDisplayAmount, truncateAddress } from "@/lib/format";
import { checkLegAuthorization, privyTypedData } from "@/lib/withdrawal-authorization";
import { PAYOUT_ADDRESS } from "./field-rules";
import { formatDate } from "./labels";
import { useMerchant, useResource } from "./session";
import type { BalanceSnapshot } from "./account-section";

/**
 * Withdrawing: the Payday wallet's whole balance in one currency to one
 * address the merchant names: USDC from every network, since it bridges
 * through CCTP at 1:1; USDT from the destination network alone, since it
 * does not bridge and Payday never quotes the merchant a rate.
 *
 * Prepare, sign, submit, poll — the same four steps the API offers a server,
 * done here with the wallet Privy holds for the merchant. The API snapshots
 * the balances into legs, one per network with funds, each with an EIP-712
 * document to sign under that network's token contract (an EIP-3009
 * authorization).
 * The page checks each document against the leg before asking the wallet
 * for a signature, then hands the signatures back; from there Payday relays
 * and pays gas, and this panel only watches. A leg's signature fixes where
 * its funds may land, so nothing Payday does afterwards can redirect them.
 */

type Stage =
  | { kind: "idle" }
  | { kind: "destination" }
  | { kind: "review"; withdrawal: Withdrawal }
  | { kind: "signing"; withdrawal: Withdrawal; signed: number }
  | { kind: "tracking"; withdrawal: Withdrawal };

const POLL_MS = 10_000;
const OPEN: ReadonlySet<Withdrawal["status"]> = new Set(["awaiting_signature", "in_progress"]);

export function WithdrawPanel({
  account,
  balances,
  onBalancesChanged,
}: {
  account: AccountMetadata;
  balances: Record<number, BalanceSnapshot>;
  /** A leg completed: the balances shown above have moved. */
  onBalancesChanged: () => void;
}) {
  const { client, signOut } = useMerchant();
  const { signTypedData } = useSignTypedData();
  const recent = useResource("withdrawals:recent", (payday) =>
    payday.withdrawals.list({ limit: 5 }),
  );
  const [stage, setStage] = useState<Stage>({ kind: "idle" });
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [currency, setCurrency] = useState<string>(currencies()[0] ?? "USDC");
  const [destinationChain, setDestinationChain] = useState<number>(chainsFor(currency)[0]?.id ?? 0);
  const chooseCurrency = (next: string) => {
    setCurrency(next);
    if (!chainsFor(next).some((chain) => chain.id === destinationChain)) {
      setDestinationChain(chainsFor(next)[0]?.id ?? 0);
    }
  };
  const [destinationAddress, setDestinationAddress] = useState("");
  const wallet = account.wallet_address;

  const failed = useCallback(
    (cause: unknown) => {
      if (cause instanceof PaydayError && cause.status === 401) {
        signOut();
        return;
      }
      if (cause instanceof PaydayError && cause.code === "nothing_to_withdraw") {
        setFailure(
          bridges(currency)
            ? `Your Payday wallet holds no ${currency} on any network right now.`
            : `Your Payday wallet holds no ${currency} on ${chainById(destinationChain)?.name ?? "that network"}. ${currency} does not bridge: withdraw each network's balance to an address on that network.`,
        );
      } else if (cause instanceof PaydayError && cause.code === "withdrawal_in_progress") {
        setFailure("A withdrawal is already in progress. Continue it below, or cancel it first.");
        recent.reload();
      } else if (cause instanceof PaydayError && cause.code === "withdrawal_exceeds_bridge_limit") {
        setFailure(
          "A network balance exceeds Circle's 10,000,000 USDC bridge limit. Withdraw that network's balance to an address on the same network.",
        );
      } else {
        setFailure(describeError(cause));
      }
      setBusy(false);
    },
    [currency, destinationChain, recent, signOut],
  );

  // While a withdrawal is being relayed, follow it: one read every ten
  // seconds, until every leg has landed.
  const tracked = stage.kind === "tracking" ? stage.withdrawal : null;
  const trackedId = tracked?.id ?? null;
  const trackedOpen = tracked ? OPEN.has(tracked.status) : false;
  useEffect(() => {
    if (!trackedId || !trackedOpen) return;
    let disposed = false;
    const timer = setInterval(() => {
      client.withdrawals
        .get(trackedId)
        .then((next) => {
          if (disposed) return;
          setStage((current) => {
            if (current.kind !== "tracking" || current.withdrawal.id !== next.id) return current;
            const landed =
              next.legs.filter((leg) => leg.state === "completed").length >
              current.withdrawal.legs.filter((leg) => leg.state === "completed").length;
            if (landed) onBalancesChanged();
            return { kind: "tracking", withdrawal: next };
          });
        })
        .catch(() => {
          // A missed poll is only a missed poll; the next one reads again.
        });
    }, POLL_MS);
    return () => {
      disposed = true;
      clearInterval(timer);
    };
  }, [client, trackedId, trackedOpen, onBalancesChanged]);

  const addressValid = PAYOUT_ADDRESS.test(destinationAddress.trim());
  // Only a bridging currency has legs on other networks to guard.
  const bridgeBalances = bridges(currency)
    ? chainsFor(currency)
        .filter((chain) => chain.id !== destinationChain)
        .map((chain) => ({ chain, balance: balances[chain.id] }))
    : [];
  const balancesUnavailable = bridgeBalances.some(({ balance }) => !balance || balance.status !== "ready");
  const overBridgeLimit = bridgeBalances.find(
    ({ balance }) =>
      balance?.status === "ready" &&
      (balance.tokens[currency] ?? 0n) > CCTP_BURN_LIMIT_USDC * 1_000_000n,
  );
  const bridgeGuard = overBridgeLimit
    ? `${overBridgeLimit.chain.name} holds more than Circle's 10,000,000 USDC bridge limit. Choose ${overBridgeLimit.chain.name} as the destination.`
    : balancesUnavailable
      ? "Wait until every network balance is available before continuing."
      : null;

  const prepare = async () => {
    setBusy(true);
    setFailure(null);
    try {
      const withdrawal = await client.withdrawals.create(
        {
          currency,
          destination: { chain_id: String(destinationChain), address: destinationAddress.trim() },
        },
        crypto.randomUUID(),
      );
      // The signature binds to the API's destination, so the one it echoed
      // back must be the one the merchant typed. A mismatch is refused here,
      // before anything is signed.
      if (
        withdrawal.currency !== currency ||
        withdrawal.destination.address.toLowerCase() !== destinationAddress.trim().toLowerCase() ||
        Number(withdrawal.destination.chain.id) !== destinationChain
      ) {
        throw new Error(
          "The API returned a withdrawal for a different destination than the one entered. Nothing was signed; please contact support.",
        );
      }
      setStage({ kind: "review", withdrawal });
      setBusy(false);
    } catch (cause) {
      failed(cause);
    }
  };

  const sign = async (withdrawal: Withdrawal) => {
    if (!wallet) return;
    setBusy(true);
    setFailure(null);
    setStage({ kind: "signing", withdrawal, signed: 0 });
    let restoreWithdrawal = withdrawal;
    try {
      const authorizations: Array<{ leg_id: string; signature: string }> = [];
      const awaiting = withdrawal.legs.filter(
        (leg) => leg.state === "awaiting_signature" && leg.authorization,
      );
      for (const leg of awaiting) {
        const problem = checkLegAuthorization(withdrawal, leg);
        if (problem) {
          throw new Error(
            `${problem} Nothing was signed; please try again, and contact support if this persists.`,
          );
        }
      }
      for (const leg of awaiting) {
        if (!leg.authorization) continue;
        const { signature } = await signTypedData(
          privyTypedData(leg.authorization.typed_data),
          { uiOptions: { showWalletUIs: false }, address: wallet },
        );
        authorizations.push({ leg_id: leg.id, signature });
        setStage({ kind: "signing", withdrawal, signed: authorizations.length });
      }
      let next: Withdrawal;
      if (authorizations.length > 0) {
        try {
          next = await client.withdrawals.authorize(withdrawal.id, authorizations);
        } catch (cause) {
          const fresh = await client.withdrawals.get(withdrawal.id);
          restoreWithdrawal = fresh;
          throw cause;
        }
      } else {
        next = await client.withdrawals.get(withdrawal.id);
      }
      setStage({ kind: "tracking", withdrawal: next });
      recent.reload();
      setBusy(false);
    } catch (cause) {
      setStage({ kind: "review", withdrawal: restoreWithdrawal });
      failed(cause);
    }
  };

  const cancel = async (withdrawal: Withdrawal) => {
    setBusy(true);
    setFailure(null);
    try {
      await client.withdrawals.cancel(withdrawal.id);
      setStage({ kind: "idle" });
      recent.reload();
      setBusy(false);
    } catch (cause) {
      failed(cause);
    }
  };

  const reset = () => {
    setStage({ kind: "idle" });
    setFailure(null);
    setDestinationAddress("");
    recent.reload();
  };

  const resume = (withdrawal: Withdrawal) => {
    setFailure(null);
    setStage(
      withdrawal.status === "awaiting_signature"
        ? { kind: "review", withdrawal }
        : { kind: "tracking", withdrawal },
    );
  };

  return (
    <div className="overflow-hidden rounded-[12px] border border-line bg-surface" aria-label="Withdraw">
      <div className="flex flex-wrap items-center gap-3 px-4 py-3.5">
        <span className="text-[14px] font-medium">Withdraw</span>
        <span className="min-w-0 flex-1 text-[12px] text-faint">
          Move everything in your Payday wallet to your selected destination address and chain.
          USDC moves from every network; USDT moves from the destination network only. The
          wait-time depends on the destination chain.{" "}
          <span className="font-medium text-brand-green">No fees apply.</span>
        </span>
        {stage.kind === "idle" ? (
          <Button
            type="button"
            variant="secondary"
            size="sm"
            disabled={!wallet}
            onClick={() => {
              setFailure(null);
              setStage({ kind: "destination" });
            }}
          >
            Withdraw
          </Button>
        ) : null}
      </div>

      {stage.kind === "destination" ? (
        <form
          className="dash-step border-t border-line px-4 pt-4 pb-5"
          onSubmit={(event) => {
            event.preventDefault();
            if (addressValid && !busy && !bridgeGuard) void prepare();
          }}
        >
          {currencies().length > 1 ? (
            <fieldset>
              <legend className="text-[11px] font-medium tracking-[0.14em] text-faint uppercase">
                Currency
              </legend>
              <div className="mt-2 grid gap-2 sm:grid-cols-3" role="radiogroup" aria-label="Currency">
                {currencies().map((option) => (
                  <CurrencyChoice
                    key={option}
                    currency={option}
                    checked={currency === option}
                    onSelect={() => chooseCurrency(option)}
                    disabled={busy}
                  />
                ))}
              </div>
            </fieldset>
          ) : null}
          <fieldset className={currencies().length > 1 ? "mt-4" : undefined}>
            <legend className="text-[11px] font-medium tracking-[0.14em] text-faint uppercase">
              Withdraw to
            </legend>
            <div className="mt-2 grid gap-2 sm:grid-cols-3" role="radiogroup" aria-label="Withdraw to">
              {chainsFor(currency).map((chain) => (
                <NetworkChoice
                  key={chain.id}
                  chain={chain}
                  currency={currency}
                  checked={destinationChain === chain.id}
                  onSelect={() => setDestinationChain(chain.id)}
                  disabled={busy}
                />
              ))}
            </div>
          </fieldset>
          <label className="mt-4 block">
            <span className="text-[11px] font-medium tracking-[0.14em] text-faint uppercase">
              Destination address
            </span>
            <Input
              className="mt-2 font-mono"
              value={destinationAddress}
              onChange={(event) => setDestinationAddress(event.target.value)}
              placeholder="0x…"
              autoComplete="off"
              spellCheck={false}
              disabled={busy}
              aria-invalid={destinationAddress.length > 0 && !addressValid}
            />
          </label>
          {destinationAddress.length > 0 && !addressValid ? (
            <p role="alert" className="mt-1.5 text-[12px] text-danger">
              Not a valid address: 0x and 40 hex characters.
            </p>
          ) : (
            <p className="mt-1.5 text-[12px] text-faint">This cannot be changed later.</p>
          )}
          {bridgeGuard ? (
            <p role="alert" className="mt-2 text-[12px] text-danger">{bridgeGuard}</p>
          ) : null}
          <div className="mt-4 flex flex-wrap items-center gap-3">
            <Button type="submit" disabled={busy || !addressValid || Boolean(bridgeGuard)}>
              {busy ? <Loader2 className="size-4 animate-spin" /> : null}
              Continue
            </Button>
            <Button type="button" variant="ghost" onClick={reset} disabled={busy}>
              Cancel
            </Button>
          </div>
        </form>
      ) : null}

      {stage.kind === "review" || stage.kind === "signing" ? (
        <div className="dash-step border-t border-line px-4 pt-4 pb-5">
          <p className="text-[13px] font-medium">
            {stage.withdrawal.legs.length === 1 ? "One leg" : `${stage.withdrawal.legs.length} legs`}{" "}
            to {stage.withdrawal.destination.chain.name}
          </p>
          <p className="mt-1 text-[12.5px] text-muted">
            Your wallet signs one authorization per network. Payday relays them and pays the gas;
            each signature only permits that leg&apos;s funds to reach{" "}
            <span className="font-mono">{truncateAddress(stage.withdrawal.destination.address)}</span>.
          </p>
          <LegList withdrawal={stage.withdrawal} />
          <div className="mt-4 flex flex-wrap items-center gap-3">
            <Button type="button" onClick={() => void sign(stage.withdrawal)} disabled={busy}>
              {busy ? <Loader2 className="size-4 animate-spin" /> : null}
              {stage.kind === "signing"
                ? `Signing ${Math.min(stage.signed + 1, stage.withdrawal.legs.length)} of ${stage.withdrawal.legs.length}…`
                : "Sign and withdraw"}
            </Button>
            <Button
              type="button"
              variant="ghost"
              onClick={() => void cancel(stage.withdrawal)}
              disabled={busy}
            >
              Cancel withdrawal
            </Button>
          </div>
        </div>
      ) : null}

      {stage.kind === "tracking" ? (
        <div
          className={cn(
            "dash-step border-t border-line px-4 pt-4 pb-5",
            stage.withdrawal.status === "completed" && "bg-success/[0.06]",
          )}
        >
          <p className="flex items-center gap-1.5 text-[13px] font-medium">
            {stage.withdrawal.status === "completed" ? (
              <Check className="size-4 text-success" />
            ) : stage.withdrawal.status === "in_progress" ? (
              <Loader2 className="size-4 animate-spin text-muted" />
            ) : null}
            {trackingHeadline(stage.withdrawal)}
          </p>
          {stage.withdrawal.status === "in_progress" &&
          stage.withdrawal.legs.some(
            (leg) => leg.kind === "bridge" && !["completed", "failed"].includes(leg.state),
          ) ? (
            <p className="mt-1 text-[12.5px] text-muted">
              Bridged legs wait for Circle to attest the burn: seconds from Monad, about twenty
              minutes from Base or Arbitrum. You can leave this page; it carries on without you.
            </p>
          ) : null}
          <LegList withdrawal={stage.withdrawal} />
          <div className="mt-4 flex justify-end">
            <Button type="button" variant="secondary" size="sm" onClick={reset}>
              {OPEN.has(stage.withdrawal.status) ? "Close" : "Done"}
            </Button>
          </div>
        </div>
      ) : null}

      {failure ? (
        <div className="border-t border-line px-4 py-3">
          <Problem>{failure}</Problem>
        </div>
      ) : null}

      {stage.kind === "idle" && recent.data && recent.data.withdrawals.length > 0 ? (
        <ul className="border-t border-line" aria-label="Recent withdrawals">
          {recent.data.withdrawals.map((withdrawal) => (
            <li
              key={withdrawal.id}
              className="flex flex-wrap items-center gap-3 border-b border-line px-4 py-2.5 text-[12.5px] last:border-b-0"
            >
              <StatusDot tone={withdrawalTone(withdrawal.status)} pulse={OPEN.has(withdrawal.status)} />
              <span className="font-medium">{withdrawalLabel(withdrawal.status)}</span>
              <span className="min-w-0 flex-1 truncate text-faint">
                {withdrawal.legs.map((leg) => `${formatDisplayAmount(leg.amount)} ${leg.token.symbol} from ${leg.source_chain.name}`).join(" · ")}{" "}
                → {withdrawal.destination.chain.name} · {formatDate(withdrawal.created_at)}
              </span>
              {OPEN.has(withdrawal.status) ? (
                <Button type="button" variant="ghost" size="sm" onClick={() => resume(withdrawal)}>
                  {withdrawal.status === "awaiting_signature" ? "Continue" : "Track"}
                  <ArrowRight aria-hidden="true" className="size-3.5" />
                </Button>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}

function CurrencyChoice({
  currency,
  checked,
  onSelect,
  disabled,
}: {
  currency: string;
  checked: boolean;
  onSelect: () => void;
  disabled: boolean;
}) {
  return (
    <button
      type="button"
      role="radio"
      aria-checked={checked}
      disabled={disabled}
      onClick={onSelect}
      className={cn(
        "flex w-full items-center justify-between gap-3 rounded-[10px] border px-3.5 py-3 text-left transition-colors",
        checked ? "border-ink bg-raised" : "border-line hover:border-muted disabled:opacity-60",
      )}
    >
      <span className="min-w-0">
        <span className="block text-[14px] font-medium">{currency}</span>
        <span className="mt-0.5 block text-[12px] text-faint">
          {bridges(currency) ? "from every network" : "from the destination network only"}
        </span>
      </span>
      <span
        aria-hidden
        className={cn(
          "flex size-5 shrink-0 items-center justify-center rounded-full border",
          checked ? "border-ink bg-ink text-surface" : "border-line",
        )}
      >
        {checked ? <Check className="size-3" /> : null}
      </span>
    </button>
  );
}

function NetworkChoice({
  chain,
  currency,
  checked,
  onSelect,
  disabled,
}: {
  chain: PublicChain;
  currency: string;
  checked: boolean;
  onSelect: () => void;
  disabled: boolean;
}) {
  const symbol = tokenOn(chain, currency)?.symbol ?? currency;
  const reach = !bridges(currency)
    ? " · this network's balance only"
    : chain.cctp
      ? ""
      : " · same-network only";
  return (
    <button
      type="button"
      role="radio"
      aria-checked={checked}
      disabled={disabled}
      onClick={onSelect}
      className={cn(
        "flex w-full items-center justify-between gap-3 rounded-[10px] border px-3.5 py-3 text-left transition-colors",
        checked ? "border-ink bg-raised" : "border-line hover:border-muted disabled:opacity-60",
      )}
    >
      <span className="min-w-0">
        <span className="block text-[14px] font-medium">{chain.name}</span>
        <span className="mt-0.5 block text-[12px] text-faint">
          {symbol}
          {reach}
        </span>
      </span>
      <span
        aria-hidden
        className={cn(
          "flex size-5 shrink-0 items-center justify-center rounded-full border",
          checked ? "border-ink bg-ink text-surface" : "border-line",
        )}
      >
        {checked ? <Check className="size-3" /> : null}
      </span>
    </button>
  );
}

function LegList({ withdrawal }: { withdrawal: Withdrawal }) {
  return (
    <ul className="mt-3 divide-y divide-line rounded-[10px] border border-line" aria-label="Legs">
      {withdrawal.legs.map((leg) => (
        <LegRow key={leg.id} leg={leg} withdrawal={withdrawal} />
      ))}
    </ul>
  );
}

function LegRow({ leg, withdrawal }: { leg: WithdrawalLeg; withdrawal: Withdrawal }) {
  const source = chainById(leg.source_chain.id);
  const destination = chainById(withdrawal.destination.chain.id);
  const links: Array<{ label: string; href: string }> = [];
  const link = (label: string, chain: PublicChain | null, hash: string | null) => {
    const href = chain && hash ? explorerTxUrl(chain.explorerUrl, hash) : null;
    if (href) links.push({ label, href });
  };
  link("Transfer", source, leg.transfer_tx_hash);
  link("Burn", source, leg.burn_tx_hash);
  link("Mint", destination, leg.mint_tx_hash);
  return (
    <li className="flex flex-wrap items-center gap-x-4 gap-y-1 px-3.5 py-2.5 text-[13px]">
      <StatusDot tone={legTone(leg.state)} pulse={!["completed", "failed", "expired", "cancelled", "awaiting_signature"].includes(leg.state)} />
      <span className="tabular font-medium">
        {formatDisplayAmount(leg.amount)} {leg.token.symbol}
      </span>
      <span className="text-muted">
        {leg.kind === "bridge"
          ? `${leg.source_chain.name} → ${withdrawal.destination.chain.name} via CCTP`
          : `on ${leg.source_chain.name}`}
      </span>
      <span className="min-w-0 flex-1 text-[12px] text-faint">{legLabel(leg)}</span>
      {links.map((item) => (
        <a
          key={item.label}
          href={item.href}
          target="_blank"
          rel="noreferrer noopener"
          className="inline-flex items-center gap-1 rounded-md px-1.5 py-1 text-[12px] font-medium text-muted transition-colors hover:bg-raised hover:text-ink"
        >
          {item.label}
          <ArrowUpRight aria-hidden="true" className="size-3.5" />
        </a>
      ))}
      {leg.kind === "bridge" && leg.state === "awaiting_signature" && leg.authorization?.forwarder ? (
        <span className="basis-full text-[11.5px] text-faint">
          Pays Payday&apos;s forwarder{" "}
          <span className="font-mono">{truncateAddress(leg.authorization.forwarder)}</span>, which can
          only burn towards your address.{" "}
          <Link href="/docs/withdrawals" className="underline decoration-line underline-offset-2">
            How this works
          </Link>
        </span>
      ) : null}
    </li>
  );
}

function legLabel(leg: WithdrawalLeg): string {
  switch (leg.state) {
    case "awaiting_signature":
      return "Awaiting your signature";
    case "authorized":
      return "Signed · queued for relay";
    case "relaying":
      return leg.kind === "bridge" ? "Burning" : "Sending";
    case "burned":
      return "Burned · waiting for Circle's attestation";
    case "attested":
      return "Attested · queued to mint";
    case "minting":
      return "Minting";
    case "completed":
      return "Done";
    case "failed":
      return leg.failure_reason ? `Failed: ${leg.failure_reason}` : "Failed";
    case "expired":
      return "Expired unsigned";
    case "cancelled":
      return "Cancelled";
  }
}

function legTone(state: WithdrawalLeg["state"]): CheckoutTone {
  switch (state) {
    case "completed":
      return "success";
    case "failed":
    case "expired":
      return "warning";
    case "awaiting_signature":
    case "cancelled":
      return "neutral";
    default:
      return "progress";
  }
}

function withdrawalTone(status: Withdrawal["status"]): CheckoutTone {
  switch (status) {
    case "completed":
      return "success";
    case "failed":
      return "warning";
    case "cancelled":
      return "neutral";
    default:
      return "progress";
  }
}

function withdrawalLabel(status: Withdrawal["status"]): string {
  switch (status) {
    case "awaiting_signature":
      return "Awaiting signature";
    case "in_progress":
      return "In progress";
    case "completed":
      return "Completed";
    case "failed":
      return "Failed";
    case "cancelled":
      return "Cancelled";
  }
}

function trackingHeadline(withdrawal: Withdrawal): string {
  switch (withdrawal.status) {
    case "completed":
      return `Withdrawn to ${withdrawal.destination.chain.name}.`;
    case "failed":
      return "The withdrawal did not complete.";
    case "cancelled":
      return "Withdrawal cancelled.";
    case "awaiting_signature":
      return "Waiting for your signature.";
    case "in_progress":
      return "Withdrawing…";
  }
}
