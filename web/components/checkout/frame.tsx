import type { ReactNode } from "react";
import { SiteHeader } from "@/components/site-header";

/**
 * The shell every checkout state renders inside, so nothing jumps between
 * states. Full-page use (`/pay/[id]`, its error and not-found states) opts
 * into `.gum` (see globals.css) so the hosted checkout reads as the same
 * product as the landing page and dashboard — same palette, same type —
 * rather than falling back to a generic light/dark scheme. Embedded use (a
 * preview dropped inside another page) skips it: it already sits inside that
 * page's own `.gum`-themed tree.
 *
 * The page is the landing page's picture of itself (the phone in "Embed in
 * your app"): the site's header, the card, and "Secured by Gum" under it
 * with the brand's one pink dot.
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
        <div className="overflow-hidden rounded-[12px] border border-line bg-surface">
          {children}
        </div>
        <Footnote />
      </div>
    );
  }

  return (
    <div className="gum relative flex min-h-dvh flex-col bg-canvas">
      <SiteHeader trailing={aside} />

      <main className="relative flex flex-1 items-start justify-center px-4 pt-8 pb-16 sm:px-6 sm:pt-12">
        <div className="w-full max-w-[440px]">
          <div className="overflow-hidden rounded-[12px] border border-line bg-surface">
            {children}
          </div>
          <Footnote />
        </div>
      </main>
    </div>
  );
}

/** Who stands behind the address, and what the status on it means. */
function Footnote() {
  return (
    <p className="mt-5 flex flex-wrap items-center justify-center gap-x-2 gap-y-1 text-center text-xs leading-relaxed text-muted">
      <span className="inline-flex items-center gap-1.5 font-medium text-ink">
        <span aria-hidden="true" className="size-1.5 rounded-full bg-gum-pink" />
        Secured by Gum
      </span>
      <span aria-hidden="true" className="text-faint">
        ·
      </span>
      <span>Status reflects finalized on-chain activity only.</span>
    </p>
  );
}
