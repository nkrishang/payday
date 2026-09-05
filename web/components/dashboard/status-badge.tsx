import type { DepositRequestStatus } from "@payday/sdk";
import { StatusDot } from "@/components/ui/status-dot";
import { isTerminalStatus } from "@/lib/checkout-state";
import { statusLabel, statusTone } from "./labels";

/** The deposit request's lifecycle state, separate from any verification state. */
export function StatusBadge({ status }: { status: DepositRequestStatus }) {
  return (
    // Wraps rather than overflows, the way the verification badge does: in a
    // column too narrow for "Awaiting deposit" the label belongs under the
    // dot, not past the cell and over the next one.
    <span className="inline-flex flex-wrap items-center gap-x-2 gap-y-1 text-[13px] font-medium">
      <StatusDot tone={statusTone(status)} pulse={!isTerminalStatus(status)} />
      {statusLabel(status)}
    </span>
  );
}
