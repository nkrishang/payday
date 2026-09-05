"use client";

import type { VerificationAttempt } from "@payday/sdk";
import { Button } from "@/components/ui/button";
import { Problem } from "@/components/ui/field";
import { formatDate } from "./labels";
import { useResource } from "./session";

/**
 * What the merchant may know about a payer's verification beyond the verdict
 * beside it: every attempt made against the deposit request, with its status and
 * when it happened. The payer's session and the code they typed are never
 * here, because the API never sends them.
 *
 * Renders nothing — rule included — when no attempt has been made, which is
 * the ordinary case for a request whose payer has not opened the link yet.
 */

/** The rule that separates this from the verdict above it. */
const RULE = "mt-4 border-t border-line pt-4";
export function VerificationActivity({ paymentId }: { paymentId: string }) {
  const detail = useResource(paymentId, (client) => client.depositRequests.verification(paymentId));

  if (detail.error) {
    return (
      <div className={`${RULE} grid gap-3`}>
        <Problem>{detail.error}</Problem>
        <div>
          <Button variant="secondary" size="sm" onClick={detail.reload}>
            Try again
          </Button>
        </div>
      </div>
    );
  }
  if (!detail.data) {
    return (
      <p role="status" className={`${RULE} text-[13px] text-faint`}>
        Loading verification…
      </p>
    );
  }
  const data = detail.data;
  if (data.payer_policy_mode === "permissionless" || data.attempts.length === 0) return null;

  return (
    <div className={`${RULE} grid gap-4`} aria-label="Verification activity">
      <ol className="grid gap-2" aria-label="Verification attempts">
        {data.attempts.map((attempt) => (
          <li key={attempt.id}>
            <Attempt attempt={attempt} />
          </li>
        ))}
      </ol>
    </div>
  );
}

const STATUS_LABEL: Record<VerificationAttempt["status"], string> = {
  pending: "Code sent",
  approved: "Approved",
  abandoned: "Abandoned",
};

/** An attempt in the merchant's words: what happened, by which method. */
const KIND_LABEL: Record<VerificationAttempt["kind"], string> = {
  email: "Email verification",
  merchant_session: "Opened by your app",
};

function Attempt({ attempt }: { attempt: VerificationAttempt }) {
  // A merchant-session attempt is the exchange itself: there is no code to
  // send, so "Approved" would only restate the kind.
  const status =
    attempt.kind === "merchant_session" ? "Session opened" : STATUS_LABEL[attempt.status];
  return (
    <div className="rounded-[10px] border border-line px-3.5 py-3 text-[13px]">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <span className="font-medium">{KIND_LABEL[attempt.kind] ?? attempt.kind}</span>
        <span className="text-muted">{status}</span>
      </div>
      <dl className="mt-2 grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1 text-[12px]">
        <dt className="text-faint">{attempt.verified_at ? "Verified" : "Started"}</dt>
        <dd>{formatDate(attempt.verified_at ?? attempt.created_at)}</dd>
      </dl>
    </div>
  );
}
