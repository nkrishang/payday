import type { PayerPayment, PayerPolicyMode, VerificationFactStatus } from "@payday/sdk";
import type { CheckoutView } from "@/lib/checkout-state";
import { Lock, ShieldCheck } from "lucide-react";
import { EmailVerification } from "./email-verification";

/**
 * What a gated invoice shows before the payer has verified: the issuer, the
 * heading, a masked hint of the mailbox it was issued to, what is required,
 * and the controls for the step that is due. Nothing else is in the tree —
 * the API does not send the amount, parties, attachment, or address, and this
 * component never asks for them.
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
  const { mode, expected_email_hint } = payment.payer_policy;
  const identityDue = view.phase === "identity_required";

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
          {identityDue ? (
            <ShieldCheck className="size-4 text-muted" />
          ) : (
            <Lock className="size-4 text-muted" />
          )}
          {view.title}
        </h2>
        <p className="mt-2 text-[13px] leading-relaxed text-muted">
          {identityDue ? view.detail : explanation(mode)}
        </p>

        {expected_email_hint ? (
          <p className="mt-3 text-[13px] text-muted">
            Expected email{" "}
            <span className="font-mono font-medium text-ink">{expected_email_hint}</span>
          </p>
        ) : null}

        <Requirements payment={payment} />

        {identityDue ? (
          <p className="mt-4 border-t border-line pt-3 text-[12px] leading-relaxed text-faint">
            Identity verification is being enabled for this invoice. Nothing can be paid from this
            page until it completes.
          </p>
        ) : (
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
        )}
      </section>

      <p className="mt-4 text-[12px] leading-relaxed text-faint">
        Verification proves who may view and pay this invoice. It does not prove ownership of the
        wallet that sends the funds.
      </p>
    </div>
  );
}

function explanation(mode: PayerPolicyMode): string {
  switch (mode) {
    case "verified_identity":
      return "The amount, payment details, and attachment are shown once the payer verifies the email address this invoice was issued to and completes an identity check matching the person it names.";
    case "verified_identity_unattributed":
      return "The amount, payment details, and attachment are shown once the payer verifies the email address this invoice was issued to and completes an identity check.";
    case "verified_email":
    case "permissionless":
      return "The amount, payment details, and attachment are shown once the payer verifies the email address this invoice was issued to.";
  }
}

const FACTS: ReadonlyArray<{
  key: "email" | "document" | "liveness" | "identity_match";
  label: string;
}> = [
  { key: "email", label: "Email ownership" },
  { key: "document", label: "Identity document" },
  { key: "liveness", label: "Liveness" },
  { key: "identity_match", label: "Name matches the invoice" },
];

const STATUS_LABEL: Record<Exclude<VerificationFactStatus, "not_required">, string> = {
  pending: "Pending",
  approved: "Approved",
  declined: "Declined",
};

/** The facts this policy needs, so the payer knows what verifying will involve. */
function Requirements({ payment }: { payment: PayerPayment }) {
  const required = FACTS.filter((fact) => payment.requirements[fact.key] !== "not_required");
  if (required.length === 0) return null;

  return (
    <ul className="mt-3 space-y-1.5 text-[13px]" aria-label="Verification requirements">
      {required.map((fact) => {
        const status = payment.requirements[fact.key] as Exclude<
          VerificationFactStatus,
          "not_required"
        >;
        return (
          <li key={fact.key} className="flex items-baseline justify-between gap-3">
            <span className="text-muted">{fact.label}</span>
            <span className="tabular text-faint">{STATUS_LABEL[status]}</span>
          </li>
        );
      })}
    </ul>
  );
}
