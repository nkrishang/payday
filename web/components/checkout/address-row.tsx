import type { PayerPayment } from "@payday/sdk";
import { ArrowUpRight } from "lucide-react";
import { CopyButton } from "@/components/ui/copy-button";

export function AddressRow({ payment }: { payment: PayerPayment }) {
  return (
    <div>
      <div className="flex items-center justify-between gap-2">
        <p className="text-[11px] font-medium uppercase tracking-[0.14em] text-faint">
          One-time address
        </p>
        {payment.address_explorer_url ? (
          <a
            href={payment.address_explorer_url}
            target="_blank"
            rel="noreferrer noopener"
            className="inline-flex items-center gap-1 rounded-md text-[11px] font-medium text-muted transition-colors hover:text-ink"
          >
            Explorer
            <ArrowUpRight className="size-3" />
          </a>
        ) : null}
      </div>
      <div className="mt-2 flex items-center gap-2 rounded-[10px] border border-line bg-raised py-1.5 pr-1.5 pl-3">
        <span className="min-w-0 flex-1 truncate font-mono text-[13px] text-ink">
          {payment.address}
        </span>
        <CopyButton value={payment.address} label="payment address" className="bg-surface" />
      </div>
    </div>
  );
}
