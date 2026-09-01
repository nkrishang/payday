import type { Metadata } from "next";
import Link from "next/link";
import { CustomerTable } from "@/components/dashboard/customer-table";
import { buttonStyles } from "@/components/ui/button";

export const metadata: Metadata = { title: "Customers" };

export default function CustomersPage() {
  return (
    <>
      <div className="flex items-center justify-between gap-4">
        <h1 className="text-[22px] font-semibold tracking-tight">Customers</h1>
        <Link href="/dashboard/customers/new" className={buttonStyles({ size: "sm" })}>
          New customer
        </Link>
      </div>
      <div className="mt-6">
        <CustomerTable />
      </div>
    </>
  );
}
