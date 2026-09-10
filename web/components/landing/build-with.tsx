"use client";

import Image from "next/image";
import { useState } from "react";
import { cn } from "@/lib/cn";

/**
 * Which coding agent the reader builds with. For now it only remembers the
 * choice; the guides it will one day switch between are not written yet.
 */
const TOOLS = [
  { id: "claude", label: "Claude", src: "/logos/claude.svg" },
  { id: "cursor", label: "Cursor", src: "/logos/cursor.svg" },
  { id: "codex", label: "Codex", src: "/logos/openai.svg" },
] as const;

type Tool = (typeof TOOLS)[number]["id"];

export function BuildWith() {
  const [tool, setTool] = useState<Tool>("claude");

  return (
    <div>
      <p className="text-[11px] font-medium tracking-[0.14em] text-brand-subtle uppercase">
        Build with
      </p>
      <div role="radiogroup" aria-label="Build with" className="mt-2 grid grid-cols-3 gap-2">
        {TOOLS.map((entry) => {
          const checked = entry.id === tool;
          return (
            <button
              key={entry.id}
              type="button"
              role="radio"
              aria-checked={checked}
              onClick={() => setTool(entry.id)}
              className={cn(
                "flex h-11 items-center justify-center gap-2 rounded-[10px] border text-[13px] font-medium transition-colors",
                checked
                  ? "border-brand-black bg-white"
                  : "border-brand-black/10 bg-white/40 text-brand-black/80 hover:border-brand-black/40",
              )}
            >
              <Image src={entry.src} width={24} height={24} alt="" className="size-4" />
              {entry.label}
            </button>
          );
        })}
      </div>
    </div>
  );
}
