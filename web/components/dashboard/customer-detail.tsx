"use client";

import Link from "next/link";
import { Button, buttonStyles } from "@/components/ui/button";
import { Problem } from "@/components/ui/field";
import { CustomerForm } from "./customer-form";
import { formatDate } from "./labels";
import { useResource } from "./session";

export function CustomerDetail({ id }: { id: string }) {
  const customer = useResource(id, (client) => client.customers.get(id));

  if (customer.error) {
    return (
      <div className="grid gap-3">
        <Problem>{customer.error}</Problem>
        <div>
          <Button variant="secondary" size="sm" onClick={customer.reload}>
            Try again
          </Button>
        </div>
      </div>
    );
  }
  if (!customer.data) {
    return (
      <p role="status" className="text-[13px] text-faint">
        Loading…
      </p>
    );
  }

  const record = customer.data;

  return (
    <div className="grid gap-6">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <p className="font-mono text-[12px] text-faint">{record.id}</p>
          <h1 className="mt-1 text-[22px] font-semibold tracking-tight">{record.name}</h1>
          <p className="mt-1 text-[13px] text-muted">
            Created {formatDate(record.created_at)} · updated {formatDate(record.updated_at)}
          </p>
        </div>
        <Link
          href={`/dashboard/invoices/new?customer=${encodeURIComponent(record.id)}`}
          className={buttonStyles({ size: "sm" })}
        >
          New invoice for this customer
        </Link>
      </div>

      <div className="max-w-[560px]">
        {/* Remount on each save so the form reflects what the API stored. */}
        <CustomerForm key={record.updated_at} customer={record} onSaved={customer.reload} />
      </div>
    </div>
  );
}
