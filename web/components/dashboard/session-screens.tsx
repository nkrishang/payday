"use client";

import { Loader2 } from "lucide-react";
import Image from "next/image";
import { cn } from "@/lib/cn";

/**
 * The two full-page states the merchant session can hold on its own: signing
 * out, and waiting for the session to be restored. Both are what the merchant
 * sees instead of the dashboard, so both carry the page's palette and type
 * rather than a bare line of text.
 */

/**
 * The whole page, from the moment sign-out is asked for until the landing
 * page replaces it. Replacing everything is the point: a sign-out must never
 * show the dashboard still standing behind it, and the screen holds through
 * the navigation away, so there is no blank frame on the way out.
 */
export function SigningOutScreen() {
  return (
    <div role="status" aria-label="Signing out" className="dash-enter grid min-h-dvh place-items-center">
      <div className="flex flex-col items-center gap-6">
        <Image
          src="/payday-logo-full.svg"
          width={2929}
          height={1000}
          priority
          alt="Payday"
          className="h-auto w-[116px] opacity-90 sm:w-[140px]"
        />
        <div className="flex flex-col items-center gap-2.5">
          <Loader2 className="size-5 animate-spin text-muted" aria-hidden="true" />
          <p className="text-[13px] text-muted">Signing you out…</p>
        </div>
      </div>
    </div>
  );
}

/** One quiet bar of the skeleton. */
function Bar({ className }: { className?: string }) {
  return <span className={cn("block rounded-full bg-raised motion-safe:animate-pulse", className)} />;
}

/** One section of the page, as it will stand: a heading, a line, and its card. */
function SectionSkeleton({ heading, card }: { heading: string; card: string }) {
  return (
    <section>
      <Bar className={cn("h-6", heading)} />
      <Bar className="mt-2.5 h-3.5 w-[min(440px,90%)]" />
      <div className={cn("mt-5 rounded-[16px] border border-line bg-surface", card)}>
        <div className="grid gap-3 px-4 py-4">
          <Bar className="h-4 w-[min(320px,80%)]" />
          <Bar className="h-4 w-[min(260px,65%)]" />
          <Bar className="h-4 w-[min(400px,90%)]" />
        </div>
      </div>
    </section>
  );
}

/**
 * The dashboard, as it will stand, while the session is being restored. The
 * page is deterministic — the same sections in the same order every time —
 * so the skeleton holds their shape and the settled page lands in place of
 * it rather than jumping.
 */
export function DashboardChecking() {
  return (
    <div role="status" aria-label="Loading your dashboard" className="dash-enter">
      <main className="mx-auto max-w-[1120px] px-5 py-8 sm:px-8">
        <div className="pt-2">
          <Bar className="h-8 w-[172px]" />
          <Bar className="mt-3 h-3.5 w-[min(460px,90%)]" />
        </div>
        <div className="mt-12 grid gap-14">
          <SectionSkeleton heading="w-[124px]" card="h-48" />
          <SectionSkeleton heading="w-[196px]" card="h-36" />
          <SectionSkeleton heading="w-[136px]" card="h-36" />
          <SectionSkeleton heading="w-[110px]" card="h-40" />
          <SectionSkeleton heading="w-[104px]" card="h-40" />
        </div>
      </main>
    </div>
  );
}
