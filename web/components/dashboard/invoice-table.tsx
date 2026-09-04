"use client";

import type { Customer, Issuer, PaymentStatus } from "@payday/sdk";
import { ChevronRight, Paperclip, Plus } from "lucide-react";
import Link from "next/link";
import { useState } from "react";
import { AddButton } from "@/components/ui/add-button";
import { Amount } from "@/components/ui/amount";
import { Button, buttonStyles } from "@/components/ui/button";
import { Problem } from "@/components/ui/field";
import { MenuSelect, type MenuOption } from "@/components/ui/menu-select";
import { StatusDot } from "@/components/ui/status-dot";
import { cn } from "@/lib/cn";
import { InvoiceRowDetail } from "./invoice-row-detail";
import { formatShortDate, modeLabel, STATUSES } from "./labels";
import { useResource } from "./session";
import { StatusBadge } from "./status-badge";
import { VerificationBadge } from "./verification-status";

/**
 * Every deposit request, with the three facts worth filtering on: where the
 * payment stands, where its verification stands (a separate fact — a gated
 * request can be funded before its payer has verified), and who it is billed
 * to. All three are the API's own parameters, so a filter narrows the query
 * rather than the page.
 *
 * A row opens in place rather than navigating: the list is where a merchant
 * works, and leaving it to read one request and coming back to re-filter and
 * re-page was the cost of a click. One row is open at a time — the detail is
 * tall enough that two would make the table hard to read — and its request is
 * only fetched once it has been opened.
 */

/** Short enough that the whole table is read at a glance, not scrolled. */
const PAGE_SIZE = 5;

type VerificationFilter = "not_required" | "pending" | "verified" | "likely_unsolicited";

const VERIFICATIONS: ReadonlyArray<{ value: VerificationFilter; label: string }> = [
  { value: "not_required", label: "Not required" },
  { value: "pending", label: "Pending" },
  { value: "verified", label: "Verified" },
  { value: "likely_unsolicited", label: "Likely unsolicited" },
];

