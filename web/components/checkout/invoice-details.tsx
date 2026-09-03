import type { Party } from "@payday/sdk";
import type { UnlockedPayerPayment } from "@/lib/checkout-state";
import { formatDisplayAmount } from "@/lib/format";
import { AttachmentLink } from "./attachment-link";

/**
 * The invoice as a document: who issued it, who it bills, what it references,
 * and the amount it was issued for. It sits above the payment mechanics in
 * every unlocked state, so a paid invoice still reads as the invoice it was.
 *
 * Party `details` are bounded free text the merchant wrote; they are rendered
 * verbatim with line breaks preserved and are never parsed.
 */
export function InvoiceDetails({
  payment,
  payerSession = null,
}: {
  payment: UnlockedPayerPayment;
  /** This tab's session, which the attachment fetch must present for a gated invoice. */
  payerSession?: string | null;
}) {
  const invoice = payment.invoice;

  return (
    <section aria-label="Invoice" className="border-b border-line px-5 py-5 sm:px-6">
      <p className="text-[11px] font-medium tracking-[0.14em] text-faint uppercase">Invoice</p>
      {payment.heading ? (
        <h2 className="mt-1.5 text-[16px] font-semibold tracking-tight">{payment.heading}</h2>
      ) : null}

      <dl className="mt-3 grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 text-[13px]">
        <dt className="text-faint">From</dt>
        <dd className="min-w-0 font-medium">{payment.issuer_name}</dd>

        {invoice ? (
          <>
            <dt className="text-faint">Bill to</dt>
            <dd className="min-w-0">
              <PartyLine party={invoice.bill_to} />
            </dd>

            {invoice.reference ? (
              <>
                <dt className="text-faint">Reference</dt>
                <dd className="min-w-0 font-mono break-all">{invoice.reference}</dd>
              </>
            ) : null}

            <dt className="text-faint">Amount</dt>
            <dd className="tabular font-medium">
              {formatDisplayAmount(invoice.amount)} {payment.token.symbol}
            </dd>
          </>
        ) : null}
      </dl>

      {invoice?.notes ? (
        <p className="mt-3 text-[13px] leading-relaxed whitespace-pre-wrap text-muted">
          {invoice.notes}
        </p>
      ) : null}

      {invoice?.attachment ? (
        <div className="mt-4">
          <AttachmentLink
            paymentId={payment.id}
            attachment={invoice.attachment}
            payerSession={payerSession}
          />
        </div>
      ) : null}
    </section>
  );
}

function PartyLine({ party }: { party: Party }) {
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
