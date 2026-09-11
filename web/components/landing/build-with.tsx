"use client";

import { Check, Copy } from "lucide-react";
import Image from "next/image";
import { useCallback, useEffect, useRef, useState } from "react";

/**
 * One button that puts an integration brief on the clipboard, for whichever
 * agent the reader builds with. The brief points at the docs and names the
 * three things an integration does, so the agent starts from the right
 * shape rather than from the marketing page.
 */
const PROMPT = `Integrate Payday (https://payday.sh) into this app so users can deposit stablecoins.

Read the documentation at https://payday.sh/docs first. Then:
1. From the backend, create a deposit request with payer_policy mode "merchant_session" and the signed-in user's id as payer_reference.
2. Open the hosted checkout for the user with the client_secret from that response.
3. Handle the deposit_request.settled webhook: verify its signature, read the request back from the API, and credit the user's balance.`;

const AGENTS = [
  { name: "Claude", src: "/logos/claude.svg" },
  { name: "Cursor", src: "/logos/cursor.svg" },
  { name: "Codex", src: "/logos/openai.svg" },
] as const;

const COPIED_MS = 1_800;

export function BuildWith() {
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
        await navigator.clipboard.writeText(PROMPT);
      } catch {
        // No clipboard API on plain HTTP, or the document is not focused;
        // the old way still works in both cases.
        const field = document.createElement("textarea");
        field.value = PROMPT;
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
  }, []);

  return (
    <button
      type="button"
      onClick={copy}
      aria-live="polite"
      className="flex h-12 items-center gap-3 rounded-[12px] border border-brand-black/10 bg-white/60 px-4 text-[13.5px] font-medium transition-colors hover:border-brand-black/40"
    >
      <span className="flex items-center" aria-hidden="true">
        {AGENTS.map((agent, index) => (
          <span
            key={agent.name}
            className="flex size-7 items-center justify-center rounded-full border border-brand-black/10 bg-brand-white"
            style={{ marginLeft: index === 0 ? 0 : -8, zIndex: AGENTS.length - index }}
          >
            <Image src={agent.src} width={24} height={24} alt="" className="size-3.5" />
          </span>
        ))}
      </span>
      <span className="whitespace-nowrap">
        {copied ? "Copied prompt" : "Build with your AI agent"}
      </span>
      {copied ? (
        <Check className="ml-auto size-4 shrink-0 text-success" />
      ) : (
        <Copy className="ml-auto size-4 shrink-0 text-brand-subtle" />
      )}
    </button>
  );
}
