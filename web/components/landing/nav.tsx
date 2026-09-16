"use client";

import { ChevronDown } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import { cn } from "@/lib/cn";
import { RESOURCES } from "./links";

/** The header's one dropdown: a button, a list, and the usual ways to close it. */
export function ResourcesMenu() {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const menuId = useId();

  useEffect(() => {
    if (!open) return;
    const close = (event: MouseEvent | KeyboardEvent) => {
      if (event instanceof KeyboardEvent) {
        if (event.key === "Escape") setOpen(false);
        return;
      }
      if (!rootRef.current?.contains(event.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", close);
    document.addEventListener("keydown", close);
    return () => {
      document.removeEventListener("mousedown", close);
      document.removeEventListener("keydown", close);
    };
  }, [open]);

  return (
    <div ref={rootRef} className="relative">
      <button
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={menuId}
        onClick={() => setOpen((current) => !current)}
        className={cn(
          "flex items-center gap-1.5 rounded-[4px] transition-colors hover:text-gum-black",
          open && "text-gum-black",
        )}
      >
        Resources
        <ChevronDown
          className={cn("size-4 transition-transform duration-200", open && "rotate-180")}
          aria-hidden="true"
        />
      </button>
      {open ? (
        <ul
          id={menuId}
          role="menu"
          className="landing-dialog absolute top-full right-0 z-40 mt-3 w-[168px] rounded-[10px] border border-gum-grey/30 bg-gum-white p-1.5 text-[15px] text-gum-black shadow-[0_16px_40px_-20px_rgb(18_18_18/0.35)]"
        >
          {RESOURCES.map((entry) => (
            <li key={entry.label} role="none">
              <a
                role="menuitem"
                href={entry.href}
                target={entry.href.startsWith("mailto:") ? undefined : "_blank"}
                rel={entry.href.startsWith("mailto:") ? undefined : "noopener noreferrer"}
                onClick={() => setOpen(false)}
                className="block rounded-[6px] px-3 py-2 transition-colors hover:bg-gum-black/[0.05]"
              >
                {entry.label}
              </a>
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
