"use client";

import type { Customer, DepositRequestStatus } from "@payday/sdk";
import { ChevronRight, Paperclip } from "lucide-react";
import { useEffect, useState } from "react";
import { Amount } from "@/components/ui/amount";
import { Button } from "@/components/ui/button";
import { MenuSelect, type MenuOption } from "@/components/ui/menu-select";
import { StatusDot } from "@/components/ui/status-dot";
import { cn } from "@/lib/cn";
import { DepositRowDetail } from "./deposit-row-detail";
import { formatShortDate, modeLabel, STATUSES } from "./labels";
import { LoadProblem } from "./load-problem";
import { useMerchant, useResource } from "./session";
import { StatusBadge } from "./status-badge";
import { VerificationBadge } from "./verification-status";

/**
 * Every deposit request, view-only: the dashboard issues nothing. The three
 * facts worth filtering on are where the deposit stands, where its
 * verification stands (a separate fact — a gated request can be funded before
 * its payer has verified), and which customer it is addressed to. All three
 * are the API's own parameters, so a filter narrows the query rather than the
 * page.
 *
 * A row opens in place rather than navigating: the list is where a merchant
 * looks, and leaving it to read one request and coming back to re-filter and
 * re-page was the cost of a click. One row is open at a time — the detail is
 * tall enough that two would make the table hard to read — and its request is
 * only fetched once it has been opened.
 */

/** Short enough that the whole table is read at a glance, not scrolled. */
const PAGE_SIZE = 5;
/** Customer options for the filter: fetched once, pages until exhausted. */
const CUSTOMER_PAGE = 100;

type VerificationFilter = "not_required" | "pending" | "verified" | "likely_unsolicited";

/** A column shown from `md` up; see the table's own comment. */
const WIDE_CELL = "hidden md:table-cell";
const WIDE_COL = "hidden md:table-column";

const VERIFICATIONS: ReadonlyArray<{ value: VerificationFilter; label: string }> = [
  { value: "not_required", label: "Not required" },
  { value: "pending", label: "Pending" },
  { value: "verified", label: "Verified" },
  { value: "likely_unsolicited", label: "Likely unsolicited" },
];

