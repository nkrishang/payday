import type { CreateDepositRequest, Party } from "@gum/sdk";

/**
 * The add-ons the composer can attach. Merchant auth is not one of them: it
 * needs the merchant's application to sign the payer in and hand over the
 * client secret, which a request composed by hand has no way to do. It is
 * attached through the API only.
 */
export interface ComposerVerification {
  verifyEmail: boolean;
  expectedEmail: string;
  walletAttestation: boolean;
}

/**
 * The `POST /v1/deposit-requests` body, built from what the composer collected.
 *
 * The composer asks its questions over four steps and keeps a draft of
 * strings; this turns that draft into the request the SDK sends. Only the
 * shape is decided here — limits, add-on rules, and the expiry window are the
 * API's to enforce, and its message is shown verbatim when it refuses.
 */

export interface DepositRequestValues {
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
  /** `USDC` or `USDT`. */
  currency: string;
  /** The pinned network's decimal chain id; empty leaves the choice to the payer (USDC only). */
  chainId: string;
  /** Hours; empty means the API's default deadline. */
  expiresInHours: string;
  /** An absolute RFC 3339 moment, which wins over `expiresInHours` when set. */
  expiresAt: string;
  heading: string;
  reference: string;
  notes: string;
  verification: ComposerVerification;
}

export const EMPTY_VALUES: DepositRequestValues = {
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
  currency: "USDC",
  chainId: "",
  expiresInHours: "",
  expiresAt: "",
  heading: "",
  reference: "",
  notes: "",
  verification: { verifyEmail: false, expectedEmail: "", walletAttestation: false },
};

function party(name: string, email: string, details: string): Party {
  return {
    name: name.trim(),
    ...(email.trim() ? { email: email.trim() } : {}),
    ...(details.trim() ? { details: details.trim() } : {}),
  };
}

/**
 * The `verification` add-ons, in the API's own shape. An empty set is left
 * out entirely: omitted or `{}`, the request is fully permissionless. The
 * expected email is only sent when the email add-on is on, and wallet
 * attestation only when it is asked for.
 */
function verification(values: ComposerVerification): CreateDepositRequest["verification"] {
  const expected_email = values.expectedEmail.trim();
  const email = values.verifyEmail ? { expected_email } : undefined;
  const wallet_attestation = values.walletAttestation || undefined;
  if (!email && !wallet_attestation) return undefined;
  return { ...(email ? { email } : {}), ...(wallet_attestation ? { wallet_attestation } : {}) };
}

export function buildCreateDepositRequest(
  values: DepositRequestValues,
  attachmentId: string | null,
): CreateDepositRequest {
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
  const addOns = verification(values.verification);

  return {
    amount: values.amount.trim(),
    currency: values.currency,
    payout_address: values.payoutAddress.trim(),
    issuer: party(values.issuerName, values.issuerEmail, values.issuerDetails),
    payer: party(values.billName, values.billEmail, values.billDetails),
    ...(addOns ? { verification: addOns } : {}),
    ...(values.chainId.trim() ? { chain_id: values.chainId.trim() } : {}),
    ...(values.customerId ? { customer_id: values.customerId } : {}),
    ...(values.issuerId ? { issuer_id: values.issuerId } : {}),
    ...(heading ? { heading: heading.value } : {}),
    ...(reference ? { reference: reference.value } : {}),
    ...(notes ? { notes: notes.value } : {}),
    ...(attachmentId ? { attachment_id: attachmentId } : {}),
    ...(expiresAt ? { expires_at: expiresAt.value } : expiresIn ? { expires_in: expiresIn } : {}),
  };
}
