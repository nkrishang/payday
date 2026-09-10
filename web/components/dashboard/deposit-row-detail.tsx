"use client";

import { type DepositRequest, PaydayError } from "@payday/sdk";
import {
  ArrowUpRight,
  Download,
  ExternalLink,
  FileText,
  Loader2,
  Paperclip,
  ShieldCheck,
} from "lucide-react";
import Link from "next/link";
import { useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { Problem } from "@/components/ui/field";
import { describeError } from "@/lib/attachment-upload";
import { cn } from "@/lib/cn";
import { chainById } from "@/lib/config";
import { saveBlob, saveJson } from "@/lib/download";
import {
  explorerAddressUrl,
  formatBytes,
  formatDisplayAmount,
  receivedPercent,
  truncateAddress,
  truncateHash,
} from "@/lib/format";
import { usePayerPreviewUrl } from "@/lib/payer-session";
import { formatDate, formatRelative } from "./labels";
import { RecoveredFunds } from "./recovered-funds";
import { useMerchant, useResource } from "./session";
import { StatusBadge } from "./status-badge";
import { VerificationActivity } from "./verification-activity";
import { VerificationStatus } from "./verification-status";

/**
 * One deposit request in full, inside its own row.
 *
 * The row above already carries what a list is scanned for — who and how
 * much. This answers the rest, starting with where it stands — a picture
 * rather than a field, because that reads faster than a label. What follows
 * is grouped by what a merchant came for next — the money, the payer's proof,
 * and the files — with nothing repeated from the row.
 */
export function DepositRowDetail({ id }: { id: string }) {
  const request = useResource(id, (client) => client.depositRequests.get(id));

  if (request.error) {
    return (
      <div className="grid gap-3 px-4 py-5">
        <Problem>{request.error}</Problem>
        <div>
          <Button variant="secondary" size="sm" onClick={request.reload}>
            Try again
          </Button>
        </div>
      </div>
    );
  }
  if (!request.data) {
    return (
      <p role="status" className="px-4 py-6 text-[13px] text-faint">
        Loading…
      </p>
    );
  }
  return <Loaded payment={request.data} />;
}

const MONO = "font-mono text-[12.5px] break-all";
/** Reads as a link before it is hovered, which is the point of a link. */
const LINK =
  "text-brand-green underline decoration-brand-green/40 underline-offset-2 transition-colors hover:decoration-brand-green";

function Loaded({ payment }: { payment: DepositRequest }) {
  const { client } = useMerchant();
  const previewHref = usePayerPreviewUrl(client, payment);
  // One reading of the clock for the whole row, so its relative times agree
  // with each other and do not shift between renders.
  const [now] = useState(() => Date.now());
  const percent = receivedPercent(payment.received_base_units, payment.amount_base_units);
  const settled = payment.status === "settled";
  const failed = payment.status === "expired" || payment.status === "returned";
  const gated = payment.payer_policy.mode !== "permissionless";
  const live = !settled && !failed;
  // A live request past its deadline is waiting on the sweep, not on a payer.
  const overdue = new Date(payment.expires_at).getTime() < now;

  // Explorer links follow the network the payer chose; none until then.
  const explorer = chainById(payment.chain?.id)?.explorerUrl ?? null;
  return (
    <div className="grid min-w-0 gap-4 px-4 pt-4 pb-5">
      {/* How much has arrived, its status, and its milestones: the picture a
          merchant opens a row to see, kept to one tight group rather than
          spread across the page. */}
      <div className="grid gap-4">
        <div className="flex flex-wrap items-center justify-between gap-x-4 gap-y-1.5">
          <span className="flex flex-wrap items-center gap-x-3 gap-y-1">
            <StatusBadge status={payment.status} />
            <span className="text-faint/60" aria-hidden="true">
              ·
            </span>
            <span className="text-[13px]">
              <span className="tabular font-medium">{formatDisplayAmount(payment.received)}</span>
              <span className="text-faint">
                {" "}
                of {formatDisplayAmount(payment.amount)} {payment.currency} received
              </span>
            </span>
          </span>
          {/* Only while it can still be funded: once a request has settled or
              closed, its deadline is history the line below tells. */}
          {live ? (
            <span className={cn("text-[12px]", overdue ? "text-warning/80" : "text-faint")}>
              {overdue ? "Deadline passed" : "Expires"} {formatRelative(payment.expires_at, now)}
              <span className="text-faint/70"> · {formatDate(payment.expires_at)}</span>
            </span>
          ) : null}
        </div>
        {/* The track carries the role, not the fill: an empty request fills
            nothing, and a zero-width element is no control at all. */}
        <div
          role="progressbar"
          aria-valuenow={percent}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-label="Received"
          className="h-1.5 overflow-hidden rounded-full bg-raised"
        >
          <div
            style={{ width: `${percent > 0 ? Math.max(percent, 2) : 0}%` }}
            className={cn(
              "h-full rounded-full transition-[width] duration-500 ease-out",
              settled ? "bg-brand-green" : failed ? "bg-warning/70" : "bg-brand-yellow",
            )}
          />
        </div>
      </div>

      {/* What the payer sees, abbreviated to the link itself: the full preview
          used to sit here as its own card, but it only ever restated what
          this row already shows the merchant. */}
      <p className="flex min-w-0 items-center gap-1.5 text-[12.5px]">
        <span className="shrink-0 text-faint">Payer&apos;s view</span>
        <a
          href={previewHref}
          target="_blank"
          rel="noreferrer noopener"
          className={cn(MONO, LINK, "min-w-0 truncate")}
        >
          {payment.deposit_url.replace(/^https?:\/\//, "")}
        </a>
        <ExternalLink aria-hidden="true" className="size-3 shrink-0 text-faint" />
      </p>

      {payment.notes || payment.payer.details || payment.customer_id ? (
        <Panel label="Document">
          <Facts>
            {payment.payer.email ? (
              <Fact label="Payer email">{payment.payer.email}</Fact>
            ) : null}
            {payment.payer.details ? (
              <Fact label="Their details">
                <span className="whitespace-pre-wrap">{payment.payer.details}</span>
              </Fact>
            ) : null}
            {payment.notes ? (
              <Fact label="Notes">
                <span className="whitespace-pre-wrap">{payment.notes}</span>
              </Fact>
            ) : null}
            {payment.customer_id ? (
              <Fact label="Customer">
                <Link
                  href={`/dashboard/customers/${encodeURIComponent(payment.customer_id)}`}
                  className="underline underline-offset-2"
                >
                  Open customer
                </Link>
              </Fact>
            ) : null}
          </Facts>
        </Panel>
      ) : null}

      {gated ? (
        <Panel label="Verification">
          <VerificationStatus
            policy={payment.payer_policy}
            completedAt={payment.verification_completed_at}
            unsolicitedAt={payment.likely_unsolicited_at}
          />
          {/* Brings its own rule, and nothing at all when it would only
              restate the verdict above. */}
          <VerificationActivity paymentId={payment.id} />
        </Panel>
      ) : null}

      <Panel label="DepositRequest">
        <Facts>
          <Fact label="Address">
            {payment.address ? (
              <AddressLink
                address={payment.address}
                href={
                  payment.address_explorer_url ??
                  explorerAddressUrl(explorer, payment.address)
                }
              />
            ) : (
              <span className="text-muted">
                Created once the payer signs from the wallet they will pay from
              </span>
            )}
          </Fact>
          {payment.payer_wallet ? (
            <Fact label="Payer wallet">
              <AddressLink
                address={payment.payer_wallet}
                href={explorerAddressUrl(explorer, payment.payer_wallet)}
              />
              {payment.wallet_bound_at ? (
                <span className="block text-[12px] text-faint">
                  Signed {formatDate(payment.wallet_bound_at)}. Only its transfers count; excess
                  and late funds return to it.
                </span>
              ) : null}
            </Fact>
          ) : null}
          <Fact label="Settles to">
            <AddressLink
              address={payment.payout_address}
              href={explorerAddressUrl(explorer, payment.payout_address)}
            />
          </Fact>
          <Fact label="Network">
            {payment.chain && payment.token ? (
              `${payment.chain.name} · ${payment.token.symbol}`
            ) : (
              <span className="text-muted">
                Payer&apos;s choice: {payment.networks.map((network) => network.chain.name).join(", ")}
              </span>
            )}
          </Fact>
          {payment.deposited_at ? <Fact label="Deposited">{formatDate(payment.deposited_at)}</Fact> : null}
          {payment.settlement_tx_hash ? (
            <Fact label="Settlement">
              {payment.settlement_explorer_url ? (
                <a
                  href={payment.settlement_explorer_url}
                  target="_blank"
                  rel="noreferrer noopener"
                  className={cn(MONO, LINK)}
                >
                  {truncateHash(payment.settlement_tx_hash)}
                  <ArrowUpRight aria-hidden="true" className="ml-1 inline size-3 align-[-1px]" />
                </a>
              ) : (
                <span className={MONO}>{truncateHash(payment.settlement_tx_hash)}</span>
              )}
            </Fact>
          ) : null}
        </Facts>
        {payment.attention ? (
          <p
            role="note"
            className="mt-3 rounded-[10px] border border-warning/40 px-3 py-2.5 text-[12.5px] leading-relaxed text-warning"
          >
            {payment.attention.message} {payment.attention.action}
          </p>
        ) : null}
      </Panel>

      <RecoveredFunds payment={payment} />

      <Files payment={payment} />
    </div>
  );
}

/** The attached PDF and the two documents the API generates from the request. */
function Files({ payment }: { payment: DepositRequest }) {
  const { client } = useMerchant();
  const [busy, setBusy] = useState<"attachment" | "pdf" | "proof" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const stem = payment.reference?.replace(/[^\w.-]+/g, "_") || payment.id;
  const attachment = payment.attachment;

  const run = async (kind: "attachment" | "pdf" | "proof", action: () => Promise<void>) => {
    setBusy(kind);
    setError(null);
    try {
      await action();
    } catch (cause) {
      setError(
        cause instanceof PaydayError && cause.code === "deposit_request_not_settled"
          ? "The Proof of Payment is available once the request settles."
          : describeError(cause),
      );
    } finally {
      setBusy(null);
    }
  };

  const spinner = (kind: "attachment" | "pdf" | "proof", icon: ReactNode) =>
    busy === kind ? <Loader2 className="size-4 animate-spin" /> : icon;

  return (
    <section
      aria-label="Files"
      className="min-w-0 rounded-[16px] border border-line bg-surface p-5"
    >
      <h3 className="text-[13px] font-semibold tracking-tight">Files</h3>

      {attachment ? (
        <div className="mt-3 flex items-start gap-2.5 rounded-[10px] border border-line bg-raised px-3 py-2.5">
          <FileText className="mt-0.5 size-4 shrink-0 text-muted" />
          <span className="min-w-0 flex-1">
            <span className="block truncate text-[12.5px] font-medium">{attachment.filename}</span>
            <span className="block text-[11.5px] text-faint">
              {formatBytes(attachment.byte_length)} · SHA-256{" "}
              <span className="font-mono" title={attachment.sha256}>
                {truncateAddress(attachment.sha256, 8, 6)}
              </span>
            </span>
          </span>
        </div>
      ) : (
        <p className="mt-3 flex items-center gap-2 text-[12.5px] text-faint">
          <Paperclip className="size-3.5" />
          No PDF was attached at issuance.
        </p>
      )}

      <div className="mt-4 grid gap-2">
        {attachment ? (
          <Button
            variant="secondary"
            size="sm"
            disabled={busy !== null}
            onClick={() =>
              run("attachment", async () => {
                const descriptor = await client.depositRequests.attachment(payment.id);
                if (!descriptor.download_url) throw new Error("The API returned no download link");
                window.open(descriptor.download_url, "_blank", "noopener,noreferrer");
              })
            }
          >
            {spinner("attachment", <Download className="size-4" />)}
            Download attachment
          </Button>
        ) : null}
        <Button
          variant="secondary"
          size="sm"
          disabled={busy !== null}
          onClick={() =>
            run("pdf", async () =>
              saveBlob(await client.depositRequests.requestPdf(payment.id), `${stem}.pdf`),
            )
          }
        >
          {spinner("pdf", <FileText className="size-4" />)}
          Deposit request PDF
        </Button>
        <Button
          variant="secondary"
          size="sm"
          disabled={busy !== null || payment.status !== "settled"}
          onClick={() =>
            run("proof", async () =>
              saveJson(await client.depositRequests.proof(payment.id), `${stem}-proof.json`),
            )
          }
        >
          {spinner("proof", <ShieldCheck className="size-4" />)}
          Proof of Payment
        </Button>
      </div>

      {payment.status !== "settled" ? (
        <p className="mt-3 text-[11.5px] leading-relaxed text-faint">
          The proof is generated once the request settles, and verifies offline
          without Payday.
        </p>
      ) : null}
      <div className="mt-2">
        <Problem>{error}</Problem>
      </div>
    </section>
  );
}

/**
 * An address in full, on the explorer.
 *
 * Wrapped rather than shortened: the whole string is what a merchant compares
 * against a wallet, and a column too narrow for it breaks the line instead of
 * hiding characters. Plain text when the deployment has no explorer — a local
 * chain has no page to send anyone to, and a dead link is worse than none.
 */
function AddressLink({ address, href }: { address: string; href: string | null }) {
  if (!href) return <span className={MONO}>{address}</span>;
  return (
    <a href={href} target="_blank" rel="noreferrer noopener" className={cn(MONO, LINK)}>
      {address}
      <ArrowUpRight aria-hidden="true" className="ml-1 inline size-3 align-[-1px]" />
    </a>
  );
}

function Panel({ label, children }: { label: string; children: ReactNode }) {
  return (
    <section
      aria-label={label}
      className="min-w-0 rounded-[14px] border border-line bg-raised/60 p-4"
    >
      <h3 className="text-[11px] font-medium tracking-[0.1em] text-faint uppercase">{label}</h3>
      <div className="mt-3">{children}</div>
    </section>
  );
}

function Facts({ children }: { children: ReactNode }) {
  return (
    <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-5 gap-y-2 text-[13px]">{children}</dl>
  );
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-faint">{label}</dt>
      <dd className="min-w-0">{children}</dd>
    </>
  );
}
