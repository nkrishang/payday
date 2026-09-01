import Link from "next/link";
import type { ReactNode } from "react";
import { Logo } from "@/components/ui/logo";

/** The shell every checkout state renders inside, so nothing jumps between states. */
export function CheckoutFrame({
  children,
  aside,
}: {
  children: ReactNode;
  aside?: ReactNode;
}) {
  return (
    <div className="flex min-h-dvh flex-col bg-canvas">
      <header className="flex items-center justify-between px-5 py-5 sm:px-8">
        <Link href="/" className="rounded-md">
          <Logo className="h-[18px]" />
        </Link>
        {aside}
      </header>

      <main className="flex flex-1 items-start justify-center px-4 pb-16 sm:px-6">
        <div className="w-full max-w-[440px]">
          <div className="overflow-hidden rounded-[16px] border border-line bg-surface">
            {children}
          </div>
          <p className="mt-5 text-center text-xs leading-relaxed text-faint">
            Status reflects finalized on-chain activity only.
          </p>
        </div>
      </main>
    </div>
  );
}
