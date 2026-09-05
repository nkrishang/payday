"use client";

import type { AccountMetadata } from "@payday/sdk";
import { PaydayError } from "@payday/sdk";
import { Check, Loader2 } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { CopyButton } from "@/components/ui/copy-button";
import { controlStyles, Problem } from "@/components/ui/field";
import { StatusDot } from "@/components/ui/status-dot";
import { describeError } from "@/lib/attachment-upload";
import { cn } from "@/lib/cn";
import {
  confirmEmailOtp,
  EMAIL_OTP_LIFETIME_MS,
  EmailOtpError,
  sessionEmail,
  startEmailOtp,
} from "@/lib/merchant-payday";
import { Labeled } from "./labeled";
import { formatDate } from "./labels";
import { useMerchant, useResource } from "./session";

/**
 * The API key a merchant's own server calls the API with.
 *
 * The dashboard signs in with an emailed code and never simply holds this
 * credential — reading the account here proves nothing about permission to
 * mint a live key for it — so generating, rolling, or revoking steps up with
 * its own fresh code first, the same email-OTP authentication `payday login`
 * and `payday keys` use from the CLI. Only that action needs it; reading the
 * current key's metadata below does not.
 */
export function ApiKeySection() {
  const account = useResource("account", (client) => client.account.get());

  return (
    <section aria-label="API key">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h2 className="font-heading text-[24px] leading-tight font-medium tracking-[-0.04em]">
            API key<span className="text-brand-yellow">.</span>
          </h2>
          <p className="mt-1.5 text-[13px] text-muted">
            The credential your own server calls the API with, separate from the email you sign in
            here with.
          </p>
        </div>
      </div>

      <div className="mt-6">
        {account.data ? (
          <ApiKeyCard metadata={account.data} onChanged={account.reload} />
        ) : account.error ? (
          <div className="grid gap-3">
            <Problem>{account.error}</Problem>
            <div>
              <Button type="button" variant="secondary" size="sm" onClick={account.reload}>
                Try again
              </Button>
            </div>
          </div>
        ) : (
          <div className="h-[68px] animate-pulse rounded-[12px] border border-line bg-surface" />
        )}
      </div>
    </section>
  );
}

type Action = "issue" | "revoke";

type Stage =
  | { kind: "idle" }
  | { kind: "stepup"; action: Action; sent: boolean }
  | { kind: "revealed"; apiKey: string; replacedPreviousKey: boolean }
  | { kind: "revoked" };

