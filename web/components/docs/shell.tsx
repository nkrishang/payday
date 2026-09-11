"use client";

import { Menu, X } from "lucide-react";
import Image from "next/image";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { type ReactNode, useEffect, useState } from "react";
import { PricingDialog } from "@/components/pricing-dialog";
import { cn } from "@/lib/cn";
import { isEndpointPath } from "./api";
import { MethodBadge } from "./endpoint";
import { type DocsSection, navFor, sectionOf } from "./nav";
import { Toc } from "./toc";

/**
 * The frame around every documentation page: the landing page's header on
 * its 88px rule, a row of section tabs under it (the guides, and the API
 * reference), the section's page list down the left, the page in the
 * middle, and its own headings down the right. `brand-dark` (globals.css)
 * gives it the landing page's palette and type in both colour schemes, the
 * same way the dashboard and the checkout wear it.
 *
 * An endpoint page in the reference carries its samples in its own right
 * column, so it takes the outline's column too. On a phone the page list
 * folds into a drawer under the header.
 */

/** The header's two rows: the 88px rule and the 44px tab row. */
const HEADER_HEIGHT = 132;

const SECTIONS: ReadonlyArray<{ id: DocsSection; title: string; href: string }> = [
  { id: "docs", title: "Documentation", href: "/docs" },
  { id: "api", title: "API Reference", href: "/docs/api" },
];

export function DocsShell({ children }: { children: ReactNode }) {
  const pathname = usePathname();
  const section = sectionOf(pathname);
  const wide = isEndpointPath(pathname);
  const [open, setOpen] = useState(false);

  // Choosing a page closes the drawer (PageList's onNavigate); so does Escape.
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
        <nav className="mx-auto flex h-[88px] max-w-[1400px] items-center justify-between px-5 sm:px-8">
          <div className="flex items-center gap-3">
            <Link href="/" className="rounded-md" aria-label="Payday home">
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
            <span aria-hidden="true" className="hidden h-5 w-px bg-brand-grey/25 sm:block" />
            <Link
              href="/docs"
              className="hidden font-heading text-[15px] font-medium tracking-[-0.02em] text-brand-white sm:block"
            >
              Docs
            </Link>
          </div>

          <div className="hidden items-center gap-6 text-[15px] font-medium text-brand-grey sm:gap-8 md:flex">
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

        <SectionTabs section={section} />

        <div
          id="docs-drawer"
          hidden={!open}
          className="max-h-[calc(100dvh-132px)] overflow-y-auto border-t border-brand-grey/20 bg-canvas px-5 py-5 md:hidden"
        >
          <PageList section={section} pathname={pathname} onNavigate={() => setOpen(false)} />
          <div className="mt-6 flex flex-wrap items-center gap-4 border-t border-brand-grey/20 pt-5 text-[15px] font-medium text-brand-grey">
            <PricingDialog />
            <Link href="/dashboard" className="text-brand-green">
              Dashboard
            </Link>
          </div>
        </div>
      </header>

      <div
        className={cn(
          "mx-auto grid max-w-[1400px] grid-cols-1 gap-x-10 px-5 sm:px-8 md:grid-cols-[240px_minmax(0,1fr)]",
          !wide && "xl:grid-cols-[240px_minmax(0,1fr)_200px]",
        )}
      >
        <aside
          className="sticky hidden overflow-y-auto py-8 pr-2 md:block"
          style={{ top: HEADER_HEIGHT, maxHeight: `calc(100dvh - ${HEADER_HEIGHT}px)` }}
        >
          <PageList section={section} pathname={pathname} />
        </aside>

        <main className="min-w-0 py-10 md:py-12">{children}</main>

        {wide ? null : (
          <aside
            className="sticky hidden overflow-y-auto py-12 xl:block"
            style={{ top: HEADER_HEIGHT, maxHeight: `calc(100dvh - ${HEADER_HEIGHT}px)` }}
          >
            <Toc />
          </aside>
        )}
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

/** The row of sections under the header rule: the guides, and the reference. */
function SectionTabs({ section }: { section: DocsSection }) {
  return (
    <div className="border-t border-brand-grey/20">
      <nav
        aria-label="Sections"
        className="mx-auto flex h-11 max-w-[1400px] items-stretch gap-6 px-5 sm:px-8"
      >
        {SECTIONS.map((entry) => {
          const active = entry.id === section;
          return (
            <Link
              key={entry.id}
              href={entry.href}
              aria-current={active ? "page" : undefined}
              className={cn(
                "-mb-px flex items-center border-b-2 text-[14px] transition-colors",
                active
                  ? "border-brand-green font-medium text-brand-white"
                  : "border-transparent text-brand-grey hover:text-brand-white",
              )}
            >
              {entry.title}
            </Link>
          );
        })}
      </nav>
    </div>
  );
}

function PageList({
  section,
  pathname,
  onNavigate,
}: {
  section: DocsSection;
  pathname: string;
  onNavigate?: () => void;
}) {
  return (
    <nav
      aria-label={section === "api" ? "API reference" : "Documentation"}
      className="flex flex-col gap-7"
    >
      {navFor(section).map((group) => (
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
                      "-ml-3 flex items-center gap-2.5 rounded-[8px] px-3 py-1.5 text-[14px] leading-snug transition-colors",
                      active
                        ? "bg-white/[0.06] font-medium text-brand-white"
                        : "text-[#b0afa9] hover:text-brand-white",
                    )}
                  >
                    {page.method ? <MethodBadge method={page.method} variant="tint" /> : null}
                    <span className="min-w-0">{page.title}</span>
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
