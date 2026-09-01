import { cn } from "@/lib/cn";
import { formatCountdown } from "@/lib/format";

/** Under ten minutes we stop encouraging a payment that may not settle in time. */
const URGENT_SECONDS = 600;

export function Countdown({ seconds }: { seconds: number }) {
  const urgent = seconds > 0 && seconds <= URGENT_SECONDS;

  return (
    <span
      className={cn("tabular text-[13px] font-medium", urgent ? "text-warning" : "text-muted")}
      // Announce the deadline, but not on every tick.
      aria-label={seconds > 0 ? `Expires in ${formatCountdown(seconds)}` : "Deadline reached"}
    >
      {seconds > 0 ? `Expires in ${formatCountdown(seconds)}` : "Deadline reached"}
    </span>
  );
}
