import type { Party } from "@payday/sdk";
import { tokenSymbol, type UnlockedPayerDepositRequest } from "@/lib/checkout-state";
import { formatDisplayAmount } from "@/lib/format";
import { AttachmentLink } from "./attachment-link";

/**
 * The deposit request as a document: who issued it, who is asked to deposit,
 * what it references, and the amount. It sits above the deposit mechanics in
 * every unlocked state, so a funded request still reads as the request it was.
 *
 * Party `details` are bounded free text the merchant wrote; they are rendered
 * verbatim with line breaks preserved and are never parsed.
 */
export function RequestDetails({
  payment,
  payerSession = null,
}: {
  payment: UnlockedPayerDepositRequest;
  /** This tab's session, which the attachment fetch must present for a gated deposit request. */
  payerSession?: string | null;
}) {
  const details = payment.details;

  return (
    <section aria-label="Deposit request" className="border-b border-line px-5 py-5 sm:px-6">
      <p className="text-[11px] font-medium tracking-[0.14em] text-faint uppercase">Deposit request</p>
      {payment.heading ? (
        <h2 className="mt-1.5 text-[16px] font-semibold tracking-tight">{payment.heading}</h2>
      ) : null}

      <dl className="mt-3 grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 text-[13px]">
        <dt className="text-faint">From</dt>
        <dd className="min-w-0 font-medium">{payment.issuer_name}</dd>

        {details ? (
          <>
            <dt className="text-faint">Payer</dt>
            <dd className="min-w-0">
              <PartyLine party={details.payer} />
            </dd>

            {details.reference ? (
              <>
                <dt className="text-faint">Reference</dt>
                <dd className="min-w-0 font-mono break-all">{details.reference}</dd>
              </>
            ) : null}

            <dt className="text-faint">Amount</dt>
            <dd className="tabular font-medium">
              {formatDisplayAmount(details.amount)} {tokenSymbol(payment)}
            </dd>
          </>
        ) : null}
      </dl>

      {details?.notes ? (
        <p className="mt-3 text-[13px] leading-relaxed whitespace-pre-wrap text-muted">
          {details.notes}
        </p>
      ) : null}

      {details?.attachment ? (
        <div className="mt-4">
          <AttachmentLink
            paymentId={payment.id}
            attachment={details.attachment}
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
