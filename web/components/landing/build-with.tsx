"use client";

import Image from "next/image";
import { CopyPromptButton } from "@/components/ui/copy-prompt-button";

/**
 * The landing page's copy-a-prompt button, for whichever agent the reader
 * builds with. The brief points at the docs and names the three things an
 * integration does, so the agent starts from the right shape rather than
 * from the marketing page.
 */
const PROMPT = `Integrate Payday (https://payday.sh) into this app so users can deposit stablecoins.

Read the documentation at https://payday.sh/docs first. Then:
1. From the backend, create a deposit request with payer_policy mode "merchant_session" and the signed-in user's id as payer_reference.
2. Open the hosted checkout for the user with the client_secret from that response.
3. Handle the deposit_request.settled webhook: verify its signature, read the request back from the API, and credit the user's balance.`;

export const AGENTS = [
  { name: "Claude", src: "/logos/claude.svg" },
  { name: "Cursor", src: "/logos/cursor.svg" },
  { name: "Codex", src: "/logos/openai.svg" },
] as const;

/** The three agents' marks, overlapping, as the leading decoration of a prompt button. */
export function AgentAvatars({ className }: { className?: string }) {
  return (
    <span className="flex items-center" aria-hidden="true">
      {AGENTS.map((agent, index) => (
        <span
          key={agent.name}
          className={
            className ??
            "flex size-7 items-center justify-center rounded-full border border-brand-black/10 bg-brand-white"
          }
          style={{ marginLeft: index === 0 ? 0 : -8, zIndex: AGENTS.length - index }}
        >
          <Image src={agent.src} width={24} height={24} alt="" className="size-3.5" />
        </span>
      ))}
    </span>
  );
}

export function BuildWith() {
  return (
    <CopyPromptButton
      prompt={PROMPT}
      label="Build with your AI agent"
      className="border-brand-black/10 bg-white/60 hover:border-brand-black/40 [&_svg]:text-brand-subtle"
    >
      <AgentAvatars />
    </CopyPromptButton>
  );
}
