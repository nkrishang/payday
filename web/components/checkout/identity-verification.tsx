"use client";

import { PaydayError, type PayerIdentityStatus, type PayerPolicyMode } from "@payday/sdk";
import { ExternalLink, Loader2 } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Problem } from "@/components/ui/field";
import { config } from "@/lib/config";
import { payerClient } from "@/lib/payday";
import { backoffMs, DEFAULT_POLL_MS, jitter } from "@/lib/poll";

/**
 * The hosted identity step for the two identity modes. The payer consents
 * here, is sent to the verification partner's hosted session, and comes
 * back to this page; the session token was in this tab's storage all along,
 * so the page resumes without anything in the URL. Whatever the partner
 * appended to the return URL is ignored: the only truth is what the gateway
 * says on `/verify`, which this component polls while an attempt is open.
 */

export const PAYDAY_PRIVACY_URL = "https://payday.sh/privacy";
export const DIDIT_PRIVACY_URL = "https://didit.me/privacy-policy";
const REVIEW_POLL_MS = 30_000;

export function IdentityVerification({
  paymentId,
  mode,
  payerSession,
  onSession,
  onVerified,
  navigate = (url) => window.location.assign(url),
}: {
  paymentId: string;
  mode: PayerPolicyMode;
  payerSession: string;
  onSession: (token: string | null) => void;
  onVerified: () => void;
  /** Sends the payer to the hosted session; injectable for tests. */
  navigate?: (url: string) => void;
}) {
  const [identity, setIdentity] = useState<PayerIdentityStatus | null | undefined>(undefined);
  const [startAvailable, setStartAvailable] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const matched = mode === "verified_identity";
  const verified = useRef(onVerified);
  useEffect(() => {
    verified.current = onVerified;
  });

  // The gateway's word on where this session stands, re-read while the
  // partner may still be deciding.
  useEffect(() => {
    let disposed = false;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let failures = 0;
    const controller = new AbortController();

    const read = async () => {
      try {
        const status = await payerClient.verification.status(paymentId, {
          payerSession,
          signal: controller.signal,
        });
        if (disposed) return;
        failures = 0;
        setIdentity(status.identity);
        setStartAvailable(status.identity_start_available);
        if (status.requirements.complete) {
          verified.current();
          return;
        }
        const open =
          status.identity?.status === "pending" || status.identity?.status === "in_review";
        if (open && !document.hidden) {
          const delay = status.identity?.status === "in_review" ? REVIEW_POLL_MS : DEFAULT_POLL_MS;
          timer = setTimeout(() => void read(), jitter(delay, Math.random()));
        }
      } catch (cause) {
        if (disposed || controller.signal.aborted) return;
        if (cause instanceof PaydayError && cause.code === "payer_session_invalid") {
          onSession(null);
          return;
        }
        // Keep whatever was known and try again; a failed read is not a verdict.
        failures += 1;
        if (!document.hidden) {
          timer = setTimeout(() => void read(), jitter(backoffMs(failures), Math.random()));
        }
      }
    };
    void read();

    return () => {
      disposed = true;
      controller.abort();
      if (timer !== null) clearTimeout(timer);
    };
  }, [paymentId, payerSession, onSession]);

  const start = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const { outcome } = await payerClient.verification.startIdentity(paymentId, payerSession);
      if (outcome.type === "reused") {
        onVerified();
        return;
      }
      navigate(outcome.url);
    } catch (cause) {
      if (cause instanceof PaydayError && cause.code === "payer_session_invalid") {
        onSession(null);
      } else if (cause instanceof PaydayError && cause.code === "review_required") {
        setIdentity((current) =>
          current
            ? { ...current, status: "review_required", retry_available: false }
            : { status: "review_required", attempt_number: 0, retry_available: false },
        );
        setStartAvailable(false);
      } else if (cause instanceof PaydayError && cause.code === "identity_in_review") {
        setIdentity((current) =>
          current
            ? { ...current, status: "in_review", retry_available: false }
            : { status: "in_review", attempt_number: 0, retry_available: false },
        );
        setStartAvailable(false);
      } else if (cause instanceof PaydayError && cause.code === "verification_unavailable") {
        setError("Identity verification is not available right now. Try again later.");
      } else if (cause instanceof PaydayError && cause.code === "identity_provider_unavailable") {
        setError("The verification service did not respond. Try again shortly.");
      } else {
        setError("The identity check could not be started. Try again in a moment.");
      }
    } finally {
      setBusy(false);
    }
  }, [paymentId, payerSession, navigate, onSession, onVerified]);

  if (identity === undefined) {
    return (
      <p role="status" className="text-[13px] text-faint">
        Checking your verification…
      </p>
    );
  }

  const status = identity?.status ?? null;

  if (status === "pending") {
    return (
      <div className="grid gap-3">
        <p role="status" className="flex items-center gap-2 text-[13px] text-muted">
          <Loader2 className="size-4 animate-spin" />
          Waiting for the result of your identity check. If you completed it, this page updates on
          its own; if you left before finishing, you can start again.
        </p>
        {startAvailable ? (
          <div>
            <Button type="button" variant="secondary" size="sm" onClick={start} disabled={busy}>
              Start the identity check again
            </Button>
          </div>
        ) : null}
        <Problem>{error}</Problem>
      </div>
    );
  }

  if (status === "in_review") {
    return (
      <p role="status" className="text-[13px] leading-relaxed text-muted">
        Your identity check is being reviewed by the verification partner. This page updates on its
        own once there is a result; you do not need to do anything now.
      </p>
    );
  }

  if (status === "approved") {
    return (
      <p role="status" className="text-[13px] text-muted">
        Your identity check was approved. Opening the invoice…
      </p>
    );
  }

  if (status === "declined" || status === "review_required") {
    const retry = status === "declined" && startAvailable;
    return (
      <div className="grid gap-3">
        <p className="text-[13px] leading-relaxed text-muted">
          {status === "declined"
            ? matched
              ? "The identity check did not confirm that the document matches the person this invoice names."
              : "The identity check could not confirm the document and liveness checks."
            : "Automated identity verification has stopped for this invoice. A person will look at it; nothing more can be submitted from this page."}
        </p>
        {retry ? (
          <>
            <Consent matched={matched} />
            <div>
              <Button type="button" onClick={start} disabled={busy}>
                {busy ? <Loader2 className="size-4 animate-spin" /> : null}
                Try the identity check again
              </Button>
            </div>
          </>
        ) : null}
        <AppealContact />
        <Problem>{error}</Problem>
      </div>
    );
  }

  // Nothing started, or an earlier session lapsed: consent, then go.
  return (
    <div className="grid gap-3">
      {status === "expired" || status === "abandoned" ? (
        <p className="text-[13px] leading-relaxed text-muted">
          Your earlier identity check was not completed in time. Start it again when you are ready.
        </p>
      ) : null}
      <Consent matched={matched} />
      <div>
        <Button type="button" onClick={start} disabled={busy || !startAvailable}>
          {busy ? <Loader2 className="size-4 animate-spin" /> : null}
          Continue to identity verification
        </Button>
      </div>
      {!startAvailable ? (
        <p className="text-[12px] text-faint">
          Identity verification cannot be started for this invoice right now.
        </p>
      ) : null}
      <Problem>{error}</Problem>
    </div>
  );
}

