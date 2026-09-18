"use client";

import { Menu, X } from "lucide-react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { type ReactNode, useEffect, useState } from "react";
import { RESOURCES } from "@/components/landing/links";
import { PricingDialog } from "@/components/pricing-dialog";
import { SiteFooter } from "@/components/site-footer";
import { SiteHeader } from "@/components/site-header";
import { cn } from "@/lib/cn";
import { isEndpointPath } from "./api";
import { MethodBadge } from "./endpoint";
import { type DocsSection, navFor, sectionOf } from "./nav";
import { Toc } from "./toc";

/**
 * The frame around every documentation page: the site's header, a row of
 * section tabs under it (the guides, and the API reference), the section's
 * page list down the left, the page in the middle, and its own headings
 * down the right. `brand-dark` (globals.css) is Gum's palette on a black
 * ground: the docs are the one part of the site that reads dark.
 *
 * An endpoint page in the reference carries its samples in its own right
 * column, so it takes the outline's column too. On a phone the page list
 * folds into a drawer under the header.
 */

/** The header's two rows: the site header's rule and the 44px tab row. */
const HEADER_HEIGHT = 76 + 44;

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
      <div className="sticky top-0 z-40 bg-canvas/90 backdrop-blur-md">
        <SiteHeader
          tone="dark"
          collapse
          trailing={
            <>
              <Link
                href="/dashboard"
                className="hidden h-9 items-center rounded-[6px] bg-gum-white px-3.5 text-[14px] font-medium text-gum-black transition-colors hover:bg-gum-white/90 md:flex"
              >
                Dashboard
              </Link>
              <button
                type="button"
                onClick={() => setOpen((value) => !value)}
                aria-expanded={open}
                aria-controls="docs-drawer"
                aria-label={open ? "Close navigation" : "Open navigation"}
                className="flex size-9 items-center justify-center rounded-[6px] border border-gum-white/15 text-gum-white md:hidden"
              >
                {open ? <X className="size-4" /> : <Menu className="size-4" />}
              </button>
            </>
          }
        />

        <SectionTabs section={section} />

        <div
          id="docs-drawer"
          hidden={!open}
          className="max-h-[calc(100dvh-104px)] overflow-y-auto border-t border-line bg-canvas px-4 py-5 md:hidden"
        >
          <PageList section={section} pathname={pathname} onNavigate={() => setOpen(false)} />
          <div className="mt-6 flex flex-wrap items-center gap-5 border-t border-line pt-5 text-[15px] text-muted">
            <PricingDialog appearance="dark" />
            {RESOURCES.map((entry) => (
              <a
                key={entry.label}
                href={entry.href}
                target={entry.href.startsWith("mailto:") ? undefined : "_blank"}
                rel={entry.href.startsWith("mailto:") ? undefined : "noopener noreferrer"}
                className="transition-colors hover:text-ink"
              >
                {entry.label}
              </a>
            ))}
            <Link
              href="/dashboard"
              className="inline-flex h-9 items-center rounded-[6px] bg-gum-white px-3.5 text-[14px] font-medium text-gum-black"
            >
              Dashboard
            </Link>
          </div>
        </div>
      </div>

      <div
        className={cn(
          "mx-auto grid max-w-[1360px] grid-cols-1 gap-x-10 px-4 sm:px-10 md:grid-cols-[240px_minmax(0,1fr)]",
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

      <SiteFooter />
    </div>
  );
}

/** The row of sections under the header rule: the guides, and the reference. */
function SectionTabs({ section }: { section: DocsSection }) {
  return (
    <div className="border-b border-line">
      <nav
        aria-label="Sections"
        className="mx-auto flex h-11 max-w-[1360px] items-stretch gap-6 px-4 sm:px-10"
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
                  ? "border-gum-pink font-medium text-ink"
                  : "border-transparent text-muted hover:text-ink",
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
          <p className="mb-2 font-heading text-[11px] font-medium tracking-[0.08em] text-faint uppercase">
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
                      active ? "bg-ink/[0.06] font-medium text-ink" : "text-muted hover:text-ink",
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
