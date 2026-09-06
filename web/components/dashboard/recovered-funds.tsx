import type { DepositRequest } from "@payday/sdk";
import { ArrowUpRight } from "lucide-react";
import { formatBaseUnits, formatDisplayAmount, truncateHash } from "@/lib/format";
import { formatDate } from "./labels";

/**
 * Balances that went back to the payer's attested wallet rather than to the
 * payout address, read from the deposit request itself: the overpayment remainder on
 * a settled request, the whole balance of a returned one, and every transfer
 * the indexer classified as late. Renders nothing when there is nothing.
 */
export function RecoveredFunds({ payment }: { payment: DepositRequest }) {
  const received = BigInt(payment.received_base_units);
  const amount = BigInt(payment.amount_base_units);
  const remainder = payment.status === "settled" && received > amount ? received - amount : 0n;
  const returned = payment.status === "returned" && received > 0n ? received : 0n;
  const late = payment.transfers.filter((transfer) => transfer.disposition === "late");

  if (remainder === 0n && returned === 0n && late.length === 0) return null;

  const symbol = payment.token.symbol;

  return (
    <section
      aria-labelledby="recovered-funds"
      className="min-w-0 rounded-[16px] border border-line bg-surface p-5"
    >
      <h2 id="recovered-funds" className="text-[13px] font-semibold tracking-tight">
        Returned to the payer
      </h2>
      <p className="mt-1 text-[12px] leading-relaxed text-muted">
        Sent back on-chain to the wallet the payer signed with
        {payment.payer_wallet ? (
          <>
            {" "}
            (<span className="font-mono break-all">{payment.payer_wallet}</span>)
          </>
        ) : null}
        , not to the payout address. Nothing is held by Payday.
      </p>
      <ul className="mt-4 divide-y divide-line text-[13px]">
        {remainder > 0n ? (
          <li className="flex items-baseline justify-between gap-3 py-2">
            <span className="text-muted">Overpayment remainder at settlement</span>
            <span className="tabular font-medium whitespace-nowrap">
              {formatBaseUnits(remainder, payment.token.decimals)} {symbol}
            </span>
          </li>
        ) : null}
        {returned > 0n ? (
          <li className="flex items-baseline justify-between gap-3 py-2">
            <span className="text-muted">Full balance, deadline passed before completion</span>
            <span className="tabular font-medium whitespace-nowrap">
              {formatDisplayAmount(payment.received)} {symbol}
            </span>
          </li>
        ) : null}
        {late.map((transfer) => (
          <li
            key={transfer.transaction_hash}
            className="flex items-baseline justify-between gap-3 py-2"
          >
            <span className="min-w-0 text-muted">
              Late transfer, {formatDate(transfer.timestamp)} ·{" "}
              {transfer.explorer_url ? (
                <a
                  href={transfer.explorer_url}
                  target="_blank"
                  rel="noreferrer noopener"
                  className="inline-flex items-center gap-1 font-mono text-ink"
                >
                  {truncateHash(transfer.transaction_hash)}
                  <ArrowUpRight className="size-3" />
                </a>
              ) : (
                <span className="font-mono text-ink">
                  {truncateHash(transfer.transaction_hash)}
                </span>
              )}{" "}
              · {transfer.collected ? "collected" : "awaiting collection"}
            </span>
            <span className="tabular font-medium whitespace-nowrap">
              {formatDisplayAmount(transfer.amount)} {symbol}
            </span>
          </li>
        ))}
      </ul>
    </section>
  );
}
