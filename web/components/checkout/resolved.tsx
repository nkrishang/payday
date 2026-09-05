import { AlertTriangle, ArrowUpRight, Check, Clock, Loader2 } from "lucide-react";
import type { CheckoutView, UnlockedPayerDepositRequest } from "@/lib/checkout-state";
import { cn } from "@/lib/cn";
import { config } from "@/lib/config";
import { explorerTxUrl, formatDisplayAmount, truncateHash } from "@/lib/format";

const icons = {
  settled: Check,
  deposited: Loader2,
  confirming: Loader2,
  closing: Clock,
  expired_empty: Clock,
  expired_funded: AlertTriangle,
  returned: AlertTriangle,
  attention: AlertTriangle,
} as const;

/**
 * Every state in which there is nothing left for the payer to send. The
 * requested amount is not repeated here: the document block above the outcome
 * already carries it, and this list is about what actually happened.
 */
export function Resolved({
  payment,
  view,
  pendingTxHash,
}: {
  payment: UnlockedPayerDepositRequest;
  view: CheckoutView;
  pendingTxHash: string | null;
}) {
  const Icon = icons[view.phase as keyof typeof icons] ?? Clock;
  const spinning = view.phase === "deposited" || view.phase === "confirming";

  const txHash = pendingTxHash ?? payment.settlement_tx_hash;
  const txUrl =
    (pendingTxHash ? explorerTxUrl(config.explorerUrl, pendingTxHash) : null) ??
    payment.settlement_explorer_url;

  const received = BigInt(payment.received_base_units) > 0n;

  return (
    <div className="px-6 py-10 text-center sm:px-8">
      <div
        className={cn(
          "mx-auto flex size-11 items-center justify-center rounded-full border",
          view.tone === "success" && "border-success/30 text-success",
          view.tone === "warning" && "border-warning/30 text-warning",
          (view.tone === "progress" || view.tone === "neutral") && "border-line text-muted",
        )}
      >
        <Icon className={cn("size-5", spinning && "animate-spin")} />
      </div>

      <h1 className="mt-5 text-[19px] font-semibold tracking-tight">{view.title}</h1>
      <p className="mx-auto mt-2.5 max-w-[38ch] text-[14px] leading-relaxed text-muted">
        {view.detail}
      </p>

      {received || txHash ? (
        <dl className="mt-7 space-y-2.5 border-t border-line pt-5 text-[13px]">
          {received ? (
            <div className="flex items-baseline justify-between gap-3">
              <dt className="text-faint">Received</dt>
              <dd className="tabular font-medium">
                {formatDisplayAmount(payment.received)} {payment.token.symbol}
              </dd>
            </div>
          ) : null}
          {txHash ? (
            <div className="flex items-baseline justify-between gap-3">
              <dt className="text-faint">Transaction</dt>
              <dd className="font-mono">
                {txUrl ? (
                  <a
                    href={txUrl}
                    target="_blank"
                    rel="noreferrer noopener"
                    className="inline-flex items-center gap-1 rounded transition-colors hover:text-ink"
                  >
                    {truncateHash(txHash)}
                    <ArrowUpRight className="size-3" />
                  </a>
                ) : (
                  truncateHash(txHash)
                )}
              </dd>
            </div>
          ) : null}
        </dl>
      ) : null}
    </div>
  );
}
