import type { PayerDepositRequest } from "@gum/sdk";
import type { CheckoutView } from "@/lib/checkout-state";
import { Lock } from "lucide-react";
import { EmailVerification } from "./email-verification";
import { MerchantSessionGate, type ClientSecretStatus } from "./merchant-session";

/**
 * What a gated deposit request shows before the payer has verified: the issuer, the
 * heading, and the way in that each pending identity add-on allows. For an
 * email-gated request that is a masked hint of the mailbox and the controls
 * for the code; for a merchant-auth request it is the name of the app that
 * opens it, because nothing on this page can. Both can be pending at once,
 * and then both ways in are shown. Nothing else is in the tree — the API
 * does not send the amount, parties, attachment, or address, and this
 * component never asks for them.
 */
export function VerificationGate({
  payment,
  view,
  payerSession,
  clientSecretStatus,
  onSession,
  onCodeSent,
  onVerified,
}: {
  payment: PayerDepositRequest;
  view: CheckoutView;
  payerSession: string | null;
  clientSecretStatus: ClientSecretStatus;
  onSession: (token: string | null) => void;
  onCodeSent: () => void;
  onVerified: () => void;
}) {
  const { expected_email_hint, requirements } = payment;
  const merchantSession = requirements.merchant_session === "pending";
  const emailPending = requirements.email === "pending";

  return (
    <div className="px-5 py-6 sm:px-6">
      <p className="text-[11px] font-medium tracking-[0.14em] text-faint uppercase">
        Deposit request from
      </p>
      <h1 className="mt-1.5 text-[22px] font-semibold tracking-tight">{payment.issuer_name}</h1>
      {payment.heading ? (
        <p className="mt-1.5 text-[14px] leading-relaxed text-muted">{payment.heading}</p>
      ) : null}

      <section
        aria-labelledby="verification-required"
        className="mt-6 rounded-[8px] border border-line bg-surface px-4 py-4"
      >
        <h2
          id="verification-required"
          className="flex items-center gap-2 text-[14px] font-semibold tracking-tight"
        >
          <Lock className="size-4 text-muted" />
          {view.title}
        </h2>

        {merchantSession ? (
          <div className="mt-2">
            <MerchantSessionGate issuerName={payment.issuer_name} status={clientSecretStatus} />
          </div>
        ) : null}

        {emailPending ? (
          <>
            <p className="mt-2 text-[13px] leading-relaxed text-muted">
              Payment details are disclosed on this page once you verify ownership of the expected
              credentials.
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
          </>
        ) : null}
      </section>
    </div>
  );
}
