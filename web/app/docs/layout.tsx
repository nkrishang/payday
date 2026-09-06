import type { Metadata } from "next";
import type { ReactNode } from "react";
import { DocsShell } from "@/components/docs/shell";

/**
 * Documentation for Payday's customers: what it does, how to use it, and how
 * it works. Every page under here is static, prerendered at build time, and
 * reads nothing from the API — the shell is the landing page's, and the
 * content is written in `web/app/docs/**`.
 */
export const metadata: Metadata = {
  title: { default: "Docs", template: "%s · Payday Docs" },
  description:
    "How to accept USDC deposits with Payday: concepts, the dashboard, the hosted checkout, webhooks, the TypeScript SDK, and the full HTTP API reference.",
};

export default function DocsLayout({ children }: { children: ReactNode }) {
  return <DocsShell>{children}</DocsShell>;
}
