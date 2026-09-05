import Image from "next/image";
import { formatDisplayAmount } from "@/lib/format";
import { cn } from "@/lib/cn";

/**
 * An amount with its token's mark rather than its ticker. Every amount on the
 * dashboard is USDC, so spelling it out on every row is six characters of
 * noise; the mark says the same thing and lets the number keep the column.
 */
export function Amount({
  value,
  symbol = "USDC",
  className,
}: {
  value: string;
  symbol?: string;
  className?: string;
}) {
  return (
    <span className={cn("inline-flex items-center gap-1.5 whitespace-nowrap", className)}>
      <Image
        src="/payment-icons/usdc.svg"
        width={64}
        height={64}
        alt={symbol}
        className="size-4 shrink-0 rounded-full"
      />
      <span className="tabular">{formatDisplayAmount(value)}</span>
    </span>
  );
}
