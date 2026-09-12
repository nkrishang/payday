"use client";

import { Check, Copy } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useRef, useState } from "react";
import { cn } from "@/lib/cn";

const COPIED_MS = 1_800;

/**
 * One button that puts a brief for an AI agent on the clipboard. The landing
 * page hands an agent the integration in three lines; a docs page hands it a
 * whole specification. Either way the reader's agent starts from the right
 * shape rather than from prose.
 */
export function CopyPromptButton({
  prompt,
  label,
  copiedLabel = "Copied prompt",
  className,
  children,
}: {
  prompt: string;
  label: string;
  copiedLabel?: string;
  className?: string;
  /** Leading decoration, such as the agents' avatars. */
  children?: ReactNode;
}) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );

  const copy = useCallback(async () => {
    try {
      try {
        await navigator.clipboard.writeText(prompt);
      } catch {
        // No clipboard API on plain HTTP, or the document is not focused;
        // the old way still works in both cases.
        const field = document.createElement("textarea");
        field.value = prompt;
        field.setAttribute("readonly", "");
        field.style.position = "fixed";
        field.style.opacity = "0";
        document.body.appendChild(field);
        field.select();
        document.execCommand("copy");
        document.body.removeChild(field);
      }
      setCopied(true);
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => setCopied(false), COPIED_MS);
    } catch {
      // Clipboard access can be blocked outright; nothing to show for it.
    }
  }, [prompt]);

  return (
    <button
      type="button"
      onClick={copy}
      aria-live="polite"
      className={cn(
        "flex h-12 items-center gap-3 rounded-[12px] border px-4 text-[13.5px] font-medium transition-colors",
        className,
      )}
    >
      {children}
      <span className="whitespace-nowrap">{copied ? copiedLabel : label}</span>
      {copied ? (
        <Check className="ml-auto size-4 shrink-0 text-success" />
      ) : (
        <Copy className="ml-auto size-4 shrink-0 opacity-60" />
      )}
    </button>
  );
}
