"use client";

import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Problem } from "@/components/ui/field";
import { cn } from "@/lib/cn";
import { formatBaseUnits } from "@/lib/format";
import { CustomerForm } from "./customer-form";
import { InvoiceTable } from "./invoice-table";
import { formatDate } from "./labels";
import { useResource } from "./session";

/** The product's only currency; stats span every invoice, so no one row's own token names it. */
const USDC_DECIMALS = 6;

export function CustomerDetail({ id }: { id: string }) {
  const customer = useResource(id, (client) => client.customers.get(id));
  // Only for the requests table's "Issued by" badge; the form needs none of it.
  const issuers = useResource("issuers", (client) => client.issuers.list({ limit: 50 }));

  const failure = customer.error ?? issuers.error;
  if (failure) {
    return (
      <div className="grid gap-3">
        <Problem>{failure}</Problem>
        <div>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => {
              customer.reload();
              issuers.reload();
            }}
          >
            Try again
          </Button>
        </div>
      </div>
    );
  }
  if (!customer.data || !issuers.data) {
    return (
      <p role="status" className="text-[13px] text-faint">
        Loading…
      </p>
    );
  }

  const record = customer.data;

  return (
    <div className="grid gap-6">
      <div>
        <h1 className="text-[22px] font-semibold tracking-tight">{record.name}</h1>
        <p className="mt-1 text-[13px] text-faint">Created {formatDate(record.created_at)}</p>
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <Stat>
            {record.stats.request_count}{" "}
            {record.stats.request_count === 1 ? "request" : "requests"}
          </Stat>
          <Stat tone="success">
            {formatBaseUnits(record.stats.collected_base_units, USDC_DECIMALS)} USDC collected
          </Stat>
          <Stat tone="warning">
            {formatBaseUnits(record.stats.pending_base_units, USDC_DECIMALS)} USDC pending
          </Stat>
        </div>
      </div>

      <div className="grid gap-8 lg:grid-cols-[560px_minmax(0,1fr)]">
        <div className="max-w-[560px]">
          {/* Remount on each save so the form reflects what the API stored. */}
          <CustomerForm key={record.updated_at} customer={record} onSaved={customer.reload} />
        </div>
        <InvoiceTable
          identities={issuers.data.issuers}
          customers={[]}
          lockedCustomerId={record.id}
          composeHref={`/dashboard?customer=${encodeURIComponent(record.id)}`}
        />
      </div>
    </div>
  );
}

function Stat({
  tone = "neutral",
  children,
}: {
  /** `success` for what's already in, `warning` for what's still owed. */
  tone?: "neutral" | "success" | "warning";
  children: ReactNode;
}) {
  return (
    <span
      className={cn(
        "inline-flex items-center rounded-full border px-2.5 py-1 text-[12px]",
        tone === "success" && "border-success/30 bg-success/10 text-success",
        tone === "warning" && "border-warning/30 bg-warning/10 text-warning",
        tone === "neutral" && "border-line-strong bg-raised text-muted",
      )}
    >
      {children}
    </span>
  );
}
