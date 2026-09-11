import Image from "next/image";
import Link from "next/link";
import type { ReactNode } from "react";

/**
 * The shell every checkout state renders inside, so nothing jumps between
 * states. Full-page use (`/pay/[id]`, its error and not-found states) opts
 * into `.brand-dark` (see globals.css) so the hosted checkout reads as the
 * same product as the landing page and dashboard — same palette, same type,
 * same paper-grain texture — rather than falling back to a generic
 * light/dark scheme. Embedded use (the dashboard onboarding walkthrough's
 * live payer preview) skips it: it already sits inside the dashboard's own
 * `.dash`-themed tree, so re-applying it would just double the grain overlay.
 */
export function CheckoutFrame({
  children,
  aside,
  embedded,
}: {
  children: ReactNode;
  aside?: ReactNode;
  /** Dropped inside another page rather than owning the viewport: no page
   * header, no forced full-height main, just the card and its footnote. */
  embedded?: boolean | undefined;
}) {
  if (embedded) {
    return (
      <div className="w-full max-w-[440px]">
        <div className="overflow-hidden rounded-[16px] border border-line bg-surface">
          {children}
        </div>
        <p className="mt-5 text-center text-xs leading-relaxed text-faint">
          Status reflects finalized on-chain activity only.
        </p>
      </div>
    );
  }

  return (
    <div className="brand-dark relative flex min-h-dvh flex-col bg-canvas">
      {/* A quiet glow behind the card, not a spectacle: the same green the
          rest of the product treats as "go", just faded almost to nothing. */}
      <div
        aria-hidden="true"
        className="pointer-events-none absolute inset-0 flex justify-center overflow-hidden"
      >
        <div className="mt-[-12%] h-[560px] w-[820px] shrink-0 rounded-full bg-brand-green/[0.07] blur-[120px]" />
      </div>

      <header className="relative flex h-[88px] items-center justify-between px-5 sm:px-8">
        <Link href="/" className="rounded-md" aria-label="Payday">
          <Image
            src="/payday-logo-full.svg"
            width={2929}
            height={1000}
            priority
            sizes="100px"
            alt="Payday"
            className="h-auto w-[88px] sm:w-[100px]"
          />
        </Link>
        {aside}
      </header>

      <main className="relative flex flex-1 items-start justify-center px-4 pb-16 sm:px-6">
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
