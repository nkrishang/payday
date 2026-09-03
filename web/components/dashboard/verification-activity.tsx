"use client";

import {
  PaydayError,
  type VerificationAttempt,
  type VerificationDetail,
  type VerificationFactStatus,
} from "@payday/sdk";
import { Loader2 } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Problem } from "@/components/ui/field";
import { StatusDot } from "@/components/ui/status-dot";
import { describeError } from "@/lib/attachment-upload";
import { formatDate } from "./labels";
import { useMerchant, useResource } from "./session";

/**
 * What the merchant may know about a payer's verification: each fact on its
 * own, every attempt with the provider's reference and risk categories, the
 * reviewer's outcome, and the two things that can happen next. The
 * provider's extracted identity is never here, because the API never sends it.
 */
export function VerificationActivity({ paymentId }: { paymentId: string }) {
  const { client } = useMerchant();
  const [version, setVersion] = useState(0);
  const detail = useResource(`${paymentId}|${version}`, (client) =>
    client.payments.verification(paymentId),
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  if (detail.error) {
    return (
      <div className="grid gap-3">
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
      <p role="status" className="text-[13px] text-faint">
        Loading verification…
      </p>
    );
  }
  const data = detail.data;
  if (data.payer_policy_mode === "permissionless") return null;

  const requestReview = async () => {
    setBusy(true);
    setError(null);
    try {
      await client.payments.requestVerificationReview(paymentId);
      setVersion((current) => current + 1);
    } catch (cause) {
      setError(
        cause instanceof PaydayError && cause.code === "review_not_available"
          ? "No declined identity attempt is waiting for review."
          : describeError(cause),
      );
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="grid gap-4" aria-label="Verification activity">
      <Facts detail={data} />
      {data.attempts.length > 0 ? (
        <ol className="grid gap-2" aria-label="Verification attempts">
          {data.attempts.map((attempt) => (
            <li key={attempt.id}>
              <Attempt attempt={attempt} />
            </li>
          ))}
        </ol>
      ) : (
        <p className="text-[13px] text-muted">The payer has not started verifying yet.</p>
      )}
      {data.retry_available ? (
        <p className="text-[12px] leading-relaxed text-faint">
          The payer may try the identity check once more from the payment page.
        </p>
      ) : null}
      {data.review_available ? (
        <div className="grid gap-2">
          <Button variant="secondary" size="sm" onClick={requestReview} disabled={busy}>
            {busy ? <Loader2 className="size-4 animate-spin" /> : null}
            Request review
          </Button>
          <p className="text-[12px] leading-relaxed text-faint">
            A person looks at the declined attempt, usually within one business day. Automated
            resubmission stops for this payer once a review is requested.
          </p>
        </div>
      ) : null}
      <Problem>{error}</Problem>
    </div>
  );
}

const FACT_LABEL: Record<"email" | "document" | "liveness" | "identity_match", string> = {
  email: "Email ownership",
  document: "Identity document",
  liveness: "Liveness and face match",
  identity_match: "Name matches the invoice",
};

const FACT_STATUS: Record<
  VerificationFactStatus,
  { label: string; tone: "neutral" | "success" | "warning" | "progress" }
> = {
  not_required: { label: "Not required", tone: "neutral" },
  pending: { label: "Pending", tone: "progress" },
  approved: { label: "Approved", tone: "success" },
  declined: { label: "Declined", tone: "warning" },
};

function Facts({ detail }: { detail: VerificationDetail }) {
  const facts = (Object.keys(FACT_LABEL) as Array<keyof typeof FACT_LABEL>).filter(
    (key) => detail.facts[key] !== "not_required",
  );
  return (
    <dl
      className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1.5 text-[13px]"
      aria-label="Verification facts"
    >
      {facts.map((key) => {
        const { label, tone } = FACT_STATUS[detail.facts[key]];
        return (
          <FactRow key={key} label={FACT_LABEL[key]}>
            <StatusDot tone={tone} />
            {label}
          </FactRow>
        );
      })}
    </dl>
  );
}

function FactRow({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <>
      <dt className="text-faint">{label}</dt>
      <dd className="flex items-center gap-2">{children}</dd>
    </>
  );
}

const STATUS_LABEL: Record<VerificationAttempt["status"], string> = {
  pending: "In progress",
  approved: "Approved",
  declined: "Declined",
  in_review: "In review with the provider",
  expired: "Expired",
  abandoned: "Abandoned",
  review_required: "Review required",
};

const PROVIDER_LABEL: Record<VerificationAttempt["provider"], string> = {
  auth0: "Email code",
  didit: "Didit",
  manual: "Manual review",
};

function Attempt({ attempt }: { attempt: VerificationAttempt }) {
  const title =
    attempt.kind === "email"
      ? "Email verification"
      : `Identity check ${attempt.attempt_number > 1 ? `(attempt ${attempt.attempt_number})` : ""}`.trim();
  return (
    <div className="rounded-[10px] border border-line px-3.5 py-3 text-[13px]">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <span className="font-medium">{title}</span>
        <span className="text-muted">{STATUS_LABEL[attempt.status]}</span>
      </div>
      <dl className="mt-2 grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-1 text-[12px]">
        <dt className="text-faint">Provider</dt>
        <dd>
          {PROVIDER_LABEL[attempt.provider]}
          {attempt.provider_reference ? (
            <span className="ml-2 font-mono text-faint break-all" title="Provider reference">
              {attempt.provider_reference}
            </span>
          ) : null}
        </dd>
        {attempt.kind === "identity" && attempt.status !== "pending" ? (
          <>
            <dt className="text-faint">Risk categories</dt>
            <dd>
              {attempt.risk_codes.length > 0 ? (
                <span className="flex flex-wrap gap-1">
                  {attempt.risk_codes.map((code) => (
                    <span
                      key={code}
                      className="rounded-md border border-line px-1.5 py-0.5 font-mono text-[11px]"
                    >
                      {code}
                    </span>
                  ))}
                </span>
              ) : (
                <span className="text-faint">None</span>
              )}
            </dd>
          </>
        ) : null}
        {attempt.country_code ? (
          <>
            <dt className="text-faint">Document country</dt>
            <dd>{attempt.country_code}</dd>
          </>
        ) : null}
        <dt className="text-faint">{attempt.verified_at ? "Verified" : "Started"}</dt>
        <dd>{formatDate(attempt.verified_at ?? attempt.created_at)}</dd>
        {attempt.expires_at ? (
          <>
            <dt className="text-faint">Reusable until</dt>
            <dd>{formatDate(attempt.expires_at)}</dd>
          </>
        ) : null}
        {attempt.review ? (
          <>
            <dt className="text-faint">Review</dt>
            <dd>
              {attempt.review.decision
                ? `${attempt.review.decision === "approved" ? "Approved" : "Declined"} by ${attempt.review.reviewer ?? "a reviewer"} · ${formatDate(attempt.review.decided_at ?? attempt.review.requested_at)}`
                : `Requested ${formatDate(attempt.review.requested_at)}, awaiting a reviewer`}
              {attempt.review.note ? (
                <span className="block whitespace-pre-wrap text-muted">{attempt.review.note}</span>
              ) : null}
            </dd>
          </>
        ) : null}
      </dl>
    </div>
  );
}
