import Image from "next/image";
import { formatDisplayAmount } from "@/lib/format";
import { cn } from "@/lib/cn";

/**
 * An amount with its currency's mark rather than its ticker: the mark says
 * which stablecoin it is and lets the number keep the column. The mark is
 * the currency's (USDC, USDT), never a chain's own symbol for it.
 */
export function Amount({
  value,
  currency = "USDC",
  className,
}: {
  value: string;
  /** `USDC` or `USDT`. */
  currency?: string;
  className?: string;
}) {
  return (
    <span className={cn("inline-flex items-center gap-1.5 whitespace-nowrap", className)}>
      <CurrencyMark currency={currency} className="size-4" />
      <span className="tabular">{formatDisplayAmount(value)}</span>
    </span>
  );
}

/** The currency's mark, from `/public/payment-icons`. */
export function CurrencyMark({ currency, className }: { currency: string; className?: string }) {
  return (
    <Image
      src={currencyIcon(currency)}
      width={64}
      height={64}
      alt={currency}
      className={cn("shrink-0 rounded-full", className)}
    />
  );
}

export function currencyIcon(currency: string): string {
  return `/payment-icons/${currency.toLowerCase()}.svg`;
}
