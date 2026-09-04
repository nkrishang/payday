import type { CreatePayment, Party, PayerPolicy, PayerPolicyMode } from "@payday/sdk";

/**
 * The `POST /v1/payments` body, built from what the composer collected.
 *
 * The composer asks its questions over four steps and keeps a draft of
 * strings; this turns that draft into the request the SDK sends. Only the
 * shape is decided here — limits, policy rules, and the expiry window are the
 * API's to enforce, and its message is shown verbatim when it refuses.
 */

export interface PaymentValues {
  /** The saved identity this is issued under, when there is one. */
  issuerId: string;
  issuerName: string;
  issuerEmail: string;
  issuerDetails: string;
  customerId: string;
  billName: string;
  billEmail: string;
  billDetails: string;
  amount: string;
  payoutAddress: string;
  /** Hours; empty means the API's default deadline. */
  expiresInHours: string;
  /** An absolute RFC 3339 moment, which wins over `expiresInHours` when set. */
  expiresAt: string;
  heading: string;
  reference: string;
  notes: string;
  mode: PayerPolicyMode;
  expectedEmail: string;
  firstName: string;
  lastName: string;
}

export const EMPTY_VALUES: PaymentValues = {
  issuerId: "",
  issuerName: "",
  issuerEmail: "",
  issuerDetails: "",
  customerId: "",
  billName: "",
  billEmail: "",
  billDetails: "",
  amount: "",
  payoutAddress: "",
  expiresInHours: "",
  expiresAt: "",
  heading: "",
  reference: "",
  notes: "",
  mode: "permissionless",
  expectedEmail: "",
  firstName: "",
  lastName: "",
};

function party(name: string, email: string, details: string): Party {
  return {
    name: name.trim(),
    ...(email.trim() ? { email: email.trim() } : {}),
    ...(details.trim() ? { details: details.trim() } : {}),
  };
}

function policy(values: PaymentValues): PayerPolicy {
  const expected_email = values.expectedEmail.trim();
  switch (values.mode) {
    case "permissionless":
      return { mode: "permissionless" };
    case "verified_email":
      return { mode: "verified_email", expected_email };
    case "verified_identity":
      return {
        mode: "verified_identity",
        expected_email,
        expected_identity: {
          first_name: values.firstName.trim(),
          last_name: values.lastName.trim(),
        },
      };
    case "verified_identity_unattributed":
      return { mode: "verified_identity_unattributed", expected_email };
  }
}

export function buildCreatePayment(
  values: PaymentValues,
  attachmentId: string | null,
): CreatePayment {
  const optional = (value: string) => (value.trim() ? { value: value.trim() } : null);
  const heading = optional(values.heading);
  const reference = optional(values.reference);
  const notes = optional(values.notes);
  const hours = Number(values.expiresInHours);
  const expiresIn =
    values.expiresInHours.trim() && Number.isFinite(hours) && hours > 0
      ? Math.round(hours * 3600)
      : null;
  // A chosen moment is sent as the moment. Converting it to a duration would
  // re-anchor it to whenever the request happened to arrive, which is not what
  // "expires at 5pm on Friday" means.
  const expiresAt = optional(values.expiresAt);

  return {
    amount: values.amount.trim(),
    payout_address: values.payoutAddress.trim(),
    issuer: party(values.issuerName, values.issuerEmail, values.issuerDetails),
    bill_to: party(values.billName, values.billEmail, values.billDetails),
    payer_policy: policy(values),
    ...(values.customerId ? { customer_id: values.customerId } : {}),
    ...(values.issuerId ? { issuer_id: values.issuerId } : {}),
    ...(heading ? { heading: heading.value } : {}),
    ...(reference ? { reference: reference.value } : {}),
    ...(notes ? { notes: notes.value } : {}),
    ...(attachmentId ? { attachment_id: attachmentId } : {}),
    ...(expiresAt ? { expires_at: expiresAt.value } : expiresIn ? { expires_in: expiresIn } : {}),
  };
}
