"use client";

import { PaydayError } from "@payday/sdk";
import { Loader2 } from "lucide-react";
import { useEffect, useRef } from "react";
import { takeClientSecret } from "@/lib/client-secret";
import { payerClient } from "@/lib/payday";

/**
 * Where a merchant-session checkout stands with its client secret. The
 * checkout derives its locked phase from this, the same way it derives the
 * email phases from "a code was sent".
 */
export type ClientSecretStatus =
  /** No secret came with this page load, or none was needed yet. */
  | "none"
  /** A secret was in the fragment and is being exchanged. */
  | "exchanging"
  /** The exchange said the secret was already spent. */
  | "used"
  /** The exchange refused the secret: unknown, expired, or for another deposit request. */
  | "invalid"
  /** The deposit request is closed; nothing can open it now. */
  | "closed"
  /** The API could not be reached; the secret may still be good. */
  | "unavailable";

/**
 * Exchanges the client secret a merchant put in the URL fragment for this
 * tab's payer session. Runs once per page load, before anything renders that
 * could depend on it: the fragment is read and removed synchronously, the
 * exchange follows, and the session is handed up exactly as the email flow
 * hands its own up. Renders nothing itself.
 */
export function ClientSecretExchange({
  paymentId,
  payerSession,
  onStatus,
  onSession,
}: {
  paymentId: string;
  payerSession: string | null;
  onStatus: (status: ClientSecretStatus) => void;
  onSession: (token: string) => void;
}) {
  // The exchange, started at most once for this mount. React runs effects
  // twice in development (mount, simulated unmount, mount again) while
  // keeping refs; a secret can be spent only once, so the request lives here
  // and each effect run merely subscribes to its outcome. Nothing aborts it:
  // a request that reached the API has spent the secret whether or not the
  // response is read, so the answer must be read.
  const outcome = useRef<Promise<Outcome> | null>(null);

  useEffect(() => {
    if (outcome.current === null) {
      const secret = takeClientSecret();
      // A tab that already holds a session (reload with the fragment still in
      // history) does not need to spend the secret again.
      if (secret === null || payerSession !== null) {
        // Nothing to wait for: say so now, so the bare link never shows a
        // spinner for even a frame.
        onStatus("none");
        outcome.current = Promise.resolve<Outcome>({ status: "none" });
      } else {
        onStatus("exchanging");
        outcome.current = payerClient.verification.exchangeClientSecret(paymentId, secret).then(
          (opened): Outcome => ({ status: "none", session: opened.payer_session }),
          (cause: unknown): Outcome => ({ status: describe(cause) }),
        );
      }
    }
    let current = true;
    void outcome.current.then((result) => {
      if (!current) return;
      if (result.session !== undefined) onSession(result.session);
      onStatus(result.status);
    });
    return () => {
      current = false;
    };
    // The exchange is tied to the page load, not to later prop changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return null;
}

interface Outcome {
  status: ClientSecretStatus;
  /** The payer session the exchange minted, on success. */
  session?: string;
}

function describe(cause: unknown): ClientSecretStatus {
  if (!(cause instanceof PaydayError)) return "unavailable";
  switch (cause.code) {
    case "client_secret_used":
      return "used";
    case "client_secret_invalid":
    case "verification_method_not_applicable":
    case "verification_not_required":
      return "invalid";
    case "deposit_request_not_payable":
      return "closed";
    default:
      return cause.status >= 500 ? "unavailable" : "invalid";
  }
}

/**
 * What the payer sees while a merchant-session deposit request is locked. There is
 * nothing to type and nothing to click: the app that signed them in is the
 * only way in, so the copy says which way to turn.
 */
export function MerchantSessionGate({
  issuerName,
  status,
}: {
  issuerName: string;
  status: ClientSecretStatus;
}) {
  if (status === "exchanging") {
    return (
      <p role="status" className="flex items-center gap-2 text-[13px] text-muted">
        <Loader2 className="size-4 animate-spin" aria-hidden />
        Opening your deposit request…
      </p>
    );
  }
  return <p className="text-[13px] leading-relaxed text-muted">{explain(issuerName, status)}</p>;
}

function explain(issuerName: string, status: ClientSecretStatus): string {
  switch (status) {
    case "used":
      return `This link was already opened. If that was you, use the tab it opened in; otherwise go back to ${issuerName} and open the deposit request again.`;
    case "invalid":
      return `This link has expired or is not valid. Go back to ${issuerName} and open the deposit request again.`;
    case "closed":
      return "This deposit request is closed and can no longer be opened.";
    case "unavailable":
      return "Payday could not be reached. Reload to try again, or go back to the app and open the deposit request again.";
    case "none":
    case "exchanging":
      return `This deposit request opens from ${issuerName}. Sign in there and open it again; this page cannot show it on its own.`;
  }
}
