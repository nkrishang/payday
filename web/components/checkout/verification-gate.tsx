import type { PayerPayment } from "@payday/sdk";
import type { CheckoutView } from "@/lib/checkout-state";
import { Lock } from "lucide-react";
import { EmailVerification } from "./email-verification";

/**
 * What a gated invoice shows before the payer has verified: the issuer, the
 * heading, a masked hint of the mailbox it was issued to, and the controls
 * for the email code. Nothing else is in the tree — the API does not send
 * the amount, parties, attachment, or address, and this component never
 * asks for them.
 */
export function VerificationGate({
  payment,
  view,
  payerSession,
  onSession,
  onCodeSent,
  onVerified,
}: {
  payment: PayerPayment;
  view: CheckoutView;
  payerSession: string | null;
  onSession: (token: string | null) => void;
  onCodeSent: () => void;
  onVerified: () => void;
}) {
  const { expected_email_hint } = payment.payer_policy;

  return (
    <div className="px-5 py-6 sm:px-6">
      <p className="text-[11px] font-medium tracking-[0.14em] text-faint uppercase">Invoice from</p>
      <h1 className="mt-1.5 text-[22px] font-semibold tracking-tight">{payment.issuer_name}</h1>
      {payment.heading ? (
        <p className="mt-1.5 text-[14px] leading-relaxed text-muted">{payment.heading}</p>
      ) : null}

      <section
        aria-labelledby="verification-required"
        className="mt-6 rounded-[10px] border border-line bg-raised px-4 py-4"
      >
        <h2
          id="verification-required"
          className="flex items-center gap-2 text-[14px] font-semibold tracking-tight"
        >
          <Lock className="size-4 text-muted" />
          {view.title}
        </h2>
        <p className="mt-2 text-[13px] leading-relaxed text-muted">
          The amount, payment details, and attachment are shown once the payer verifies the email
          address this invoice was issued to.
        </p>

        {expected_email_hint ? (
          <p className="mt-3 text-[13px] text-muted">
            Expected email{" "}
            <span className="font-mono font-medium text-ink">{expected_email_hint}</span>
          </p>
        ) : null}

        <div className="mt-4 border-t border-line pt-4">
          <EmailVerification
            paymentId={payment.id}
            hint={expected_email_hint}
            payerSession={payerSession}
            onSession={onSession}
            onCodeSent={onCodeSent}
            onVerified={onVerified}
          />
        </div>
      </section>
    </div>
  );
}
