"use client";

import { Fragment, type ReactNode } from "react";
import { cn } from "@/lib/cn";
import { formatBaseUnits } from "@/lib/format";
import { CustomerForm } from "./customer-form";
import { DepositTable } from "./deposit-table";
import { formatDate } from "./labels";
import { LoadProblem } from "./load-problem";
import { useResource } from "./session";


export function CustomerDetail({ id }: { id: string }) {
  const customer = useResource(`customer:${id}`, (client) => client.customers.get(id));
  // Only for the requests table's "Issued by" badge; the form needs none of it.
  const issuers = useResource("issuers", (client) => client.issuers.list({ limit: 50 }));

  const failed = customer.error ? customer : issuers.error ? issuers : null;
  if (failed && (!customer.data || !issuers.data)) {
    return (
      <LoadProblem
        title="Couldn't load this customer."
        message={failed.error}
        detail={failed.detail}
        onRetry={() => {
          customer.reload();
          issuers.reload();
        }}
      />
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
          {/* One pair per currency: totals never add across currencies. Every
              supported stablecoin has six decimals. */}
          {record.stats.totals.map((total) => (
            <Fragment key={total.currency}>
              <Stat tone="success">
                {formatBaseUnits(total.collected_base_units, 6)} {total.currency} collected
              </Stat>
              <Stat tone="warning">
                {formatBaseUnits(total.pending_base_units, 6)} {total.currency} pending
              </Stat>
            </Fragment>
          ))}
        </div>
      </div>

      <div className="grid gap-8 lg:grid-cols-[560px_minmax(0,1fr)]">
        <div className="max-w-[560px]">
          {/* Remount on each save so the form reflects what the API stored. */}
          <CustomerForm key={record.updated_at} customer={record} onSaved={customer.reload} />
        </div>
        <DepositTable
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
