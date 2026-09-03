import type { PaymentStatus } from "@payday/sdk";
import { StatusDot } from "@/components/ui/status-dot";
import { isTerminalStatus } from "@/lib/checkout-state";
import { statusLabel, statusTone } from "./labels";

/** The payment's lifecycle state, separate from any verification state. */
export function StatusBadge({ status }: { status: PaymentStatus }) {
  return (
    <span className="inline-flex items-center gap-2 text-[13px] font-medium whitespace-nowrap">
      <StatusDot tone={statusTone(status)} pulse={!isTerminalStatus(status)} />
      {statusLabel(status)}
    </span>
  );
}
