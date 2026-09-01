"use client";

import { RotateCw } from "lucide-react";
import { useEffect } from "react";
import { CheckoutFrame } from "@/components/checkout/frame";
import { Button } from "@/components/ui/button";

export default function PaymentError({
  error,
  reset,
}: {
  error: Error & { digest?: string };
  reset: () => void;
}) {
  useEffect(() => {
    console.error(error);
  }, [error]);

  return (
    <CheckoutFrame>
      <div className="px-6 py-14 text-center sm:px-8">
        <h1 className="text-xl font-semibold tracking-tight">This payment could not be loaded</h1>
        <p className="mx-auto mt-3 max-w-[38ch] text-[15px] leading-relaxed text-muted">
          The payment itself is unaffected — this page could not reach Payday. Try again in a
          moment.
        </p>
        <Button onClick={reset} className="mt-8" variant="secondary">
          <RotateCw className="size-4" />
          Try again
        </Button>
        {error.digest ? (
          <p className="mt-6 font-mono text-xs text-faint">Reference {error.digest}</p>
        ) : null}
      </div>
    </CheckoutFrame>
  );
}
