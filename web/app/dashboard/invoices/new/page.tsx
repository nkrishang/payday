import type { Metadata } from "next";
import { Suspense } from "react";
import { InvoiceForm } from "@/components/dashboard/invoice-form";

export const metadata: Metadata = { title: "New invoice" };

export default function NewInvoicePage() {
  return (
    <>
      <h1 className="text-[22px] font-semibold tracking-tight">New invoice</h1>
      <p className="mt-1.5 text-[13px] text-muted">
        Issued invoices are immutable. To change the amount, parties, policy, or attachment, cancel
        and issue a new one.
      </p>
      <div className="mt-6">
        {/* The form reads `?customer=` to pre-fill; that hook needs a boundary. */}
        <Suspense>
          <InvoiceForm />
        </Suspense>
      </div>
    </>
  );
}
