"use client";

import { Menu, X } from "lucide-react";
import Image from "next/image";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { type ReactNode, useEffect, useState } from "react";
import { PricingDialog } from "@/components/pricing-dialog";
import { cn } from "@/lib/cn";
import { DOCS_NAV } from "./nav";
import { Toc } from "./toc";

/**
 * The frame around every documentation page: the landing page's header on
 * its 76px rule, a section list down the left, the page in the middle, and
 * its own headings down the right. `brand-dark` (globals.css) gives it the
 * landing page's palette and type in both colour schemes, the same way the
 * dashboard and the checkout wear it.
 *
 * On a phone the section list folds into a drawer under the header, opened
 * from a button that takes the place of the right-hand navigation.
 */
export function DocsShell({ children }: { children: ReactNode }) {
  const pathname = usePathname();
  const [open, setOpen] = useState(false);

  // Choosing a page closes the drawer (SectionList's onNavigate); so does Escape.
  useEffect(() => {
    if (!open) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "Escape") setOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [open]);

  return (
    <div className="brand-dark docs min-h-dvh bg-canvas">
      <header className="sticky top-0 z-40 border-b border-brand-grey/20 bg-canvas/90 backdrop-blur-md">
        <nav className="mx-auto flex h-[76px] max-w-[1400px] items-center justify-between px-5 sm:px-8">
          <div className="flex items-center gap-3">
            <Link href="/" className="rounded-md" aria-label="Payday home">
              <Image
                src="/payday-logo-full.svg"
                width={2929}
                height={1000}
                priority
                alt="Payday"
                className="h-auto w-[116px] sm:w-[140px]"
              />
            </Link>
            <span aria-hidden="true" className="hidden h-5 w-px bg-brand-grey/25 sm:block" />
            <Link
              href="/docs"
              className="hidden font-heading text-[15px] font-medium tracking-[-0.02em] text-brand-white sm:block"
            >
              Docs
            </Link>
          </div>

          <div className="hidden items-center gap-9 text-[16px] text-brand-grey md:flex">
            <PricingDialog />
            <Link
              href="/dashboard"
              className="flex h-10 items-center rounded-[8px] bg-brand-green px-4 text-[15px] font-medium text-brand-black transition-opacity hover:opacity-90"
            >
              Dashboard
            </Link>
          </div>

          <button
            type="button"
            onClick={() => setOpen((value) => !value)}
            aria-expanded={open}
            aria-controls="docs-drawer"
            aria-label={open ? "Close navigation" : "Open navigation"}
            className="flex size-10 items-center justify-center rounded-[8px] border border-brand-grey/25 text-brand-white md:hidden"
          >
            {open ? <X className="size-4" /> : <Menu className="size-4" />}
          </button>
        </nav>

        <div
          id="docs-drawer"
          hidden={!open}
          className="max-h-[calc(100dvh-76px)] overflow-y-auto border-t border-brand-grey/20 bg-canvas px-5 py-5 md:hidden"
        >
          <SectionList pathname={pathname} onNavigate={() => setOpen(false)} />
          <div className="mt-6 flex flex-wrap items-center gap-4 border-t border-brand-grey/20 pt-5 text-[15px] text-brand-grey">
            <PricingDialog />
            <Link href="/dashboard" className="text-brand-green">
              Dashboard
            </Link>
          </div>
        </div>
      </header>

      <div className="mx-auto grid max-w-[1400px] grid-cols-1 gap-x-10 px-5 sm:px-8 md:grid-cols-[220px_minmax(0,1fr)] xl:grid-cols-[220px_minmax(0,1fr)_200px]">
        <aside className="sticky top-[76px] hidden max-h-[calc(100dvh-76px)] overflow-y-auto py-10 pr-2 md:block">
          <SectionList pathname={pathname} />
        </aside>

        <main className="min-w-0 py-10 md:py-12">{children}</main>

        <aside className="sticky top-[76px] hidden max-h-[calc(100dvh-76px)] overflow-y-auto py-12 xl:block">
          <Toc />
        </aside>
      </div>

      <footer className="border-t border-brand-grey/20">
        <div className="mx-auto flex max-w-[1400px] flex-col gap-3 px-5 py-5 text-[12px] text-brand-grey sm:flex-row sm:items-center sm:justify-between sm:px-8">
          <p>
            Make every stablecoin <span className="text-brand-yellow">accountable.</span>
          </p>
          <p>
            Questions:{" "}
            <a href="mailto:support@payday.sh" className="text-brand-green">
              support@payday.sh
            </a>
          </p>
        </div>
      </footer>
    </div>
  );
}

function SectionList({ pathname, onNavigate }: { pathname: string; onNavigate?: () => void }) {
  return (
    <nav aria-label="Documentation" className="flex flex-col gap-7">
      {DOCS_NAV.map((group) => (
        <div key={group.title}>
          <p className="mb-2 font-heading text-[11px] font-medium tracking-[0.08em] text-brand-grey uppercase">
            {group.title}
          </p>
          <ul className="flex flex-col">
            {group.pages.map((page) => {
              const active = pathname === page.href;
              return (
                <li key={page.href}>
                  <Link
                    href={page.href}
                    {...(onNavigate ? { onClick: onNavigate } : {})}
                    aria-current={active ? "page" : undefined}
                    className={cn(
                      "-ml-3 block rounded-[8px] px-3 py-1.5 text-[14px] leading-snug transition-colors",
                      active
                        ? "bg-white/[0.06] font-medium text-brand-white"
                        : "text-[#b0afa9] hover:text-brand-white",
                    )}
                  >
                    {page.title}
                  </Link>
                </li>
              );
            })}
          </ul>
        </div>
      ))}
    </nav>
  );
}
