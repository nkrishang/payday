import type { UnlockedPayerPayment } from "@/lib/checkout-state";
import { truncateAddress } from "@/lib/format";

/**
 * The one warning worth interrupting for. Bridged USDC, a look-alike token, or
 * USDC on another network will not be credited and may be unrecoverable, and a
 * matching symbol in a wallet is not evidence of the right contract.
 */
export function AssetNotice({ payment }: { payment: UnlockedPayerPayment }) {
  return (
    <div className="rounded-[10px] border border-line bg-raised px-3.5 py-3">
      <p className="text-[13px] leading-relaxed text-muted">
        Send only native{" "}
        <span className="font-medium text-ink">{payment.token.symbol}</span> on{" "}
        <span className="font-medium text-ink">{payment.chain.name}</span>. A matching symbol is not
        enough — check the token contract in your wallet.
      </p>
      <dl className="mt-3 space-y-1.5 border-t border-line pt-3 text-[12px]">
        <div className="flex items-baseline justify-between gap-3">
          <dt className="text-faint">Network</dt>
          <dd className="tabular font-mono text-muted">
            {payment.chain.name} · {payment.chain.id}
          </dd>
        </div>
        <div className="flex items-baseline justify-between gap-3">
          <dt className="text-faint">Token contract</dt>
          <dd className="font-mono text-muted" title={payment.token.address}>
            {truncateAddress(payment.token.address, 10, 8)}
          </dd>
        </div>
      </dl>
    </div>
  );
}
