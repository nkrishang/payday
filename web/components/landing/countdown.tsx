"use client";

import { useEffect, useState } from "react";
import { cn } from "@/lib/cn";

/** A deposit request's clock, counting down from just under an hour and going round again. */
const FROM_S = 59 * 60 + 52;

export function Countdown({ className }: { className?: string }) {
  const [seconds, setSeconds] = useState(FROM_S);

  useEffect(() => {
    const timer = setInterval(
      () => setSeconds((current) => (current > 0 ? current - 1 : FROM_S)),
      1_000,
    );
    return () => clearInterval(timer);
  }, []);

  return (
    <span className={cn("tabular", className)}>
      {Math.floor(seconds / 60)}:{String(seconds % 60).padStart(2, "0")}
    </span>
  );
}
