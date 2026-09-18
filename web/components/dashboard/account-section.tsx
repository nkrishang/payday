"use client";

import type { AccountMetadata } from "@payday/sdk";
import { useExportWallet, useUpdateEmail } from "@privy-io/react-auth";
import { ArrowUpRight, KeyRound, LogOut, RefreshCw } from "lucide-react";
import { CurrencyMark } from "@/components/ui/amount";
import Link from "next/link";
import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";
import { createPublicClient, erc20Abi, http } from "viem";
import { Button } from "@/components/ui/button";
import { CopyButton } from "@/components/ui/copy-button";
import { Input, Problem } from "@/components/ui/field";
import { describeError } from "@/lib/attachment-upload";
import { cn } from "@/lib/cn";
import { config, type PublicChain, type PublicToken } from "@/lib/config";
import { explorerAddressUrl, formatBaseUnits } from "@/lib/format";
import { EMAIL } from "./field-rules";
import { useMerchant } from "./session";
import { WithdrawPanel } from "./withdraw-panel";

/**
 * The account: who is signed in, the wallet that is theirs, and the flow that
 * pulls its settled balance out to a chain of the merchant's choice.
 *
 * Every account gets an embedded EVM wallet from Privy at its first sign-in.
 * It is the merchant's own — Gum never holds its key — and it is where
 * settled deposits accumulate. So it is shown here with what a merchant wants
 * to know about a wallet: the full address, ready to copy, and what is in it
 * right now on every network a payer can pay on. The wallet is an ordinary
 * account, so it has the same address on each of them.
 *
 * The sign-in email lives in the same view: it is how Gum reaches the
 * merchant, and the merchant changes it here rather than through support —
 * Privy emails the new address a code, and the address changes only once that
 * code is confirmed.
 */
export function AccountSection({
  account,
  onChanged,
}: {
  account: AccountMetadata;
  /** Re-reads the account; a wallet still being created may have arrived. */
  onChanged: () => void;
}) {
  const { email, signOut } = useMerchant();
  const { exportWallet } = useExportWallet();
  const wallet = account.wallet_address;
  // Bumped when a withdrawal leg lands, so every balance row reads again.
  const [balancesVersion, setBalancesVersion] = useState(0);
  const [chainBalances, setChainBalances] = useState<Record<number, BalanceSnapshot>>({});
  const onBalancesChanged = useCallback(() => setBalancesVersion((current) => current + 1), []);
  const reportBalance = useCallback((chainId: number, balance: BalanceSnapshot) => {
    setChainBalances((current) => ({ ...current, [chainId]: balance }));
  }, []);

  return (
    <section aria-label="Account">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h2 className="font-heading text-[24px] leading-tight font-medium tracking-[-0.04em]">
            Balance.
          </h2>
          <p className="mt-1.5 text-[13px] text-muted">
            Your sign-in email, Gum wallet, and withdrawing its balance.
          </p>
        </div>
        <Button type="button" variant="ghost" size="sm" onClick={signOut}>
          <LogOut aria-hidden="true" className="size-3.5" />
          Sign out
        </Button>
      </div>

      <dl className="mt-6 grid gap-px overflow-hidden rounded-[10px] border border-line bg-line">
        <Row label="Signed in as">
          <EmailRow email={account.email ?? email} onChanged={onChanged} />
        </Row>

        <Row label="Gum wallet">
          {wallet ? (
            <div className="flex min-w-0 flex-wrap items-center gap-2">
              <span className="min-w-0 font-mono text-[13px] break-all">{wallet}</span>
              <CopyButton value={wallet} label="wallet address" className="bg-raised" />
            </div>
          ) : (
            <div className="flex flex-wrap items-center gap-3">
              <span className="text-[13px] text-muted">
                Your wallet is still being created. It appears here within a moment.
              </span>
              <Button type="button" variant="secondary" size="sm" onClick={onChanged}>
                Check again
              </Button>
            </div>
          )}
        </Row>

        {config.chains.map((chain) => (
          <BalanceRow
            key={`${chain.id}-${balancesVersion}`}
            chain={chain}
            wallet={wallet}
            onBalance={reportBalance}
          />
        ))}

        <Row label="Wallet key">
          <div className="flex flex-wrap items-center gap-3">
            <Button
              type="button"
              variant="secondary"
              size="sm"
              disabled={!wallet}
              onClick={() => {
                if (wallet) void exportWallet({ address: wallet });
              }}
            >
              <KeyRound aria-hidden="true" className="size-3.5" />
              Export wallet key
            </Button>
            <span className="text-[12px] text-faint">
              Gum never sees your private key. An exported key is needed for{" "}
              <Link href="/docs/withdrawals" className="underline decoration-line underline-offset-2">
                withdrawing from a server
              </Link>
              .
            </span>
          </div>
        </Row>
      </dl>

      <div className="mt-4">
        <WithdrawPanel
          account={account}
          balances={chainBalances}
          onBalancesChanged={onBalancesChanged}
        />
      </div>
    </section>
  );
}

