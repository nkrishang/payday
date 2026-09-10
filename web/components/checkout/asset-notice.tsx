import type { ReadyPayerDepositRequest } from "@/lib/checkout-state";
import { chainById } from "@/lib/config";
import { explorerTokenUrl, truncateAddress } from "@/lib/format";

/**
 * The one warning worth interrupting for: only native USDC on the chain the
 * payer chose will be credited. The token contract is linked rather than
 * spelled out in prose — the explorer's page for it is the check that matters.
 */
export function AssetNotice({ payment }: { payment: ReadyPayerDepositRequest }) {
  const explorer = chainById(payment.chain.id)?.explorerUrl ?? null;
  const tokenExplorer = explorerTokenUrl(explorer, payment.token.address);
  return (
    <div className="rounded-[10px] border border-line bg-raised px-3.5 py-3">
      <p className="text-[13px] leading-relaxed text-muted">
        Send only native{" "}
        <span className="font-medium text-ink">{payment.token.symbol}</span> on{" "}
        <span className="font-medium text-ink">{payment.chain.name}</span>, the network you chose.
        The same address on any other network is not this deposit.
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
            {tokenExplorer ? (
              <a
                href={tokenExplorer}
                target="_blank"
                rel="noreferrer noopener"
                className="transition-colors hover:text-ink"
              >
                {truncateAddress(payment.token.address, 10, 8)}
              </a>
            ) : (
              truncateAddress(payment.token.address, 10, 8)
            )}
          </dd>
        </div>
      </dl>
    </div>
  );
}
