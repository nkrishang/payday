"use client";

import Link from "next/link";
import { useRouter } from "next/navigation";
import { useState } from "react";
import { AddButton } from "@/components/ui/add-button";
import { Button } from "@/components/ui/button";
import { Problem } from "@/components/ui/field";
import { formatShortDate } from "./labels";
import { useResource } from "./session";

/**
 * The customers section: its own heading, its own control, and its own table —
 * the same shape the identities section has, so the two read as siblings
 * rather than as a section and a stray toolbar.
 */

/** The same page size the requests table uses, for the same reason. */
const PAGE_SIZE = 5;

export function CustomerTable({ onAdd }: { onAdd: () => void }) {
  const router = useRouter();
  const [cursors, setCursors] = useState<string[]>([]);
  const after = cursors[cursors.length - 1];
  const page = useResource(after ?? "", (client) =>
    client.customers.list({ limit: PAGE_SIZE, ...(after ? { starting_after: after } : {}) }),
  );
  const customers = page.data?.customers ?? [];
  const paged = cursors.length > 0 || Boolean(page.data?.next_cursor);

  return (
    <section aria-label="Customers">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h2 className="font-heading text-[24px] leading-tight font-medium tracking-[-0.04em]">
            Customers<span className="text-brand-yellow">.</span>
          </h2>
          <p className="mt-1.5 text-[13px] text-muted">
            Saved counterparties. Editing one never changes a request already issued.
          </p>
        </div>
        <div className="flex items-center gap-3">
          {page.loading ? (
            <span role="status" className="text-[12px] text-faint">
              Loading…
            </span>
          ) : null}
          <AddButton label="New customer" onClick={onAdd} />
        </div>
      </div>

      <div className="mt-6">
        <Problem>{page.error}</Problem>
      </div>

      <div className="mt-6 overflow-x-auto rounded-[16px] border border-line bg-surface">
        <table className="w-full table-fixed text-[13px]">
          <colgroup>
            <col className="w-[40%]" />
            <col className="w-[40%]" />
            <col className="w-[20%]" />
          </colgroup>
          <thead className="border-b border-line text-[11px] font-medium tracking-[0.1em] text-faint uppercase">
            <tr>
              <th className="px-4 py-3 text-left font-medium whitespace-nowrap">Name</th>
              <th className="px-4 py-3 text-left font-medium whitespace-nowrap">Email</th>
              <th className="px-4 py-3 text-right font-medium whitespace-nowrap">Created</th>
            </tr>
          </thead>
          <tbody>
            {customers.map((customer) => {
              const href = `/dashboard/customers/${encodeURIComponent(customer.id)}`;
              return (
                <tr
                  key={customer.id}
                  onClick={() => router.push(href)}
                  className="cursor-pointer border-b border-line align-middle last:border-b-0 hover:bg-raised"
                >
                  <td className="truncate px-4 py-3 text-left">
                    <Link
                      href={href}
                      onClick={(event) => event.stopPropagation()}
                      className="font-medium"
                    >
                      {customer.name}
                    </Link>
                  </td>
                  <td className="truncate px-4 py-3 text-left text-muted">
                    {customer.email ?? "—"}
                  </td>
                  <td className="px-4 py-3 text-right whitespace-nowrap text-muted">
                    {formatShortDate(customer.created_at)}
                  </td>
                </tr>
              );
            })}
            {!page.loading && customers.length === 0 ? (
              <tr>
                <td colSpan={3} className="px-4 py-10 text-center text-muted">
                  No customers yet.
                </td>
              </tr>
            ) : null}
          </tbody>
        </table>
      </div>

      {paged ? (
        <div className="mt-4 flex items-center justify-end gap-2">
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
      ) : null}
    </section>
  );
}
