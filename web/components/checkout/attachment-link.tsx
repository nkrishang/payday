"use client";

import { type AttachmentDescriptor, PaydayError } from "@payday/sdk";
import { FileText, Loader2 } from "lucide-react";
import { useState } from "react";
import { formatBytes } from "@/lib/format";
import { payerClient } from "@/lib/payday";

type LinkState =
  | { status: "idle" }
  | { status: "fetching" }
  /** The browser blocked the new tab; offer the URL as a plain link instead. */
  | { status: "blocked"; url: string }
  | { status: "failed"; message: string };

/**
 * The attached PDF. The signed download URL is short-lived and is fetched only
 * when the payer asks for it, straight from the browser: it never appears in
 * the server-rendered page, and a locked invoice's request is refused by the
 * gateway with `verification_required` before any URL is minted.
 */
export function AttachmentLink({
  paymentId,
  attachment,
  payerSession = null,
}: {
  paymentId: string;
  attachment: AttachmentDescriptor;
  /** This tab's session; a gated invoice's descriptor is minted only for it. */
  payerSession?: string | null;
}) {
  const [state, setState] = useState<LinkState>({ status: "idle" });

  const open = async () => {
    // Open synchronously so popup blockers see the user's click. With
    // `noopener`, browsers deliberately return null even when navigation works.
    const tab = window.open("", "_blank");
    if (tab) tab.opener = null;
    setState({ status: "fetching" });
    try {
      const descriptor =
        payerSession === null
          ? await payerClient.payments.attachment(paymentId)
          : await payerClient.payments.attachment(paymentId, payerSession);
      if (!descriptor.download_url) {
        throw new Error("The gateway returned no download link");
      }
      if (tab) {
        tab.location.replace(descriptor.download_url);
        setState({ status: "idle" });
      } else {
        setState({ status: "blocked", url: descriptor.download_url });
      }
    } catch (error) {
      tab?.close();
      setState({
        status: "failed",
        message:
          error instanceof PaydayError && error.code === "verification_required"
            ? "Verify first to open the attachment."
            : "The attachment could not be fetched. Try again in a moment.",
      });
    }
  };

  const fetching = state.status === "fetching";

  return (
    <div>
      <button
        type="button"
        onClick={open}
        disabled={fetching}
        className="flex w-full items-center gap-3 rounded-[10px] border border-line bg-raised px-3.5 py-3 text-left transition-colors hover:border-line-strong disabled:opacity-60"
      >
        {fetching ? (
          <Loader2 className="size-4 shrink-0 animate-spin text-muted" />
        ) : (
          <FileText className="size-4 shrink-0 text-muted" />
        )}
        <span className="min-w-0 flex-1">
          <span className="block truncate text-[13px] font-medium" title={attachment.sha256}>
            {attachment.filename}
          </span>
          <span className="block text-[12px] text-faint">
            PDF · {formatBytes(attachment.byte_length)}
          </span>
        </span>
        <span className="shrink-0 text-[12px] font-medium text-muted">
          {fetching ? "Fetching…" : "Open"}
        </span>
      </button>

      {state.status === "blocked" ? (
        <p className="mt-2 text-[13px] text-muted">
          Your browser blocked the new tab.{" "}
          <a
            href={state.url}
            target="_blank"
            rel="noreferrer noopener"
            className="font-medium text-ink underline underline-offset-2"
          >
            Open {attachment.filename}
          </a>
        </p>
      ) : null}

      {state.status === "failed" ? (
        <p role="alert" className="mt-2 text-[13px] text-danger">
          {state.message}
        </p>
      ) : null}
    </div>
  );
}
