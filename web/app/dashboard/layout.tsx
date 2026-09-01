import type { Metadata } from "next";
import { DashboardShell } from "@/components/dashboard/shell";

/**
 * The dashboard is rendered per request and never prerendered: the pages are
 * empty shells that fetch merchant data in the browser with the session token,
 * so no invoice, customer, or token is ever in server-rendered HTML or a build
 * artifact. Per-request rendering also earns the strict, nonced CSP.
 */
export const dynamic = "force-dynamic";

export const metadata: Metadata = {
  title: "Dashboard",
  // Merchant pages are private; nothing here should be indexed.
  robots: { index: false, follow: false },
};

export default function DashboardLayout({ children }: { children: React.ReactNode }) {
  return <DashboardShell>{children}</DashboardShell>;
}
