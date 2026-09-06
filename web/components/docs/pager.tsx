"use client";

import { ArrowLeft, ArrowRight } from "lucide-react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import { neighbours } from "./nav";

/** Previous and next pages in reading order, at the foot of every page. */
export function Pager() {
  const pathname = usePathname();
  const { previous, next } = neighbours(pathname);
  if (!previous && !next) return null;

  return (
    <nav
      aria-label="Adjacent pages"
      className="mt-14 grid grid-cols-1 gap-3 border-t border-line pt-6 sm:grid-cols-2"
    >
      {previous ? (
        <Link
          href={previous.href}
          className="group flex flex-col gap-1 rounded-[12px] border border-line bg-surface p-4 transition-colors hover:border-line-strong"
        >
          <span className="flex items-center gap-1.5 text-[12px] text-muted">
            <ArrowLeft className="size-3.5" />
            Previous
          </span>
          <span className="font-heading text-[15px] font-medium text-ink">{previous.title}</span>
        </Link>
      ) : (
        <span />
      )}
      {next ? (
        <Link
          href={next.href}
          className="group flex flex-col items-end gap-1 rounded-[12px] border border-line bg-surface p-4 text-right transition-colors hover:border-line-strong"
        >
          <span className="flex items-center gap-1.5 text-[12px] text-muted">
            Next
            <ArrowRight className="size-3.5" />
          </span>
          <span className="font-heading text-[15px] font-medium text-ink">{next.title}</span>
        </Link>
      ) : null}
    </nav>
  );
}