export function InvoiceTable({
  identities,
  customers,
  initialOpen,
  onCompose,
  composeHref,
  lockedCustomerId,
}: {
  /** Resolves the issuer badge on each row; the request stores only the id. */
  identities: Issuer[];
  customers: Customer[];
  /** A request to open on arrival — what "Track this request" hands over. */
  initialOpen?: string | undefined;
  /** Opens the composer in place, on the dashboard's own table. */
  onCompose?: (() => void) | undefined;
  /** Issues pre-billed to this customer instead, on a customer's own page. */
  composeHref?: string | undefined;
  /**
   * Scopes the table to one customer's own requests, the way a customer's page
   * embeds this table. The Bill-to filter would only ever narrow to the one
   * customer it is already narrowed to, so it is dropped rather than shown
   * disabled.
   */
  lockedCustomerId?: string | undefined;
}) {
  const [openId, setOpenId] = useState<string | null>(initialOpen ?? null);
  // Rows that have been opened at least once keep their detail mounted, so
  // coming back to one costs nothing and a closed row costs no request at all.
  const [fetched, setFetched] = useState<ReadonlySet<string>>(
    () => new Set(initialOpen ? [initialOpen] : []),
  );
  const [status, setStatus] = useState<PaymentStatus | "">("");
  const [verification, setVerification] = useState<VerificationFilter | "">("");
  const [customer, setCustomer] = useState(lockedCustomerId ?? "");
  // Cursors of the pages visited, so "Previous" is a pop rather than a re-walk.
  const [cursors, setCursors] = useState<string[]>([]);
  const after = cursors[cursors.length - 1];

  const page = useResource(`${status}|${verification}|${customer}|${after ?? ""}`, (client) =>
    client.payments.list({
      limit: PAGE_SIZE,
      ...(status ? { status } : {}),
      ...(verification ? { verification } : {}),
      ...(customer ? { customer_id: customer } : {}),
      ...(after ? { starting_after: after } : {}),
    }),
  );

  const invoices = page.data?.payments ?? [];
  const issuerName = (id: string | null) =>
    identities.find((identity) => identity.id === id)?.name ?? null;

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

  // The locked customer is not a filter the page applied; it is scope the
  // caller already narrowed to, so it should not read "no matches" for a
  // customer with no requests at all.
  const filtered = Boolean(status || verification || (!lockedCustomerId && customer));

  return (
    <section aria-label="Deposit requests">
      {lockedCustomerId ? null : (
        <div>
          <h1 className="font-heading text-[30px] leading-tight font-medium tracking-[-0.045em]">
            Deposit requests<span className="text-brand-yellow">.</span>
          </h1>
          <p className="mt-1.5 text-[13px] text-muted">
            Every request issued, where its payment stands, and where its verification stands.
          </p>
        </div>
      )}

      {/* The control sits with the filters rather than beside the heading: both
          act on the table under them, and both end at its right edge. */}
      <div
        className={cn(
          "flex flex-wrap items-end justify-between gap-3",
          lockedCustomerId ? "" : "mt-6",
        )}
      >
        <div className="flex flex-wrap items-end gap-3">
          <MenuSelect
            label="Status"
            className="w-[190px]"
            value={status}
            onChange={(next) => narrow(() => setStatus(next as PaymentStatus | ""))}
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
          {lockedCustomerId ? null : (
            <MenuSelect
              label="Bill to"
              className="w-[190px]"
              value={customer}
              onChange={(next) => narrow(() => setCustomer(next))}
              options={[
                { value: "", label: "Any customer" },
                ...customers.map((entry): MenuOption => ({ value: entry.id, label: entry.name })),
              ]}
            />
          )}
        </div>
        <div className="flex items-center gap-3">
          {page.loading ? (
            <span role="status" className="text-[12px] text-faint">
              Loading…
            </span>
          ) : null}
          {onCompose ? (
            <AddButton label="New deposit request" onClick={onCompose} />
          ) : composeHref ? (
            <Link
              href={composeHref}
              aria-label="New deposit request for this customer"
              className={cn(buttonStyles({ size: "sm" }), "w-9 px-0")}
            >
              <Plus className="size-4" />
            </Link>
          ) : null}
        </div>
      </div>

      <div className="mt-4">
        <Problem>{page.error}</Problem>
      </div>

      <div className="mt-4 overflow-x-auto rounded-[16px] border border-line bg-surface">
        {/* Fixed rather than content-driven, so the headers sit over their own
            column whatever the rows hold — including when there are none.
            Request is the widest because it also carries the disclosure
            control, and anything it clips is a line away in the open row.
            On a customer's own page, status, policy, verification, and when
            it was created all read faster as a picture than as more columns,
            and the open row already draws that picture — so that table stays
            to what is scanned across many requests at once: what, who, and
            how much. */}
        <table className={cn("w-full table-fixed text-[13px]", !lockedCustomerId && "min-w-[960px]")}>
          <colgroup>
            {lockedCustomerId ? (
              <>
                <col className="w-[54%]" />
                <col className="w-[24%]" />
                <col className="w-[22%]" />
              </>
            ) : (
              <>
                <col className="w-[17%]" />
                <col className="w-[9%]" />
                <col className="w-[11%]" />
                <col className="w-[10%]" />
                <col className="w-[16%]" />
                <col className="w-[12%]" />
                <col className="w-[15%]" />
                <col className="w-[10%]" />
              </>
            )}
          </colgroup>
          <thead className="border-b border-line text-[11px] font-medium tracking-[0.1em] text-faint uppercase">
            <tr>
              <th className="px-4 py-3 text-left font-medium whitespace-nowrap">Request</th>
              <th className="px-4 py-3 text-left font-medium whitespace-nowrap">Issued by</th>
              {lockedCustomerId ? null : (
                <th className="px-4 py-3 text-left font-medium whitespace-nowrap">Bill to</th>
              )}
              <th className="px-4 py-3 text-right font-medium whitespace-nowrap">Amount</th>
              {lockedCustomerId ? null : (
                <>
                  <th className="px-4 py-3 text-left font-medium whitespace-nowrap">Status</th>
                  <th className="px-4 py-3 text-left font-medium whitespace-nowrap">Policy</th>
                  <th className="px-4 py-3 text-left font-medium whitespace-nowrap">
                    Verification
                  </th>
                  <th className="px-4 py-3 text-right font-medium whitespace-nowrap">Created</th>
                </>
              )}
            </tr>
          </thead>
          {invoices.map((invoice) => {
            const issuer = issuerName(invoice.issuer_id);
            const open = openId === invoice.id;
            const title = invoice.heading ?? invoice.reference ?? "Untitled request";
            return (
              // One body per request, so the summary and its detail are one
              // group and the rule between requests falls in the right place.
              <tbody key={invoice.id} className="border-b border-line last:border-b-0">
                <tr
                  onClick={() => toggle(invoice.id)}
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
                        toggle(invoice.id);
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
                      {invoice.has_attachment ? (
                        <Paperclip
                          className="size-3.5 shrink-0 text-faint"
                          aria-label="Has attachment"
                        />
                      ) : null}
                    </button>
                    {invoice.heading && invoice.reference ? (
                      <span className="mt-0.5 block pl-6 font-mono text-[12px] text-faint">
                        {invoice.reference}
                      </span>
                    ) : null}
                  </td>
                  <td className="px-4 py-3 text-left">
                    {issuer ? (
                      <span className="inline-flex max-w-[160px] items-center rounded-full border border-line-strong bg-raised px-2 py-0.5 text-[11.5px]">
                        <span className="truncate">{issuer}</span>
                      </span>
                    ) : (
                      <span className="text-faint">—</span>
                    )}
                  </td>
                  {lockedCustomerId ? null : (
                    <td className="truncate px-4 py-3 text-left">{invoice.bill_to_name}</td>
                  )}
                  <td className="px-4 py-3 text-right">
                    <Amount value={invoice.amount} />
                  </td>
                  {lockedCustomerId ? null : (
                    <>
                      <td className="px-4 py-3 text-left">
                        <StatusBadge status={invoice.status} />
                      </td>
                      <td className="truncate px-4 py-3 text-left">
                        {modeLabel(invoice.payer_policy_mode)}
                      </td>
                      <td className="px-4 py-3 text-left">
                        <VerificationBadge
                          mode={invoice.payer_policy_mode}
                          completedAt={invoice.verification_completed_at}
                          unsolicitedAt={invoice.likely_unsolicited_at}
                        />
                      </td>
                      <td className="px-4 py-3 text-right whitespace-nowrap text-muted">
                        {formatShortDate(invoice.created_at)}
                      </td>
                    </>
                  )}
                </tr>

                {/* Kept in the tree so its height can animate, and inert while
                    closed so nothing inside is reachable by tab or by a
                    screen reader. */}
                <tr>
                  <td colSpan={lockedCustomerId ? 3 : 8} className="p-0">
                    <div className="dash-expand" data-open={open}>
                      <div inert={!open} className="border-t border-line">
                        {fetched.has(invoice.id) ? <InvoiceRowDetail id={invoice.id} /> : null}
                      </div>
                    </div>
                  </td>
                </tr>
              </tbody>
            );
          })}
          {!page.loading && invoices.length === 0 ? (
            <tbody>
              <tr>
                <td colSpan={lockedCustomerId ? 3 : 8} className="px-4 py-10 text-center text-muted">
                  {filtered ? "No requests match these filters." : "No deposit requests yet."}
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