/** The wallet's balance in each configured stablecoin on one network, keyed by currency code. */
export type BalanceSnapshot =
  | { status: "loading" | "unavailable" }
  | { status: "ready"; tokens: Record<string, bigint> };

function BalanceRow({
  chain,
  wallet,
  onBalance,
}: {
  chain: PublicChain;
  wallet: string | null;
  onBalance: (chainId: number, balance: BalanceSnapshot) => void;
}) {
  const balances = useBalances(wallet, chain);
  const { status, tokens } =
    balances.status === "ready"
      ? { status: balances.status, tokens: balances.tokens }
      : { status: balances.status, tokens: null };
  const snapshot = useMemo<BalanceSnapshot>(
    () => (tokens === null ? { status } : { status: "ready", tokens }),
    [status, tokens],
  );
  useEffect(() => {
    onBalance(chain.id, snapshot);
  }, [chain.id, snapshot, onBalance]);
  const explorer = wallet ? explorerAddressUrl(chain.explorerUrl, wallet) : null;
  return (
    <Row label={`Balance on ${chain.name}`}>
      {!wallet ? (
        <span className="text-[13px] text-faint">—</span>
      ) : balances.status === "loading" ? (
        <span role="status" className="block h-5 w-40 animate-pulse rounded bg-raised" />
      ) : balances.status === "unavailable" ? (
        <div className="flex flex-wrap items-center gap-3">
          <span className="text-[13px] text-muted">
            Balance unavailable — {chain.name} did not answer.
          </span>
          <Button type="button" variant="ghost" size="sm" onClick={balances.reload}>
            <RefreshCw aria-hidden="true" className="size-3.5" />
            Retry
          </Button>
        </div>
      ) : (
        <div className="flex flex-wrap items-center gap-x-5 gap-y-1">
          {chain.tokens.map((token) => (
            <span
              key={token.currency}
              className="tabular inline-flex items-center gap-1.5 text-[15px] font-medium"
            >
              {formatBaseUnits(balances.tokens[token.currency] ?? 0n, token.decimals)}
              <CurrencyMark currency={token.currency} className="size-4" />
              {token.symbol}
            </span>
          ))}
          <button
            type="button"
            onClick={balances.reload}
            aria-label={`Refresh balance on ${chain.name}`}
            className="rounded-md p-1 text-faint transition-colors hover:bg-raised hover:text-ink"
          >
            <RefreshCw aria-hidden="true" className="size-3.5" />
          </button>
          {explorer ? (
            <a
              href={explorer}
              target="_blank"
              rel="noreferrer noopener"
              className="inline-flex items-center gap-1 rounded-md px-1.5 py-1 text-[12px] font-medium text-muted transition-colors hover:bg-raised hover:text-ink"
            >
              Explorer
              <ArrowUpRight aria-hidden="true" className="size-3.5" />
            </a>
          ) : null}
        </div>
      )}
    </Row>
  );
}

function Row({ label, hint, children }: { label: string; hint?: string; children: ReactNode }) {
  return (
    <div className="grid gap-1.5 bg-surface px-4 py-3.5 sm:grid-cols-[180px_minmax(0,1fr)] sm:gap-4">
      <dt className="text-[12px] font-medium text-muted">
        {label}
        {hint ? <span className="mt-0.5 block font-normal text-faint">{hint}</span> : null}
      </dt>
      <dd className={cn("min-w-0", hint ? "sm:pt-0.5" : "")}>{children}</dd>
    </div>
  );
}

type Balances =
  | { status: "loading" }
  | { status: "unavailable"; reload: () => void }
  | { status: "ready"; tokens: Record<string, bigint>; reload: () => void };

/**
 * The sign-in email, and changing it.
 *
 * Privy owns the address — it is how the merchant proves who they are — so
 * the change is Privy's flow: a code to the new address, and the address
 * changes only once that code is confirmed. Until then the old address is
 * still the signed-in one, and it is what this row shows.
 */
