"use client";

import type { AccountMetadata } from "@payday/sdk";
import { ArrowUpRight, LogOut, RefreshCw } from "lucide-react";
import Image from "next/image";
import { useCallback, useEffect, useState, type ReactNode } from "react";
import { createPublicClient, erc20Abi, http } from "viem";
import { Button } from "@/components/ui/button";
import { CopyButton } from "@/components/ui/copy-button";
import { cn } from "@/lib/cn";
import { config, type PublicChain } from "@/lib/config";
import { explorerAddressUrl, formatBaseUnits } from "@/lib/format";
import { useMerchant } from "./session";

/**
 * The account: who is signed in, and the wallet that is theirs.
 *
 * Every account gets an embedded EVM wallet from Privy at its first sign-in.
 * It is the merchant's own — Payday never holds its key — and it is where
 * deposit requests settle unless one of an identity's saved wallets is
 * chosen instead. So it is shown here with what a merchant wants to know
 * about a wallet: the full address, ready to copy, and what is in it right
 * now on every network a payer can pay on. The wallet is an ordinary
 * account, so it has the same address on each of them.
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
  const wallet = account.wallet_address;

  return (
    <section aria-label="Account">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h2 className="font-heading text-[24px] leading-tight font-medium tracking-[-0.04em]">
            Account<span className="text-brand-yellow">.</span>
          </h2>
          <p className="mt-1.5 text-[13px] text-muted">
            View your account details: sign-up email, Payday wallet and its balance.
          </p>
        </div>
        <Button type="button" variant="ghost" size="sm" onClick={signOut}>
          <LogOut aria-hidden="true" className="size-3.5" />
          Sign out
        </Button>
      </div>

      <dl className="mt-6 grid gap-px overflow-hidden rounded-[12px] border border-line bg-line">
        <Row label="Signed in as">
          <span className="text-[14px]">{account.email ?? email ?? "—"}</span>
        </Row>

        <Row
          label="Payday wallet"
          hint="Deposit requests settle here unless you pick one of an identity's saved wallets."
        >
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
          <BalanceRow key={chain.id} chain={chain} wallet={wallet} />
        ))}
      </dl>
    </section>
  );
}

/** The wallet's USDC balance on one network, with its explorer link. */
function BalanceRow({ chain, wallet }: { chain: PublicChain; wallet: string | null }) {
  const balances = useBalances(wallet, chain);
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
          <span className="tabular inline-flex items-center gap-1.5 text-[15px] font-medium">
            {formatBaseUnits(balances.usdc, 6)}
            <Image
              src="/payment-icons/usdc.svg"
              width={64}
              height={64}
              alt=""
              className="size-4 shrink-0 rounded-full"
            />
            USDC
          </span>
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
  | { status: "ready"; usdc: bigint; reload: () => void };

type Reading = { outcome: "unavailable" } | { outcome: "ready"; usdc: bigint };

/**
 * The wallet's USDC balance on one chain, read straight from the public RPC
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
    client
      .readContract({
        address: chain.usdcAddress as `0x${string}`,
        abi: erc20Abi,
        functionName: "balanceOf",
        args: [address],
      })
      .then((usdc) => {
        if (!disposed) setSettled({ request, reading: { outcome: "ready", usdc } });
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
    ? { status: "ready", usdc: settled.reading.usdc, reload }
    : { status: "unavailable", reload };
}
