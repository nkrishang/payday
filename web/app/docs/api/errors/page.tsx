import type { Metadata } from "next";
import { Fragment } from "react";
import { CodeBlock } from "@/components/docs/code";
import { DocsPage, H2, Table } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Errors",
  description: "Every stable error code, its status, and its condition.",
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
          "Malformed JSON, missing or unknown field, wrong type, missing JSON content type, or control character. Message names the field.",
      },
      { code: "invalid_amount", status: "400", meaning: "Syntax, precision, or sign." },
      { code: "missing_idempotency_key", status: "400", meaning: "Header absent on create." },
      {
        code: "idempotency_conflict",
        status: "409",
        meaning: "Key reused with a different immutable field, including attachment hash.",
      },
      { code: "payload_too_large", status: "413", meaning: "Body exceeds the route limit." },
      { code: "rate_limited", status: "429", meaning: "Retry-After set." },
      { code: "not_found, method_not_allowed", status: "404 / 405", meaning: "Route or method." },
      { code: "internal_error", status: "500", meaning: "" },
      { code: "database_unavailable", status: "503", meaning: "" },
    ],
  },
  {
    title: "Authentication and account",
    id: "authentication",
    rows: [
      { code: "unauthorized", status: "401", meaning: "Missing or invalid API key or session." },
      {
        code: "identity_unauthorized",
        status: "401",
        meaning: "Session-only route called with an API key or invalid session.",
      },
      {
        code: "identity_unavailable",
        status: "503",
        meaning: "Identity provider keys unavailable; sessions cannot be verified.",
      },
      { code: "account_disabled", status: "403", meaning: "" },
      {
        code: "account_contact_required",
        status: "409",
        meaning: "Account has no verified email.",
      },
      {
        code: "api_key_generation_conflict",
        status: "409",
        meaning: "expected_generation mismatch.",
      },
      {
        code: "authentication_event_already_used",
        status: "409",
        meaning: "Sign-in event already consumed.",
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
        meaning: "Missing, malformed, or foreign id.",
      },
      { code: "customer_not_found", status: "404", meaning: "" },
      { code: "issuer_not_found", status: "404", meaning: "" },
      { code: "payout_address_not_found", status: "404", meaning: "" },
      {
        code: "webhook_not_found",
        status: "404",
        meaning: "Also: test event requested on a disabled endpoint.",
      },
      { code: "attachment_not_found", status: "404", meaning: "Also: request has no attachment." },
      { code: "issuer_name_taken", status: "409", meaning: "" },
      { code: "issuer_in_use", status: "409", meaning: "Requests issued under the identity." },
      { code: "issuer_email_already_verified", status: "409", meaning: "" },
      {
        code: "unsupported_chain, unsupported_token",
        status: "422",
        meaning: "Override not served by the environment.",
      },
      {
        code: "webhooks_unavailable",
        status: "503",
        meaning: "No webhook encryption key configured.",
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
        meaning: "Scan not reported. Retry finalize with backoff.",
      },
      {
        code: "attachment_rejected",
        status: "422",
        meaning: "Not a clean PDF within limits. Object deleted.",
      },
      {
        code: "attachment_not_ready",
        status: "409",
        meaning: "Not finalized, rejected, expired unused, or no object uploaded.",
      },
      {
        code: "attachment_already_attached",
        status: "409",
        meaning: "Belongs to another request.",
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
        meaning: "Proof requested before settlement.",
      },
      {
        code: "deposit_sender_mismatch",
        status: "409",
        meaning: "Credited transfer from a wallet other than payer_wallet. No proof issued.",
      },
      {
        code: "deposit_request_not_payable",
        status: "410",
        meaning: "Expired, settled, or closed.",
      },
      {
        code: "deposit_request_not_blocked",
        status: "409",
        meaning: "Operator release on a request not in needs_attention.",
      },
    ],
  },
  {
    title: "Verification and wallet",
    id: "verification",
    rows: [
      { code: "invalid_deposit_link", status: "401", meaning: "Unknown id on a payer route." },
      {
        code: "verification_required",
        status: "401",
        meaning: "Gated content requested without an unlocked session.",
      },
      {
        code: "payer_session_invalid",
        status: "401",
        meaning: "Header missing, unknown, expired, or for another request.",
      },
      { code: "otp_invalid", status: "401", meaning: "" },
      {
        code: "otp_resend_cooldown",
        status: "429",
        meaning: "One code per minute. Retry-After set.",
      },
      { code: "verification_not_started", status: "409", meaning: "No outstanding code." },
      { code: "verification_not_required", status: "409", meaning: "Mode is permissionless." },
      {
        code: "verification_method_not_applicable",
        status: "409",
        meaning: "Email routes under merchant_session; client secret under verified_email.",
      },
      {
        code: "verification_persistence_unavailable",
        status: "503",
        meaning: "Code accepted, not recorded. Retry with continuation.",
      },
      {
        code: "verification_unavailable",
        status: "503",
        meaning: "No identity provider configured.",
      },
      { code: "identity_provider_unavailable", status: "502", meaning: "" },
      {
        code: "client_secret_invalid",
        status: "401",
        meaning: "Unknown, malformed, expired, or foreign.",
      },
      { code: "client_secret_used", status: "409", meaning: "Already exchanged." },
      { code: "wallet_required", status: "409", meaning: "QR requested before a wallet is bound." },
      {
        code: "wallet_challenge_required",
        status: "409",
        meaning: "No unexpired challenge on the session.",
      },
      { code: "wallet_already_bound", status: "409", meaning: "Message names the bound wallet." },
      {
        code: "wallet_signature_invalid",
        status: "401",
        meaning: "Signature does not recover to wallet.",
      },
    ],
  },
];

export default function ErrorsPage() {
  return (
    <DocsPage
      eyebrow="API reference"
      title="Errors"
      lead="Stable codes. Branch on error.code; log request_id."
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
                <th>Condition</th>
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