function EmailRow({ email, onChanged }: { email: string | null; onChanged: () => void }) {
  const { sendCode, verifyCode } = useUpdateEmail();
  const [mode, setMode] = useState<"closed" | "edit" | "code">("closed");
  const [draft, setDraft] = useState("");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);

  const valid = EMAIL.test(draft.trim());

  const close = () => {
    setMode("closed");
    setDraft("");
    setCode("");
    setProblem(null);
  };

  const send = async () => {
    if (!valid || busy) return;
    setBusy(true);
    setProblem(null);
    try {
      await sendCode({ newEmailAddress: draft.trim() });
      setMode("code");
    } catch (cause) {
      setProblem(describeError(cause));
    } finally {
      setBusy(false);
    }
  };

  const confirm = async () => {
    if (busy) return;
    setBusy(true);
    setProblem(null);
    try {
      await verifyCode({ code: code.trim() });
      // Privy has changed the address; the account's own copy follows. The
      // identity token rotates with it, so the next request carries the new
      // email already.
      onChanged();
      close();
    } catch (cause) {
      // A wrong or expired code is fixable in place; anything else —
      // including a stale token the refresh above cannot mend — sends the
      // merchant back to the start rather than stranding them mid-flow.
      setProblem(describeError(cause));
    } finally {
      setBusy(false);
    }
  };

  if (mode === "closed") {
    return (
      <div className="flex flex-wrap items-center gap-3">
        <span className="text-[14px]">{email ?? "—"}</span>
        <Button
          type="button"
          variant="secondary"
          size="sm"
          onClick={() => {
            setMode("edit");
            setProblem(null);
          }}
        >
          Change
        </Button>
      </div>
    );
  }

  return (
    <div className="grid gap-2">
      {mode === "edit" ? (
        <>
          <div className="flex max-w-[420px] flex-wrap items-center gap-2">
            <Input
              aria-label="New email address"
              type="email"
              autoComplete="email"
              placeholder="you@example.com"
              value={draft}
              onChange={(event) => setDraft(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") void send();
              }}
              className="min-w-[240px] flex-1"
            />
            <Button type="button" size="sm" disabled={!valid || busy} onClick={() => void send()}>
              Email a code
            </Button>
            <Button type="button" variant="ghost" size="sm" disabled={busy} onClick={close}>
              Cancel
            </Button>
          </div>
          <p className="text-[12px] text-faint">
            A code goes to the new address. Your sign-in email changes only once
            that code is confirmed.
          </p>
        </>
      ) : (
        <>
          <div className="flex max-w-[420px] flex-wrap items-center gap-2">
            <Input
              aria-label="Verification code"
              inputMode="numeric"
              autoComplete="one-time-code"
              placeholder="Code from your inbox"
              value={code}
              onChange={(event) => setCode(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") void confirm();
              }}
              className="min-w-[240px] flex-1"
            />
            <Button type="button" size="sm" disabled={!code.trim() || busy} onClick={() => void confirm()}>
              Confirm
            </Button>
            <Button
              type="button"
              variant="ghost"
              size="sm"
              disabled={busy || !valid}
              onClick={() => void send()}
            >
              Resend
            </Button>
            <Button type="button" variant="ghost" size="sm" disabled={busy} onClick={close}>
              Cancel
            </Button>
          </div>
          <p className="text-[12px] text-faint">
            A code was sent to <span className="text-ink">{draft.trim()}</span>. Confirm it to make
            that your sign-in email.
          </p>
        </>
      )}
      <Problem>{problem}</Problem>
    </div>
  );
}

type Reading = { outcome: "unavailable" } | { outcome: "ready"; tokens: Record<string, bigint> };

/**
 * The wallet's balance in each of a chain's stablecoins, read straight from the public RPC
 * this deployment is configured with for it — the same endpoint the
 * checkout's wallet button reads through. One shot per wallet and per
 * refresh; a chain that does not answer is reported, not retried forever.
 */
function useBalances(wallet: string | null, chain: PublicChain): Balances {
  const [version, setVersion] = useState(0);
  // Identifies one read; a result is current only if it answers this one, so
  // a refresh or a changed wallet reads as loading without an extra write.
  const request = `${wallet ?? ""} ${chain.id} ${version}`;
  const [settled, setSettled] = useState<{ request: string; reading: Reading } | null>(null);
  const reload = useCallback(() => setVersion((current) => current + 1), []);

  useEffect(() => {
    if (!wallet) return;
    let disposed = false;
    const address = wallet as `0x${string}`;
    const client = createPublicClient({
      transport: http(chain.rpcUrl, { retryCount: 1, timeout: 8_000 }),
    });
    Promise.all(
      chain.tokens.map((token: PublicToken) =>
        client
          .readContract({
            address: token.address as `0x${string}`,
            abi: erc20Abi,
            functionName: "balanceOf",
            args: [address],
          })
          .then((balance) => [token.currency, balance] as const),
      ),
    )
      .then((entries) => {
        if (!disposed) {
          setSettled({ request, reading: { outcome: "ready", tokens: Object.fromEntries(entries) } });
        }
      })
      .catch(() => {
        if (!disposed) setSettled({ request, reading: { outcome: "unavailable" } });
      });
    return () => {
      disposed = true;
    };
  }, [wallet, request, chain]);

  if (settled?.request !== request) return { status: "loading" };
  return settled.reading.outcome === "ready"
    ? { status: "ready", tokens: settled.reading.tokens, reload }
    : { status: "unavailable", reload };
}
