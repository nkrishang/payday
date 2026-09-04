import type { Metadata } from "next";
import { Suspense } from "react";
import { DashboardHome } from "@/components/dashboard/home";

export const metadata: Metadata = { title: "Dashboard" };

export default function DashboardIndex() {
  return (
    // The page reads `?customer=` to open the composer on one; that hook needs
    // a boundary.
    <Suspense>
      <DashboardHome />
    </Suspense>
  );
}
