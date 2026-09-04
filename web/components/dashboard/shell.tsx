"use client";

import { ArrowLeft, LogOut } from "lucide-react";
import Image from "next/image";
import Link from "next/link";
import { usePathname } from "next/navigation";
import type { ReactNode } from "react";
import { HOME_PATH, MerchantGate, useMerchant } from "./session";

/**
 * The frame around every dashboard page.
 *
 * `dash` carries the landing page's palette and type (globals.css), and the
 * header is the landing page's own: the same wordmark at the same size, the
 * same Docs and Pricing links, on the same 76px rule. There are no section
 * tabs — everything a merchant does day to day is on `/dashboard` itself, and
 * the pages that remain are details of one record, which the back link and the
 * wordmark both return from.
 */
export function DashboardShell({ children }: { children: ReactNode }) {
  const pathname = usePathname();

  return (
    <div className="dash min-h-dvh bg-canvas">
      <MerchantGate>
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
    </div>
  );
}

function Header() {
  const { signOut } = useMerchant();

  return (
    <header className="border-b border-brand-grey/20">
      <nav className="mx-auto flex h-[76px] max-w-[1240px] items-center justify-between px-5 sm:px-8">
        {/* Named by the wordmark's own alt text, so it is not a second "Dashboard". */}
        <Link href={HOME_PATH} className="rounded-md">
          <Image
            src="/payday-logo-full.svg"
            width={3600}
            height={1000}
            priority
            alt="Payday"
            className="h-auto w-[116px] sm:w-[140px]"
          />
        </Link>

        <div className="flex items-center text-[15px] text-brand-grey sm:text-[16px]">
          {/* The landing page's two links keep its spacing; signing out is not
              one of them, so a rule and a wider gap set it apart. */}
          <div className="flex items-center gap-6 sm:gap-9">
            <a
              href="https://github.com/nkrishang/payday/tree/main/docs"
              className="transition-colors hover:text-brand-white"
            >
              Docs
            </a>
            <Link href="/#pricing" className="transition-colors hover:text-brand-white">
              Pricing
            </Link>
          </div>

          <span aria-hidden="true" className="mx-5 h-5 w-px bg-brand-grey/25 sm:mx-7" />

          <button
            type="button"
            onClick={signOut}
            className="inline-flex items-center gap-2 transition-colors hover:text-brand-white"
          >
            <LogOut aria-hidden="true" className="size-4" />
            Sign out
          </button>
        </div>
      </nav>
    </header>
  );
}
