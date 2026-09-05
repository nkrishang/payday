import type { UnlockedPayerDepositRequest } from "@/lib/checkout-state";
import { formatDisplayAmount, receivedPercent } from "@/lib/format";

/**
 * The amount still owed, as the page's heading. Once anything has been
 * credited the label changes to "Remaining", because the number does too.
 */
export function AmountDue({ payment }: { payment: UnlockedPayerDepositRequest }) {
  const received = BigInt(payment.received_base_units);
  const outstanding = BigInt(payment.remaining_base_units) > 0n;
  const shown = outstanding ? payment.remaining : payment.amount;

  return (
    <div>
      <p className="text-[11px] font-medium tracking-[0.14em] text-faint uppercase">
        {received > 0n ? "Remaining" : "Amount due"}
      </p>
      <h1 className="tabular mt-2 flex items-baseline gap-2 text-[40px] leading-none font-semibold tracking-tight">
        {formatDisplayAmount(shown)}
        <span className="text-base font-medium text-muted">{payment.token.symbol}</span>
      </h1>
    </div>
  );
}

/** Shown only once something has been credited, so the payer sees their progress. */
export function ReceivedProgress({ payment }: { payment: UnlockedPayerDepositRequest }) {
  const received = BigInt(payment.received_base_units);
  if (received === 0n) return null;

  const percent = receivedPercent(payment.received_base_units, payment.amount_base_units);

  return (
    <div className="mt-5">
      <div
        className="h-1 w-full overflow-hidden rounded-full bg-raised"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(percent)}
        aria-label="Amount received"
      >
        <div
          className="h-full rounded-full bg-ink transition-[width] duration-500"
          style={{ width: `${Math.max(percent, 2)}%` }}
        />
      </div>
      <p className="tabular mt-2 text-[13px] text-muted">
        {formatDisplayAmount(payment.received)} of {formatDisplayAmount(payment.amount)}{" "}
        {payment.token.symbol} received
      </p>
    </div>
  );
}
