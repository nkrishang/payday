"use client";

import { LogOut } from "lucide-react";
import Link from "next/link";
import { usePathname } from "next/navigation";
import type { ReactNode } from "react";
import { Logo } from "@/components/ui/logo";
import { cn } from "@/lib/cn";
import { LOGIN_PATH, MerchantGate, useMerchant } from "./session";

const NAV = [
  { href: "/dashboard/invoices", label: "Invoices" },
  { href: "/dashboard/customers", label: "Customers" },
] as const;

/**
 * The frame around every dashboard page. The login page renders inside it
 * without the session gate; everything else waits for a merchant.
 */
export function DashboardShell({ children }: { children: ReactNode }) {
  const pathname = usePathname();

  if (pathname === LOGIN_PATH) {
    return (
      <div className="flex min-h-dvh flex-col bg-canvas">
        <header className="px-5 py-5 sm:px-8">
          <Link href="/" className="inline-block rounded-md">
            <Logo className="h-[18px]" />
          </Link>
        </header>
        <main className="flex flex-1 items-start justify-center px-4 pb-16 sm:px-6">
          {children}
        </main>
      </div>
    );
  }

  return (
    <div className="min-h-dvh bg-canvas">
      <MerchantGate>
        <Header pathname={pathname} />
        <main className="mx-auto max-w-5xl px-5 py-8 sm:px-8">{children}</main>
      </MerchantGate>
    </div>
  );
}

function Header({ pathname }: { pathname: string }) {
  const { signOut } = useMerchant();

  return (
    <header className="border-b border-line bg-surface">
      <nav className="mx-auto flex h-14 max-w-5xl items-center gap-6 px-5 sm:px-8">
        <Link href="/dashboard/invoices" className="rounded-md">
          <Logo className="h-[18px]" />
        </Link>
        <ul className="flex flex-1 items-center gap-1">
          {NAV.map((item) => {
            const active = pathname.startsWith(item.href);
            return (
              <li key={item.href}>
                <Link
                  href={item.href}
                  aria-current={active ? "page" : undefined}
                  className={cn(
                    "rounded-md px-2.5 py-1.5 text-[13px] font-medium transition-colors",
                    active ? "bg-raised text-ink" : "text-muted hover:text-ink",
                  )}
                >
                  {item.label}
                </Link>
              </li>
            );
          })}
        </ul>
        <button
          type="button"
          onClick={signOut}
          className="inline-flex items-center gap-1.5 rounded-md px-2 py-1.5 text-[13px] font-medium text-muted transition-colors hover:bg-raised hover:text-ink"
        >
          <LogOut className="size-3.5" />
          Sign out
        </button>
      </nav>
    </header>
  );
}
