import Link from "next/link";
import { CheckoutFrame } from "@/components/checkout/frame";

export default function DepositNotFound() {
  return (
    <CheckoutFrame>
      <div className="px-6 py-14 text-center sm:px-8">
        <h1 className="text-xl font-semibold tracking-tight">This deposit link is not valid</h1>
        <p className="mx-auto mt-3 max-w-[36ch] text-[15px] leading-relaxed text-muted">
          It may have been mistyped, or the deposit request may never have existed. Ask the merchant for a
          new link — deposit addresses are single-use and must never be reused.
        </p>
        <Link
          href="/"
          className="mt-8 inline-flex h-10 items-center rounded-[10px] border border-line-strong px-4 text-sm font-medium transition-colors hover:bg-raised"
        >
          What is Payday?
        </Link>
      </div>
    </CheckoutFrame>
  );
}
