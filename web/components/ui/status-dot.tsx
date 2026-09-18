import type { CheckoutTone } from "@/lib/checkout-state";
import { cn } from "@/lib/cn";

const tones: Record<CheckoutTone, string> = {
  neutral: "bg-faint",
  /* Money on the move is the one thing on a row that is Gum's. */
  progress: "bg-gum-pink",
  success: "bg-success",
  warning: "bg-warning",
};

export function StatusDot({ tone, pulse = false }: { tone: CheckoutTone; pulse?: boolean }) {
  return (
    <span className="relative flex size-2 shrink-0">
      {pulse ? (
        <span
          className={cn("absolute inline-flex size-full animate-ping rounded-full opacity-60", tones[tone])}
        />
      ) : null}
      <span className={cn("relative inline-flex size-2 rounded-full", tones[tone])} />
    </span>
  );
}
