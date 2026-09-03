import type { Metadata } from "next";
import { CustomerForm } from "@/components/dashboard/customer-form";

export const metadata: Metadata = { title: "New customer" };

export default function NewCustomerPage() {
  return (
    <>
      <h1 className="text-[22px] font-semibold tracking-tight">New customer</h1>
      <p className="mt-1.5 text-[13px] text-muted">
        A reusable counterparty record. Each invoice keeps its own snapshot, so editing a customer
        later never changes an issued invoice.
      </p>
      <div className="mt-6 max-w-[560px]">
        <CustomerForm />
      </div>
    </>
  );
}
