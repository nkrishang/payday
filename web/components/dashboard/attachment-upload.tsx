"use client";

import type { AttachmentDescriptor } from "@payday/sdk";
import { FileText, Loader2, ShieldCheck, X } from "lucide-react";
import { useEffect, useRef, useState, type ChangeEvent } from "react";
import { controlStyles, Problem } from "@/components/ui/field";
import { fileProblem, uploadAttachment, type UploadState } from "@/lib/attachment-upload";
import { formatBytes } from "@/lib/format";
import { useMerchant } from "./session";

/**
 * One PDF for the invoice. The file goes straight to object storage with the
 * API's presigned headers and is admitted only after the malware scan; the
 * form learns the finalized descriptor's id and nothing else.
 */
export function AttachmentUpload({
  value,
  onChange,
  onBusyChange,
}: {
  /**
   * What the caller already holds. The composer unmounts this when its step
   * changes, so without it a finished upload would come back as an empty
   * picker while the request still carried the file.
   */
  value?: AttachmentDescriptor | null;
  onChange: (attachment: AttachmentDescriptor | null) => void;
  onBusyChange: (busy: boolean) => void;
}) {
  const { accessToken } = useMerchant();
  const [state, setState] = useState<UploadState>(() =>
    value ? { status: "ready", attachment: value } : { status: "idle" },
  );
  const [problem, setProblem] = useState<string | null>(null);
  const inFlight = useRef<AbortController | null>(null);

  useEffect(() => () => inFlight.current?.abort(), []);
  const pick = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    // Clear the input so choosing the same file again re-runs the upload.
    event.target.value = "";
    if (!file) return;

    const issue = fileProblem(file);
    if (issue) {
      setProblem(issue);
      return;
    }
    setProblem(null);
    onChange(null);
    onBusyChange(true);

    inFlight.current?.abort();
    const controller = new AbortController();
    inFlight.current = controller;
    try {
      const attachment = await uploadAttachment(accessToken, file, setState, controller.signal);
      if (attachment && !controller.signal.aborted) onChange(attachment);
    } finally {
      if (inFlight.current === controller) onBusyChange(false);
    }
  };

  const remove = () => {
    inFlight.current?.abort();
    onBusyChange(false);
    setState({ status: "idle" });
    setProblem(null);
    onChange(null);
  };

  if (state.status === "pending_upload" || state.status === "scanning") {
    return (
      <div
        role="status"
        className="flex items-center gap-3 rounded-[10px] border border-line bg-raised px-3.5 py-3 text-[13px]"
      >
        <Loader2 className="size-4 shrink-0 animate-spin text-muted" />
        <span className="min-w-0 flex-1 truncate">
          {state.status === "pending_upload" ? "Uploading" : "Scanning"}{" "}
          <span className="font-medium">{state.filename}</span>
          {state.status === "scanning" ? " for malware…" : "…"}
        </span>
        <button
          type="button"
          onClick={remove}
          className="rounded-md p-1 text-faint transition-colors hover:text-ink"
          aria-label="Cancel upload"
        >
          <X className="size-4" />
        </button>
      </div>
    );
  }

  if (state.status === "ready") {
    const { attachment } = state;
    return (
      <div className="flex items-center gap-3 rounded-[10px] border border-line bg-raised px-3.5 py-3 text-[13px]">
        <ShieldCheck className="size-4 shrink-0 text-success" />
        <span className="min-w-0 flex-1">
          <span className="block truncate font-medium">{attachment.filename}</span>
          <span className="block text-[12px] text-faint">
            Ready · {formatBytes(attachment.byte_length)}
          </span>
        </span>
        <button
          type="button"
          onClick={remove}
          className="rounded-md p-1 text-faint transition-colors hover:text-ink"
          aria-label="Remove attachment"
        >
          <X className="size-4" />
        </button>
      </div>
    );
  }

  return (
    <div className="grid gap-2">
      <label className="block">
        <span className="flex items-center gap-1.5 text-[12px] font-medium text-muted">
          <FileText className="size-3.5" />
          Attachment (PDF, up to 5 MiB)
        </span>
        <input
          type="file"
          accept="application/pdf,.pdf"
          onChange={pick}
          className={`${controlStyles} mt-1.5 h-10 py-2 file:mr-3 file:rounded-md file:border-0 file:bg-inverse file:px-2.5 file:py-1 file:text-[12px] file:font-medium file:text-inverse-ink`}
        />
      </label>
      <Problem>
        {problem ??
          (state.status === "rejected"
            ? `The file was rejected: ${state.message}`
            : state.status === "failed"
              ? `Upload of ${state.filename} failed: ${state.message}`
              : null)}
      </Problem>
    </div>
  );
}
