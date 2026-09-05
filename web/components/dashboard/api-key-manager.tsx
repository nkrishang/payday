"use client";

import type { AccountMetadata } from "@payday/sdk";
import { PaydayError } from "@payday/sdk";
import { Check, Loader2 } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { CopyButton } from "@/components/ui/copy-button";
import { Problem } from "@/components/ui/field";
import { StatusDot } from "@/components/ui/status-dot";
import { describeError } from "@/lib/attachment-upload";
import { cn } from "@/lib/cn";
import { formatDate } from "./labels";
import { useMerchant } from "./session";

/**
 * The API key a merchant's own server calls the API with.
 *
 * The dashboard session is the credential here: generating, rolling, or
 * revoking asks for a plain confirmation, not a second sign-in. What the API
 * still refuses is the reverse — a key can read this account but can never
 * mint or revoke a key — so the only way to this control is to have signed
 * in.
 */
export function ApiKeySection({
  account,
  onChanged,
}: {
  account: AccountMetadata;
  /** Reloads the account after any change lands. */
  onChanged: () => void;
}) {
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
        <ApiKeyCard metadata={account} onChanged={onChanged} />
      </div>
    </section>
  );
}

type Action = "issue" | "revoke";

type Stage =
  | { kind: "idle" }
  | { kind: "confirm"; action: Action }
  | { kind: "revealed"; apiKey: string; replacedPreviousKey: boolean }
  | { kind: "revoked" };

function ApiKeyCard({
  metadata,
  onChanged,
}: {
  metadata: AccountMetadata;
  onChanged: () => void;
}) {
  const { client, signOut } = useMerchant();
  const [stage, setStage] = useState<Stage>({ kind: "idle" });
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [now] = useState(() => Date.now());

  const hasKey = metadata.key_hint !== null;
  const graceActive =
    metadata.previous_key_expires_at !== null &&
    new Date(metadata.previous_key_expires_at).getTime() > now;

  const begin = (action: Action) => {
    setFailure(null);
    setStage({ kind: "confirm", action });
  };

  const cancel = () => {
    setFailure(null);
    setStage({ kind: "idle" });
  };

  const done = () => {
    setStage({ kind: "idle" });
    onChanged();
  };

  const failed = (cause: unknown) => {
    if (cause instanceof PaydayError && cause.status === 401) {
      signOut();
      return;
    }
    if (cause instanceof PaydayError && cause.code === "api_key_generation_conflict") {
      onChanged();
      setStage({ kind: "idle" });
      setBusy(false);
      setFailure(
        "This key changed elsewhere just now — reloaded the latest state below. Try again if you still want to.",
      );
      return;
    }
    setFailure(describeError(cause));
    setBusy(false);
  };

  const confirm = async () => {
    if (stage.kind !== "confirm") return;
    const action = stage.action;
    setBusy(true);
    setFailure(null);
    try {
      if (action === "issue") {
        const issued = await client.account.issueApiKey(metadata.generation);
        setStage({
          kind: "revealed",
          apiKey: issued.api_key,
          replacedPreviousKey: issued.replaced_previous_key,
        });
      } else {
        await client.account.revokeApiKey(metadata.generation);
        setStage({ kind: "revoked" });
      }
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

      {stage.kind === "confirm" ? (
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
          ) : (
            <p className="mb-3 text-[12.5px] text-muted">
              Generates a key for your own server. It is shown once, here, and never again.
            </p>
          )}
          <div className="flex flex-wrap items-center gap-3">
            <Button type="button" onClick={confirm} disabled={busy}>
              {busy ? <Loader2 className="size-4 animate-spin" /> : null}
              {stage.action === "revoke"
                ? "Confirm & revoke"
                : hasKey
                  ? "Confirm & roll key"
                  : "Confirm & generate"}
            </Button>
            <Button type="button" variant="ghost" onClick={cancel} disabled={busy}>
              Cancel
            </Button>
          </div>
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
