import type { Metadata } from "next";
import Link from "next/link";
import { InvoiceTable } from "@/components/dashboard/invoice-table";
import { buttonStyles } from "@/components/ui/button";

export const metadata: Metadata = { title: "Invoices" };

export default function InvoicesPage() {
  return (
    <>
      <div className="flex items-center justify-between gap-4">
        <h1 className="text-[22px] font-semibold tracking-tight">Invoices</h1>
        <Link href="/dashboard/invoices/new" className={buttonStyles({ size: "sm" })}>
          New invoice
        </Link>
      </div>
      <div className="mt-6">
        <InvoiceTable />
      </div>
    </>
  );
}
