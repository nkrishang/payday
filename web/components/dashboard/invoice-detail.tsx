"use client";

import { type Payment, PaydayError } from "@payday/sdk";
import { ArrowUpRight, Download, FileText, Loader2 } from "lucide-react";
import Link from "next/link";
import { useState, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { CopyButton } from "@/components/ui/copy-button";
import { Problem } from "@/components/ui/field";
import { describeError } from "@/lib/attachment-upload";
import { saveBlob, saveJson } from "@/lib/download";
import { formatBytes, formatDisplayAmount, truncateAddress, truncateHash } from "@/lib/format";
import { formatDate } from "./labels";
import { RecoveredFunds } from "./recovered-funds";
import { useMerchant, useResource } from "./session";
import { StatusBadge } from "./status-badge";
import { VerificationActivity } from "./verification-activity";
import { VerificationStatus } from "./verification-status";

export function InvoiceDetail({ id }: { id: string }) {
  const invoice = useResource(id, (client) => client.payments.get(id));

  if (invoice.error) {
    return (
      <div className="grid gap-3">
        <Problem>{invoice.error}</Problem>
        <div>
          <Button variant="secondary" size="sm" onClick={invoice.reload}>
            Try again
          </Button>
        </div>
      </div>
    );
  }
  if (!invoice.data) {
    return (
      <p role="status" className="text-[13px] text-faint">
        Loading…
      </p>
    );
  }
  return <Loaded payment={invoice.data} />;
}

function Loaded({ payment }: { payment: Payment }) {
  const title = payment.heading ?? payment.reference ?? "Invoice";

  return (
    <div className="grid gap-5">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div>
          <p className="font-mono text-[12px] text-faint">{payment.id}</p>
          <h1 className="mt-1 text-[22px] font-semibold tracking-tight">{title}</h1>
          <div className="mt-2 flex flex-wrap items-center gap-3">
            <StatusBadge status={payment.status} />
            <span className="tabular text-[13px] text-muted">
              {formatDisplayAmount(payment.amount)} {payment.token.symbol}
            </span>
          </div>
        </div>
        <div className="flex items-center gap-2 rounded-[10px] border border-line bg-surface py-1.5 pr-1.5 pl-3">
          <a
            href={payment.payment_url}
            target="_blank"
            rel="noreferrer noopener"
            className="inline-flex items-center gap-1 font-mono text-[12px] text-muted transition-colors hover:text-ink"
          >
            {payment.payment_url.replace(/^https?:\/\//, "")}
            <ArrowUpRight className="size-3" />
          </a>
          <CopyButton value={payment.payment_url} label="payment link" className="bg-raised" />
        </div>
      </div>

      <div className="grid gap-5 lg:grid-cols-[minmax(0,1fr)_340px]">
        <div className="grid gap-5">
          <Card title="Document">
            <Rows>
              <Row label="Issuer">
                <PartyBlock party={payment.issuer} />
              </Row>
              <Row label="Bill to">
                <PartyBlock party={payment.bill_to} />
              </Row>
              {payment.customer_id ? (
                <Row label="Customer">
                  <Link
                    href={`/dashboard/customers/${encodeURIComponent(payment.customer_id)}`}
                    className="font-mono text-[12px] underline underline-offset-2"
                  >
                    {payment.customer_id}
                  </Link>
                </Row>
              ) : null}
              {payment.heading ? <Row label="Heading">{payment.heading}</Row> : null}
              {payment.reference ? (
                <Row label="Reference">
                  <span className="font-mono">{payment.reference}</span>
                </Row>
              ) : null}
              {payment.notes ? (
                <Row label="Notes">
                  <span className="whitespace-pre-wrap">{payment.notes}</span>
                </Row>
              ) : null}
              <Row label="Created">{formatDate(payment.created_at)}</Row>
              <Row label="Expires">{formatDate(payment.expires_at)}</Row>
              <Row label="Attribution">
                <span className="font-mono text-[12px] break-all" title={payment.attribution.hash}>
                  v{payment.attribution.version} · {payment.attribution.hash}
                </span>
              </Row>
            </Rows>
          </Card>

          <Card title="Payment">
            <Rows>
              <Row label="Address">
                <span className="inline-flex items-center gap-1 font-mono break-all">
                  {payment.address}
                  <CopyButton value={payment.address} label="payment address" />
                </span>
              </Row>
              <Row label="Payout to">
                <span className="font-mono">{truncateAddress(payment.payout_address, 10, 8)}</span>
              </Row>
              <Row label="Network">
                {payment.chain.name} · {payment.token.symbol}{" "}
                <span className="font-mono text-faint">
                  {truncateAddress(payment.token.address)}
                </span>
              </Row>
              <Row label="Received">
                <span className="tabular">
                  {formatDisplayAmount(payment.received)} of {formatDisplayAmount(payment.amount)}{" "}
                  {payment.token.symbol}
                </span>
              </Row>
              {payment.paid_at ? <Row label="Funded">{formatDate(payment.paid_at)}</Row> : null}
              {payment.settled_at ? (
                <Row label="Settled">{formatDate(payment.settled_at)}</Row>
              ) : null}
              {payment.settlement_tx_hash ? (
                <Row label="Settlement">
                  {payment.settlement_explorer_url ? (
                    <a
                      href={payment.settlement_explorer_url}
                      target="_blank"
                      rel="noreferrer noopener"
                      className="inline-flex items-center gap-1 font-mono"
                    >
                      {truncateHash(payment.settlement_tx_hash)}
                      <ArrowUpRight className="size-3" />
                    </a>
                  ) : (
                    <span className="font-mono">{truncateHash(payment.settlement_tx_hash)}</span>
                  )}
                </Row>
              ) : null}
              {payment.attention ? (
                <Row label="Attention">
                  <span className="text-warning">
                    {payment.attention.message} {payment.attention.action}
                  </span>
                </Row>
              ) : null}
            </Rows>
          </Card>

          <RecoveredFunds payment={payment} />
        </div>

        <div className="grid content-start gap-5">
          <Card title="Verification">
            <VerificationStatus
              policy={payment.payer_policy}
              completedAt={payment.verification_completed_at}
              unsolicitedAt={payment.likely_unsolicited_at}
            />
            {payment.payer_policy.mode !== "permissionless" ? (
              <div className="mt-4 border-t border-line pt-4">
                <VerificationActivity paymentId={payment.id} />
              </div>
            ) : null}
          </Card>

          <Card title="Attachment">
            {payment.attachment ? (
              <AttachmentDownload payment={payment} />
            ) : (
              <p className="text-[13px] text-muted">No PDF was attached at issuance.</p>
            )}
          </Card>

          <Card title="Downloads">
            <Downloads payment={payment} />
          </Card>
        </div>
      </div>
    </div>
  );
}

function AttachmentDownload({ payment }: { payment: Payment }) {
  const { client } = useMerchant();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const attachment = payment.attachment;
  if (!attachment) return null;

  const open = async () => {
    setBusy(true);
    setError(null);
    try {
      const descriptor = await client.payments.attachment(payment.id);
      if (!descriptor.download_url) throw new Error("The API returned no download link");
      window.open(descriptor.download_url, "_blank", "noopener,noreferrer");
    } catch (cause) {
      setError(describeError(cause));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid gap-3">
      <div className="flex items-start gap-3 text-[13px]">
        <FileText className="mt-0.5 size-4 shrink-0 text-muted" />
        <span className="min-w-0">
          <span className="block truncate font-medium">{attachment.filename}</span>
          <span className="block text-[12px] text-faint">
            {formatBytes(attachment.byte_length)}
          </span>
          <span className="block font-mono text-[11px] break-all text-faint" title="SHA-256">
            {attachment.sha256}
          </span>
        </span>
      </div>
      <Button variant="secondary" size="sm" onClick={open} disabled={busy}>
        {busy ? <Loader2 className="size-4 animate-spin" /> : <Download className="size-4" />}
        Download attachment
      </Button>
      <Problem>{error}</Problem>
    </div>
  );
}

function Downloads({ payment }: { payment: Payment }) {
  const { client } = useMerchant();
  const [busy, setBusy] = useState<"pdf" | "proof" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const stem = payment.reference?.replace(/[^\w.-]+/g, "_") || payment.id;

  const run = async (kind: "pdf" | "proof", action: () => Promise<void>) => {
    setBusy(kind);
    setError(null);
    try {
      await action();
    } catch (cause) {
      setError(
        cause instanceof PaydayError && cause.code === "payment_not_settled"
          ? "The Proof of Payment is available once the invoice is settled."
          : describeError(cause),
      );
    } finally {
      setBusy(null);
    }
  };

  return (
    <div className="grid gap-3">
      <Button
        variant="secondary"
        size="sm"
        disabled={busy !== null}
        onClick={() =>
          run("pdf", async () =>
            saveBlob(await client.payments.invoicePdf(payment.id), `${stem}.pdf`),
          )
        }
      >
        {busy === "pdf" ? (
          <Loader2 className="size-4 animate-spin" />
        ) : (
          <Download className="size-4" />
        )}
        Invoice PDF
      </Button>
      <Button
        variant="secondary"
        size="sm"
        disabled={busy !== null || payment.status !== "settled"}
        onClick={() =>
          run("proof", async () =>
            saveJson(await client.payments.proof(payment.id), `${stem}-proof.json`),
          )
        }
      >
        {busy === "proof" ? (
          <Loader2 className="size-4 animate-spin" />
        ) : (
          <Download className="size-4" />
        )}
        Proof of Payment
      </Button>
      {payment.status !== "settled" ? (
        <p className="text-[12px] leading-relaxed text-faint">
          The proof is generated once the invoice settles. It ties this document to its address and
          the settling transfer, and verifies offline with <code>payday proof verify</code>.
        </p>
      ) : null}
      <Problem>{error}</Problem>
    </div>
  );
}

function Card({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="rounded-[16px] border border-line bg-surface p-5">
      <h2 className="text-[13px] font-semibold tracking-tight">{title}</h2>
      <div className="mt-4">{children}</div>
    </section>
  );
}

function Rows({ children }: { children: ReactNode }) {
  return (
    <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-5 gap-y-2.5 text-[13px]">
      {children}
    </dl>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt className="text-faint">{label}</dt>
      <dd className="min-w-0">{children}</dd>
    </>
  );
}

function PartyBlock({ party }: { party: Payment["issuer"] }) {
  return (
    <>
      <span className="font-medium">{party.name}</span>
      {party.email ? <span className="block text-muted">{party.email}</span> : null}
      {party.details ? (
        <span className="block whitespace-pre-wrap text-muted">{party.details}</span>
      ) : null}
    </>
  );
}
