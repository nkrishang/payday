"use client";

import type { PaymentStatus } from "@payday/sdk";
import { Paperclip } from "lucide-react";
import Link from "next/link";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Problem, Select } from "@/components/ui/field";
import { formatDisplayAmount } from "@/lib/format";
import { formatDate, modeLabel, STATUSES } from "./labels";
import { useResource } from "./session";
import { StatusBadge } from "./status-badge";
import { VerificationBadge } from "./verification-status";

const PAGE_SIZE = 20;

export function InvoiceTable() {
  const [status, setStatus] = useState<PaymentStatus | "">("");
  // Cursors of the pages visited, so "Previous" is a pop rather than a re-walk.
  const [cursors, setCursors] = useState<string[]>([]);
  const after = cursors[cursors.length - 1];

  const page = useResource(`${status}|${after ?? ""}`, (client) =>
    client.payments.list({
      limit: PAGE_SIZE,
      ...(status ? { status } : {}),
      ...(after ? { starting_after: after } : {}),
    }),
  );

  const invoices = page.data?.payments ?? [];

  return (
    <div className="grid gap-4">
      <div className="flex flex-wrap items-end justify-between gap-3">
        <label className="block">
          <span className="block text-[12px] font-medium text-muted">Status</span>
          <Select
            className="mt-1.5 w-auto min-w-44"
            value={status}
            onChange={(event) => {
              setStatus(event.target.value as PaymentStatus | "");
              setCursors([]);
            }}
          >
            <option value="">All</option>
            {STATUSES.map((entry) => (
              <option key={entry.value} value={entry.value}>
                {entry.label}
              </option>
            ))}
          </Select>
        </label>
        {page.loading ? (
          <span role="status" className="text-[12px] text-faint">
            Loading…
          </span>
        ) : null}
      </div>

      <Problem>{page.error}</Problem>

      <div className="overflow-x-auto rounded-[16px] border border-line bg-surface">
        <table className="w-full text-left text-[13px]">
          <thead className="border-b border-line text-[11px] font-medium tracking-[0.1em] text-faint uppercase">
            <tr>
              <th className="px-4 py-3 font-medium">Invoice</th>
              <th className="px-4 py-3 font-medium">Bill to</th>
              <th className="px-4 py-3 text-right font-medium">Amount</th>
              <th className="px-4 py-3 font-medium">Status</th>
              <th className="px-4 py-3 font-medium">Policy</th>
              <th className="px-4 py-3 font-medium">Verification</th>
              <th className="px-4 py-3 font-medium">Created</th>
            </tr>
          </thead>
          <tbody>
            {invoices.map((invoice) => (
              <tr key={invoice.id} className="border-b border-line last:border-b-0 hover:bg-raised">
                <td className="px-4 py-3">
                  <Link
                    href={`/dashboard/invoices/${encodeURIComponent(invoice.id)}`}
                    className="flex items-center gap-2 font-medium"
                  >
                    <span className="truncate">
                      {invoice.heading ?? invoice.reference ?? "Untitled invoice"}
                    </span>
                    {invoice.has_attachment ? (
                      <Paperclip
                        className="size-3.5 shrink-0 text-faint"
                        aria-label="Has attachment"
                      />
                    ) : null}
                  </Link>
                  {invoice.heading && invoice.reference ? (
                    <span className="block font-mono text-[12px] text-faint">
                      {invoice.reference}
                    </span>
                  ) : null}
                </td>
                <td className="px-4 py-3">{invoice.bill_to_name}</td>
                <td className="tabular px-4 py-3 text-right whitespace-nowrap">
                  {formatDisplayAmount(invoice.amount)} USDC
                </td>
                <td className="px-4 py-3">
                  <StatusBadge status={invoice.status} />
                </td>
                <td className="px-4 py-3 whitespace-nowrap">
                  {modeLabel(invoice.payer_policy_mode)}
                </td>
                <td className="px-4 py-3">
                  <VerificationBadge
                    mode={invoice.payer_policy_mode}
                    completedAt={invoice.verification_completed_at}
                    unsolicitedAt={invoice.likely_unsolicited_at}
                  />
                </td>
                <td className="px-4 py-3 whitespace-nowrap text-muted">
                  {formatDate(invoice.created_at)}
                </td>
              </tr>
            ))}
            {!page.loading && invoices.length === 0 ? (
              <tr>
                <td colSpan={7} className="px-4 py-10 text-center text-muted">
                  No invoices{status ? " with this status" : " yet"}.
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>

      <div className="flex items-center justify-end gap-2">
        <Button
          variant="secondary"
          size="sm"
          disabled={cursors.length === 0 || page.loading}
          onClick={() => setCursors((current) => current.slice(0, -1))}
        >
          Previous
        </Button>
        <Button
          variant="secondary"
          size="sm"
          disabled={!page.data?.next_cursor || page.loading}
          onClick={() => {
            const next = page.data?.next_cursor;
            if (next) setCursors((current) => [...current, next]);
          }}
        >
          Next
        </Button>
      </div>
    </div>
  );
}
