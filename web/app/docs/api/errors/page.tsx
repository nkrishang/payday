import type { Metadata } from "next";
import { Fragment } from "react";
import { CodeBlock } from "@/components/docs/code";
import { DocsPage, H2, Table } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Errors",
  description:
    "Every stable error code the Payday API returns, its usual status, and what to do about it.",
};

const SHAPE = `{
  "error": { "code": "idempotency_conflict", "message": "Idempotency-Key was used with a different document" },
  "request_id": "0198f80c-5555-7dc1-a369-90556a64f700"
}`;

interface Row {
  code: string;
  status: string;
  meaning: string;
}

const GROUPS: Array<{ title: string; id: string; rows: Row[] }> = [
  {
    title: "Requests",
    id: "requests",
    rows: [
      {
        code: "invalid_request",
        status: "400",
        meaning:
          "Malformed JSON, a missing or unknown field, a wrong type, a missing JSON content type, or a control character in text. The message names the field.",
      },
      {
        code: "invalid_amount",
        status: "400",
        meaning: "Bad amount syntax, more than six decimals, or not positive.",
      },
      { code: "missing_idempotency_key", status: "400", meaning: "The create header is absent." },
      {
        code: "idempotency_conflict",
        status: "409",
        meaning:
          "The key was reused with a different immutable field, including the attachment's hash. Fetch the original.",
      },
      { code: "payload_too_large", status: "413", meaning: "The body exceeds the route's limit." },
      {
        code: "rate_limited",
        status: "429",
        meaning: "The per-account allowance is spent; see Retry-After.",
      },
      {
        code: "not_found, method_not_allowed",
        status: "404 / 405",
        meaning: "No such route, or not that method.",
      },
      {
        code: "internal_error",
        status: "500",
        meaning: "Something failed on Payday's side. Quote the request id.",
      },
      {
        code: "database_unavailable",
        status: "503",
        meaning: "Persistent storage is unavailable. Retry.",
      },
    ],
  },
  {
    title: "Authentication and account",
    id: "authentication",
    rows: [
      {
        code: "unauthorized",
        status: "401",
        meaning: "Missing or invalid API key or dashboard session.",
      },
      {
        code: "identity_unauthorized",
        status: "401",
        meaning:
          "An account-key route was called without a dashboard session: an API key, or an invalid session token.",
      },
      {
        code: "identity_unavailable",
        status: "503",
        meaning:
          "The identity provider's keys could not be fetched; sessions cannot be verified right now.",
      },
      {
        code: "account_disabled",
        status: "403",
        meaning: "The account is disabled. Contact support.",
      },
      {
        code: "account_contact_required",
        status: "409",
        meaning: "Sign in to the dashboard again so the account has a verified email.",
      },
      {
        code: "api_key_generation_conflict",
        status: "409",
        meaning: "The key generation moved. Re-read the account.",
      },
      {
        code: "authentication_event_already_used",
        status: "409",
        meaning: "The sign-in event already changed key state.",
      },
    ],
  },
  {
    title: "Resources",
    id: "resources",
    rows: [
      {
        code: "deposit_request_not_found",
        status: "404",
        meaning: "Missing, malformed, or another account's.",
      },
      { code: "customer_not_found", status: "404", meaning: "Same." },
      { code: "issuer_not_found", status: "404", meaning: "Same." },
      { code: "payout_address_not_found", status: "404", meaning: "Same." },
      {
        code: "webhook_not_found",
        status: "404",
        meaning: "Same, and also a disabled endpoint asked to send a test.",
      },
      {
        code: "attachment_not_found",
        status: "404",
        meaning: "Same, or the request has no attachment.",
      },
      {
        code: "issuer_name_taken",
        status: "409",
        meaning: "Another of your identities already uses this name.",
      },
      {
        code: "issuer_in_use",
        status: "409",
        meaning: "Requests were issued under this identity; it cannot be deleted.",
      },
      {
        code: "issuer_email_already_verified",
        status: "409",
        meaning: "The contact address is already proven.",
      },
      {
        code: "unsupported_chain, unsupported_token",
        status: "422",
        meaning: "The environment does not serve the requested chain or token.",
      },
      {
        code: "webhooks_unavailable",
        status: "503",
        meaning: "The environment cannot store webhook secrets.",
      },
    ],
  },
  {
    title: "Attachments",
    id: "attachments",
    rows: [
      {
        code: "attachment_scan_pending",
        status: "409",
        meaning: "The malware scan has not reported. Retry finalize with backoff.",
      },
      {
        code: "attachment_rejected",
        status: "422",
        meaning: "Not a clean PDF within limits. Upload a new file.",
      },
      {
        code: "attachment_not_ready",
        status: "409",
        meaning:
          "The attachment is not finalized, was rejected, or expired unused; also finalize before any bytes arrived.",
      },
      {
        code: "attachment_already_attached",
        status: "409",
        meaning: "The attachment belongs to another request.",
      },
    ],
  },
  {
    title: "Deposits and proofs",
    id: "deposits",
    rows: [
      {
        code: "deposit_request_not_settled",
        status: "409",
        meaning: "The proof was requested before settlement.",
      },
      {
        code: "deposit_sender_mismatch",
        status: "409",
        meaning:
          "A credited transfer came from a wallet other than the attested one; no proof is issued.",
      },
      {
        code: "deposit_request_not_payable",
        status: "410",
        meaning: "The request is no longer payable: past its deadline, settled, or closed.",
      },
      {
        code: "deposit_request_not_blocked",
        status: "409",
        meaning: "An operator release was asked of a request that needs no attention.",
      },
    ],
  },
  {
    title: "Verification and wallet",
    id: "verification",
    rows: [
      {
        code: "invalid_deposit_link",
        status: "401",
        meaning: "The deposit link does not resolve to a request.",
      },
      {
        code: "verification_required",
        status: "401",
        meaning: "Gated content or the QR was requested without an unlocked session.",
      },
      {
        code: "payer_session_invalid",
        status: "401",
        meaning: "The session header is missing, unknown, expired, or for another request.",
      },
      { code: "otp_invalid", status: "401", meaning: "The code was not accepted." },
      {
        code: "otp_resend_cooldown",
        status: "429",
        meaning: "A code was sent within the last minute; see Retry-After.",
      },
      {
        code: "verification_not_started",
        status: "409",
        meaning: "Confirm was called before a code was sent, or after it was spent.",
      },
      {
        code: "verification_not_required",
        status: "409",
        meaning: "Verification or a client secret was asked of a permissionless request.",
      },
      {
        code: "verification_method_not_applicable",
        status: "409",
        meaning:
          "Email codes on a merchant-session request, or a client secret on a verified-email one.",
      },
      {
        code: "verification_persistence_unavailable",
        status: "503",
        meaning:
          "The code was accepted but not recorded; retry confirm with the returned continuation.",
      },
      {
        code: "verification_unavailable",
        status: "503",
        meaning: "The environment has no email verification configured.",
      },
      {
        code: "identity_provider_unavailable",
        status: "502",
        meaning: "The email verification provider did not answer.",
      },
      {
        code: "client_secret_invalid",
        status: "401",
        meaning: "Unknown, malformed, expired, or for another request.",
      },
      {
        code: "client_secret_used",
        status: "409",
        meaning: "Already exchanged; the link was opened once.",
      },
      {
        code: "wallet_required",
        status: "409",
        meaning: "The QR was requested before a wallet was bound.",
      },
      {
        code: "wallet_challenge_required",
        status: "409",
        meaning: "Attest was called without an outstanding challenge.",
      },
      {
        code: "wallet_already_bound",
        status: "409",
        meaning: "The request is bound to another wallet; the message names it.",
      },
      {
        code: "wallet_signature_invalid",
        status: "401",
        meaning: "The signature does not recover to the stated wallet.",
      },
    ],
  },
];

export default function ErrorsPage() {
  return (
    <DocsPage
      eyebrow="API reference"
      title="Errors"
      lead="Every error is JSON with a stable code, a message that names the problem, and the request id. Branch on the code; show the message to a developer; quote the id to support."
    >
      <CodeBlock code={SHAPE} lang="json" />
      {GROUPS.map((group) => (
        <Fragment key={group.id}>
          <H2 id={group.id}>{group.title}</H2>
          <Table>
            <thead>
              <tr>
                <th>Code</th>
                <th>Status</th>
                <th>Meaning</th>
              </tr>
            </thead>
            <tbody>
              {group.rows.map((row) => (
                <tr key={row.code}>
                  <td>
                    <code>{row.code}</code>
                  </td>
                  <td>{row.status}</td>
                  <td>{row.meaning}</td>
                </tr>
              ))}
            </tbody>
          </Table>
        </Fragment>
      ))}
    </DocsPage>
  );
}
