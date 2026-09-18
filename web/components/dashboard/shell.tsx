"use client";

import { ArrowLeft } from "lucide-react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import type { ReactNode } from "react";
import { MerchantAuth } from "@/components/merchant-auth";
import { SiteHeader } from "@/components/site-header";
import { DashboardChecking } from "./session-screens";
import { HOME_PATH, MerchantGate, useOptionalMerchant } from "./session";

/**
 * The frame around every dashboard page.
 *
 * `gum` carries the landing page's palette and type (globals.css), and the
 * header is the site's one header (components/site-header.tsx) with "Sign
 * out" appended. There are no section tabs — everything a merchant does day to day is on `/dashboard` itself, and
 * the pages that remain are details of one record, which the back link and the
 * wordmark both return from.
 *
 * The header stands outside the session: it renders while Privy is still
 * restoring the session too, so a reload lands on the skeleton with its
 * chrome already in place rather than an empty page. Signing out replaces
 * everything with its own screen (see the gate).
 */
export function DashboardShell({ children }: { children: ReactNode }) {
  const pathname = usePathname();

  return (
    <div className="gum dash min-h-dvh bg-canvas">
      <MerchantAuth>
        <MerchantGate checking={<Checking pathname={pathname} />}>
          <Header />
          <main className="mx-auto max-w-[1120px] px-5 py-8 sm:px-8">
            {pathname === HOME_PATH ? null : (
              <Link
                href={HOME_PATH}
                className="mb-6 inline-flex items-center gap-1.5 text-[13px] font-medium text-muted transition-colors hover:text-ink"
              >
                <ArrowLeft className="size-3.5" />
                Dashboard
              </Link>
            )}
            {children}
          </main>
        </MerchantGate>
      </MerchantAuth>
    </div>
  );
}

/** The frame without a session yet: the header, over the page's skeleton. */
function Checking({ pathname }: { pathname: string }) {
  return (
    <>
      <Header />
      <DashboardChecking key={pathname} />
    </>
  );
}

function Header() {
  // Null while the session is being restored: the header still stands, but
  // there is nobody to sign out yet.
  const merchant = useOptionalMerchant();
  return <SiteHeader home={HOME_PATH} signOut={{ onClick: merchant?.signOut }} />;
}