function ApiKeyCard({
  metadata,
  onChanged,
}: {
  metadata: AccountMetadata;
  /** Reloads the account after any change lands. */
  onChanged: () => void;
}) {
  const { client, accessToken, signOut } = useMerchant();
  const [stage, setStage] = useState<Stage>({ kind: "idle" });
  const [otp, setOtp] = useState("");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [now] = useState(() => Date.now());

  const email = sessionEmail(accessToken);
  const hasKey = metadata.key_hint !== null;
  const graceActive =
    metadata.previous_key_expires_at !== null &&
    new Date(metadata.previous_key_expires_at).getTime() > now;

  const begin = (action: Action) => {
    setFailure(null);
    setOtp("");
    setStage({ kind: "stepup", action, sent: false });
  };

  const cancel = () => {
    setFailure(null);
    setOtp("");
    setStage({ kind: "idle" });
  };

  const done = () => {
    setStage({ kind: "idle" });
    onChanged();
  };

  const failed = (cause: unknown) => {
    if (cause instanceof PaydayError && cause.status === 401 && cause.code === "unauthorized") {
      signOut();
      return;
    }
    if (cause instanceof PaydayError && cause.code === "api_key_generation_conflict") {
      onChanged();
      setStage({ kind: "idle" });
      setOtp("");
      setBusy(false);
      setFailure(
        "This key changed elsewhere just now — reloaded the latest state below. Try again if you still want to.",
      );
      return;
    }
    setFailure(
      cause instanceof EmailOtpError && cause.code === "invalid_grant"
        ? "That code is not valid. Check the email and try again."
        : describeError(cause),
    );
    setBusy(false);
  };

  const sendCode = async () => {
    if (stage.kind !== "stepup" || !email) return;
    setBusy(true);
    setFailure(null);
    try {
      await startEmailOtp(email);
      setStage({ ...stage, sent: true });
      setBusy(false);
    } catch (cause) {
      failed(cause);
    }
  };

  const confirm = async () => {
    if (stage.kind !== "stepup" || !email) return;
    const action = stage.action;
    setBusy(true);
    setFailure(null);
    try {
      const fresh = await confirmEmailOtp(email, otp.trim());
      if (action === "issue") {
        const issued = await client.account.issueApiKey(fresh.accessToken, metadata.generation);
        setStage({
          kind: "revealed",
          apiKey: issued.api_key,
          replacedPreviousKey: issued.replaced_previous_key,
        });
      } else {
        await client.account.revokeApiKey(fresh.accessToken, metadata.generation);
        setStage({ kind: "revoked" });
      }
      setOtp("");
      setBusy(false);
    } catch (cause) {
      failed(cause);
    }
  };

  return (
    <div className="overflow-hidden rounded-[12px] border border-line bg-surface">
      <div className="flex flex-wrap items-center gap-3 px-4 py-3.5">
        <StatusDot tone={hasKey ? "success" : "neutral"} />
        <span className={cn("truncate text-[14px] font-medium", hasKey && "font-mono")}>
          {hasKey ? metadata.key_hint : metadata.revoked_at ? "Revoked" : "No key yet"}
        </span>
        <span className="min-w-0 flex-1 truncate text-[12px] text-faint">
          {hasKey
            ? `Created ${formatDate(metadata.created_at)}${
                metadata.rotated_at ? ` · Rotated ${formatDate(metadata.rotated_at)}` : ""
              }`
            : metadata.revoked_at
              ? `Revoked ${formatDate(metadata.revoked_at)}`
              : "Generate one to call the API from your own server."}
        </span>
        {stage.kind === "idle" ? (
          <div className="flex shrink-0 items-center gap-2">
            {hasKey ? (
              <>
                <Button type="button" variant="secondary" size="sm" onClick={() => begin("issue")}>
                  Roll key
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="text-danger hover:bg-danger/10"
                  onClick={() => begin("revoke")}
                >
                  Revoke
                </Button>
              </>
            ) : (
              <Button type="button" variant="secondary" size="sm" onClick={() => begin("issue")}>
                Generate key
              </Button>
            )}
          </div>
        ) : null}
      </div>

      {graceActive && stage.kind === "idle" ? (
        <p className="border-t border-line bg-warning/[0.06] px-4 py-2.5 text-[12px] text-warning">
          Your previous key still works until{" "}
          {formatDate(metadata.previous_key_expires_at as string)}, so anything not yet switched
          over keeps running.
        </p>
      ) : null}

      {stage.kind === "stepup" ? (
        <div className="dash-step border-t border-line px-4 pt-4 pb-5">
          {stage.action === "revoke" ? (
            <p className="mb-3 text-[12.5px] text-danger">
              This immediately disables the current key
              {graceActive ? " and the previous one still in its grace window" : ""}. Anything
              using it fails right away, and this can&apos;t be undone.
            </p>
          ) : hasKey ? (
            <p className="mb-3 text-[12.5px] text-muted">
              Issues a new key. The current one keeps working for 24 hours, so nothing breaks
              while you switch over.
            </p>
          ) : null}

          {!email ? (
            <Problem>
              Your session doesn&apos;t carry a mailbox to confirm. Sign out and back in, then try
              again.
            </Problem>
          ) : !stage.sent ? (
            <div className="flex flex-wrap items-end gap-3">
              <Labeled
                label="Confirm it's you"
                hint={`We'll email a one-time code to ${email}, valid for ${Math.round(
                  EMAIL_OTP_LIFETIME_MS / 60_000,
                )} minutes.`}
              >
                <div className={cn(controlStyles, "flex h-11 items-center text-muted")}>
                  {email}
                </div>
              </Labeled>
              <Button type="button" onClick={sendCode} disabled={busy}>
                {busy ? <Loader2 className="size-4 animate-spin" /> : null}
                Send code
              </Button>
              <Button type="button" variant="ghost" onClick={cancel} disabled={busy}>
                Cancel
              </Button>
            </div>
          ) : (
            <div className="flex flex-wrap items-end gap-3">
              <Labeled label="One-time code" hint={`Sent to ${email}.`}>
                <input
                  autoFocus
                  inputMode="numeric"
                  autoComplete="one-time-code"
                  maxLength={6}
                  placeholder="000000"
                  value={otp}
                  onChange={(event) => setOtp(event.target.value.replace(/\D/g, "").slice(0, 6))}
                  className={cn(
                    controlStyles,
                    "h-11 w-[180px] text-center font-mono text-[16px] tracking-[0.35em] [text-indent:0.35em]",
                  )}
                />
              </Labeled>
              <Button type="button" onClick={confirm} disabled={busy || otp.length < 6}>
                {busy ? <Loader2 className="size-4 animate-spin" /> : null}
                {stage.action === "revoke"
                  ? "Confirm & revoke"
                  : hasKey
                    ? "Confirm & roll key"
                    : "Confirm & generate"}
              </Button>
              <Button
                type="button"
                variant="ghost"
                onClick={() => setStage({ ...stage, sent: false })}
                disabled={busy}
              >
                Resend
              </Button>
              <Button type="button" variant="ghost" onClick={cancel} disabled={busy}>
                Cancel
              </Button>
            </div>
          )}
        </div>
      ) : null}

      {stage.kind === "revealed" ? (
        <div className="dash-step border-t border-line bg-success/[0.06] px-4 pt-4 pb-5">
          <p className="flex items-center gap-1.5 text-[13px] font-medium">
            <Check className="size-4 text-success" />
            {stage.replacedPreviousKey ? "Key rolled" : "Key generated"}
          </p>
          <p className="mt-1 text-[12.5px] text-muted">
            Copy it now — this is the only time it&apos;s shown.
          </p>
          <div className="mt-3 flex items-center gap-2 rounded-[10px] border border-line bg-surface py-2 pr-2 pl-3">
            <code className="min-w-0 flex-1 truncate font-mono text-[13px]">{stage.apiKey}</code>
            <CopyButton value={stage.apiKey} label="API key" className="bg-raised" />
          </div>
          {stage.replacedPreviousKey ? (
            <p className="mt-3 text-[12px] text-muted">
              Your previous key keeps working for the next 24 hours, so you can switch over
              without downtime — or revoke it early from here once you have.
            </p>
          ) : null}
          <div className="mt-4 flex justify-end">
            <Button type="button" variant="secondary" size="sm" onClick={done}>
              Done
            </Button>
          </div>
        </div>
      ) : null}

      {stage.kind === "revoked" ? (
        <div className="dash-step border-t border-line px-4 pt-4 pb-5 text-[13px]">
          <p className="font-medium">Key revoked.</p>
          <p className="mt-1 text-[12.5px] text-muted">
            Anything still using it started failing immediately. Generate a new one whenever
            you&apos;re ready.
          </p>
          <div className="mt-3 flex justify-end">
            <Button type="button" variant="secondary" size="sm" onClick={done}>
              Done
            </Button>
          </div>
        </div>
      ) : null}

      {failure ? (
        <div className="border-t border-line px-4 py-3">
          <Problem>{failure}</Problem>
        </div>
      ) : null}
    </div>
  );
}