export function DepositTable() {
  const [openId, setOpenId] = useState<string | null>(null);
  // Rows that have been opened at least once keep their detail mounted, so
  // coming back to one costs nothing and a closed row costs no request at all.
  const [fetched, setFetched] = useState<ReadonlySet<string>>(() => new Set());
  const [status, setStatus] = useState<DepositRequestStatus | "">("");
  const [verification, setVerification] = useState<VerificationFilter | "">("");
  const [customer, setCustomer] = useState("");
  // Cursors of the pages visited, so "Previous" is a pop rather than a re-walk.
  const [cursors, setCursors] = useState<string[]>([]);
  const after = cursors[cursors.length - 1];

  const page = useResource(
    `deposit-requests:page:${status}|${verification}|${customer}|${after ?? ""}`,
    (client) =>
      client.depositRequests.list({
        limit: PAGE_SIZE,
        ...(status ? { status } : {}),
        ...(verification ? { verification } : {}),
        ...(customer ? { customer_id: customer } : {}),
        ...(after ? { starting_after: after } : {}),
      }),
  );

  const requests = page.data?.deposit_requests ?? [];

  // Paging controls earn their place only when there is another page to reach.
  const paged = cursors.length > 0 || Boolean(page.data?.next_cursor);

  const narrow = (apply: () => void) => {
    apply();
    setCursors([]);
  };

  const toggle = (id: string) => {
    setOpenId((current) => (current === id ? null : id));
    setFetched((current) => (current.has(id) ? current : new Set([...current, id])));
  };

  const filtered = Boolean(status || verification || customer);

  return (
    <section aria-label="Deposits">
      <div>
        <h1 className="font-heading text-[30px] leading-tight font-medium tracking-[-0.045em]">
          Deposits.
        </h1>
        <p className="mt-1.5 text-[13px] text-muted">
          Monitor deposits and filter them by status. New deposit requests are
          created through the API.
        </p>
      </div>

      {/* The control sits with the filters rather than beside the heading: both
          act on the table under them, and both end at its right edge. */}
      <div className="mt-6 flex flex-wrap items-end justify-between gap-3">
        <div className="flex flex-wrap items-end gap-3">
          <MenuSelect
            label="Status"
            className="w-[190px]"
            value={status}
            onChange={(next) => narrow(() => setStatus(next as DepositRequestStatus | ""))}
            options={[
              { value: "", label: "Any status" },
              ...STATUSES.map((entry): MenuOption => ({
                value: entry.value,
                label: entry.label,
                badge: <StatusDot tone={entry.tone} />,
              })),
            ]}
          />
          <MenuSelect
            label="Verification"
            className="w-[190px]"
            value={verification}
            onChange={(next) => narrow(() => setVerification(next as VerificationFilter | ""))}
            options={[
              { value: "", label: "Any verification" },
              ...VERIFICATIONS.map((entry): MenuOption => ({
                value: entry.value,
                label: entry.label,
                badge: (
                  <StatusDot
                    tone={
                      entry.value === "verified"
                        ? "success"
                        : entry.value === "not_required"
                          ? "neutral"
                          : "warning"
                    }
                  />
                ),
              })),
            ]}
          />
          <CustomerFilter value={customer} onChange={(next) => narrow(() => setCustomer(next))} />
        </div>
        {page.loading ? (
          <span role="status" className="text-[12px] text-faint">
            Loading…
          </span>
        ) : null}
      </div>

      {page.error ? (
        <LoadProblem
          compact
          className="mt-4"
          title="Couldn't load deposit requests."
          message={page.error}
          detail={page.detail}
          onRetry={page.reload}
        />
      ) : null}

      <div className="mt-4 overflow-x-auto rounded-[12px] border border-line bg-surface">
        {/* Fixed rather than content-driven, so the headers sit over their own
            column whatever the rows hold — including when there are none.
            Request is the widest because it also carries the disclosure
            control, and anything it clips is a line away in the open row.

            Below `md` the eight columns become two — the request and its
            amount, with the status under the amount — rather than a 960px
            table scrolled sideways: the open row is the reason the table is
            here, and it has to be as wide as the phone, not the table. The
            hidden columns' `<col>`s are hidden too, or a fixed layout would
            keep dealing them their share of the width. */}
        <table className="w-full table-fixed text-[13px] md:min-w-[960px]">
          <colgroup>
            <col className="w-[60%] md:w-[17%]" />
            <col className={cn(WIDE_COL, "md:w-[11%]")} />
            <col className={cn(WIDE_COL, "md:w-[10%]")} />
            <col className="w-[40%] md:w-[10%]" />
            <col className={cn(WIDE_COL, "md:w-[16%]")} />
            <col className={cn(WIDE_COL, "md:w-[12%]")} />
            <col className={cn(WIDE_COL, "md:w-[15%]")} />
            <col className={cn(WIDE_COL, "md:w-[10%]")} />
          </colgroup>
          <thead className="border-b border-line text-[11px] font-medium tracking-[0.1em] text-faint uppercase">
            <tr>
              <th className="px-4 py-3 text-left font-medium whitespace-nowrap">Request</th>
              <th className={cn("px-4 py-3 text-left font-medium whitespace-nowrap", WIDE_CELL)}>
                Payer
              </th>
              <th className={cn("px-4 py-3 text-left font-medium whitespace-nowrap", WIDE_CELL)}>
                Issuer ID
              </th>
              <th className="px-4 py-3 text-right font-medium whitespace-nowrap">Amount</th>
              <th className={cn("px-4 py-3 text-left font-medium whitespace-nowrap", WIDE_CELL)}>
                Status
              </th>
              <th className={cn("px-4 py-3 text-left font-medium whitespace-nowrap", WIDE_CELL)}>
                Policy
              </th>
              <th className={cn("px-4 py-3 text-left font-medium whitespace-nowrap", WIDE_CELL)}>
                Verification
              </th>
              <th className={cn("px-4 py-3 text-right font-medium whitespace-nowrap", WIDE_CELL)}>
                Created
              </th>
            </tr>
          </thead>
          {requests.map((request) => {
            const open = openId === request.id;
            const title = request.heading ?? request.reference ?? "Untitled request";
            return (
              // One body per request, so the summary and its detail are one
              // group and the rule between requests falls in the right place.
              <tbody key={request.id} className="border-b border-line last:border-b-0">
                <tr
                  onClick={() => toggle(request.id)}
                  className={cn(
                    "cursor-pointer align-middle transition-colors hover:bg-raised",
                    open && "bg-raised",
                  )}
                >
                  <td className="px-4 py-3 text-left">
                    <button
                      type="button"
                      aria-expanded={open}
                      // The row carries the click; this keeps it reachable by
                      // keyboard and gives the row an accessible name.
                      onClick={(event) => {
                        event.stopPropagation();
                        toggle(request.id);
                      }}
                      className="flex w-full items-center gap-2 text-left font-medium"
                    >
                      <ChevronRight
                        aria-hidden="true"
                        className={cn(
                          "size-4 shrink-0 text-faint transition-transform duration-200",
                          open && "rotate-90",
                        )}
                      />
                      <span className="truncate">{title}</span>
                      {request.has_attachment ? (
                        <Paperclip
                          className="size-3.5 shrink-0 text-faint"
                          aria-label="Has attachment"
                        />
                      ) : null}
                    </button>
                    {request.heading && request.reference ? (
                      <span className="mt-0.5 block pl-6 font-mono text-[12px] text-faint">
                        {request.reference}
                      </span>
                    ) : null}
                  </td>
                  <td className={cn("truncate px-4 py-3 text-left", WIDE_CELL)}>
                    {request.payer_name}
                  </td>
                  <td className={cn("truncate px-4 py-3 text-left font-mono text-[12px]", WIDE_CELL)}>
                    {request.issuer_id ?? <span className="text-faint">—</span>}
                  </td>
                  <td className="px-4 py-3 text-right">
                    <Amount value={request.amount} currency={request.currency} />
                    <span className="mt-1.5 flex justify-end md:hidden">
                      <StatusBadge status={request.status} />
                    </span>
                  </td>
                  <td className={cn("px-4 py-3 text-left", WIDE_CELL)}>
                    <StatusBadge status={request.status} />
                  </td>
                  <td className={cn("truncate px-4 py-3 text-left", WIDE_CELL)}>
                    {modeLabel(request.payer_policy_mode)}
                  </td>
                  <td className={cn("px-4 py-3 text-left", WIDE_CELL)}>
                    <VerificationBadge
                      mode={request.payer_policy_mode}
                      completedAt={request.verification_completed_at}
                      unsolicitedAt={request.likely_unsolicited_at}
                    />
                  </td>
                  <td className={cn("px-4 py-3 text-right whitespace-nowrap text-muted", WIDE_CELL)}>
                    {formatShortDate(request.created_at)}
                  </td>
                </tr>

                {/* Kept in the tree so its height can animate, and inert while
                    closed so nothing inside is reachable by tab or by a
                    screen reader. */}
                <tr>
                  <td colSpan={8} className="p-0">
                    <div className="dash-expand" data-open={open}>
                      <div inert={!open} className="border-t border-line">
                        {fetched.has(request.id) ? <DepositRowDetail id={request.id} /> : null}
                      </div>
                    </div>
                  </td>
                </tr>
              </tbody>
            );
          })}
          {!page.loading && requests.length === 0 ? (
            <tbody>
              <tr>
                <td colSpan={8} className="px-4 py-10 text-center text-muted">
                  {filtered ? "No deposits match these filters." : "No deposits yet."}
                </td>
              </tr>
            </tbody>
          ) : null}
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

/**
 * The customer filter's options, loaded here rather than owned by the page:
 * the dashboard no longer manages customers, so this is the one place that
 * names them. Pages through the whole list — a filter that silently omitted
 * customers past its first hundred would narrow the wrong thing.
 */
function CustomerFilter({
  value,
  onChange,
}: {
  value: string;
  onChange: (next: string) => void;
}) {
  const { client } = useMerchant();
  const first = useResource("customers:filter", (payday) =>
    payday.customers.list({ limit: CUSTOMER_PAGE }),
  );
  const [rest, setRest] = useState<Customer[]>([]);

  // Continue past the first page in the background; the filter is usable the
  // moment it is, and a list of hundreds still lands in a couple of rounds.
  const cursor = first.data?.next_cursor ?? null;
  useEffect(() => {
    if (!cursor) return;
    let disposed = false;
    (async () => {
      try {
        const page = await client.customers.list({ limit: CUSTOMER_PAGE, starting_after: cursor });
        if (!disposed) setRest((current) => [...current, ...page.customers]);
        // A failed extra page keeps the filter at the customers it has.
      } catch {
        // Leave the rest empty; the chosen value still filters correctly.
      }
    })();
    return () => {
      disposed = true;
    };
  }, [cursor, client]);

  const options = [...(first.data?.customers ?? []), ...rest];
  return (
    <MenuSelect
      label="Customer"
      className="w-[190px]"
      value={value}
      onChange={onChange}
      options={[
        { value: "", label: "Any customer" },
        ...options.map((entry): MenuOption => ({ value: entry.id, label: entry.name })),
      ]}
    />
  );
}
