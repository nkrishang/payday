"use client";

import Link from "next/link";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Problem } from "@/components/ui/field";
import { formatDate } from "./labels";
import { useResource } from "./session";

const PAGE_SIZE = 25;

export function CustomerTable() {
  const [cursors, setCursors] = useState<string[]>([]);
  const after = cursors[cursors.length - 1];
  const page = useResource(after ?? "", (client) =>
    client.customers.list({ limit: PAGE_SIZE, ...(after ? { starting_after: after } : {}) }),
  );
  const customers = page.data?.customers ?? [];

  return (
    <div className="grid gap-4">
      <Problem>{page.error}</Problem>

      <div className="overflow-x-auto rounded-[16px] border border-line bg-surface">
        <table className="w-full text-left text-[13px]">
          <thead className="border-b border-line text-[11px] font-medium tracking-[0.1em] text-faint uppercase">
            <tr>
              <th className="px-4 py-3 font-medium">Name</th>
              <th className="px-4 py-3 font-medium">Email</th>
              <th className="px-4 py-3 font-medium">Created</th>
            </tr>
          </thead>
          <tbody>
            {customers.map((customer) => (
              <tr
                key={customer.id}
                className="border-b border-line last:border-b-0 hover:bg-raised"
              >
                <td className="px-4 py-3">
                  <Link
                    href={`/dashboard/customers/${encodeURIComponent(customer.id)}`}
                    className="font-medium"
                  >
                    {customer.name}
                  </Link>
                </td>
                <td className="px-4 py-3 text-muted">{customer.email ?? "—"}</td>
                <td className="px-4 py-3 whitespace-nowrap text-muted">
                  {formatDate(customer.created_at)}
                </td>
              </tr>
            ))}
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