/**
 * What the payer agrees to before being sent away. Matched and unattributed
 * checks are described differently because they prove different things.
 */
function Consent({ matched }: { matched: boolean }) {
  return (
    <div className="grid gap-2 rounded-[10px] border border-line px-3.5 py-3 text-[13px] leading-relaxed text-muted">
      <p>
        {matched
          ? "You will be asked to photograph an identity document and take a short selfie video. Payday's verification partner, Didit, checks the document, confirms you are a live person, and confirms that the name on the document matches the person this invoice names."
          : "You will be asked to photograph an identity document and take a short selfie video. Payday's verification partner, Didit, checks the document and confirms you are a live person. It does not tell the merchant who you are: Payday records only that the checks passed, not your identity."}
      </p>
      <p>
        Payday keeps the result and its reference only — never your document, photos, name, or date
        of birth. Didit processes them under its own notice.
      </p>
      <p className="flex flex-wrap gap-x-4 gap-y-1">
        <a
          href={PAYDAY_PRIVACY_URL}
          target="_blank"
          rel="noreferrer noopener"
          className="inline-flex items-center gap-1 underline underline-offset-2"
        >
          Payday privacy notice <ExternalLink className="size-3" />
        </a>
        <a
          href={DIDIT_PRIVACY_URL}
          target="_blank"
          rel="noreferrer noopener"
          className="inline-flex items-center gap-1 underline underline-offset-2"
        >
          Didit privacy notice <ExternalLink className="size-3" />
        </a>
      </p>
      <p className="text-[12px] text-faint">
        By continuing you agree to this check. It verifies who may view and pay this invoice; it
        does not prove ownership of the wallet that sends the funds.
      </p>
    </div>
  );
}

/** Where a declined payer turns: a person, not a form (product plan §6.3). */
function AppealContact() {
  return (
    <p className="text-[12px] leading-relaxed text-faint">
      If you believe this is wrong, email{" "}
      <a href={`mailto:${config.payerAppealEmail}`} className="underline underline-offset-2">
        {config.payerAppealEmail}
      </a>{" "}
      with this payment link. A person reviews appeals, usually within one business day, and you
      will hear back by email.
    </p>
  );
}
