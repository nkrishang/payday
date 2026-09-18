"use client";

import { ChevronDown, Mail } from "lucide-react";
import { useEffect, useId, useRef, useState, type ComponentType, type SVGProps } from "react";
import { cn } from "@/lib/cn";
import { GitHubIcon, XIcon } from "./icons";
import { RESOURCES } from "./links";

const ICONS: Record<(typeof RESOURCES)[number]["label"], ComponentType<SVGProps<SVGSVGElement>>> = {
  GitHub: GitHubIcon,
  X: XIcon,
  Support: Mail,
};

/** How long the pointer may be off the menu before it closes: long enough
 *  to cross from the button to the list, or to overshoot a row. */
const CLOSE_DELAY_MS = 140;

/**
 * The header's one dropdown. It opens under the pointer and closes once the
 * pointer has left it; a tap or a keypress toggles it for touch and keyboard
 * users, and Escape, an outside click or focus leaving it all close it.
 */
export function ResourcesMenu({ tone = "light" }: { tone?: "light" | "dark" }) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const closeTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const menuId = useId();

  const cancelClose = () => {
    if (closeTimer.current) clearTimeout(closeTimer.current);
    closeTimer.current = null;
  };
  const show = () => {
    cancelClose();
    setOpen(true);
  };
  const hideSoon = () => {
    cancelClose();
    closeTimer.current = setTimeout(() => setOpen(false), CLOSE_DELAY_MS);
  };

  useEffect(() => cancelClose, []);

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
    <div
      ref={rootRef}
      className="relative"
      onMouseEnter={show}
      onMouseLeave={hideSoon}
      onBlur={(event) => {
        if (!rootRef.current?.contains(event.relatedTarget as Node | null)) setOpen(false);
      }}
    >
      <button
        type="button"
        aria-haspopup="menu"
        aria-expanded={open}
        aria-controls={menuId}
        onClick={() => setOpen((current) => !current)}
        className={cn(
          "flex items-center gap-1.5 rounded-[4px] transition-colors",
          tone === "dark" ? "hover:text-gum-white" : "hover:text-gum-black",
          open && (tone === "dark" ? "text-gum-white" : "text-gum-black"),
        )}
      >
        Resources
        <ChevronDown
          className={cn("size-4 transition-transform duration-200", open && "rotate-180")}
          aria-hidden="true"
        />
      </button>
      {open ? (
        // The padding is the bridge: the gap between the button and the list
        // stays inside the menu, so crossing it never counts as leaving.
        <div className="absolute top-full right-0 z-50 pt-3">
          <ul
            id={menuId}
            role="menu"
            className="landing-dialog w-[188px] rounded-[10px] border border-gum-grey/30 bg-gum-white p-1.5 text-[15px] text-gum-black shadow-[0_18px_40px_-16px_rgb(18_18_18/0.35)]"
          >
            {RESOURCES.map((entry) => {
              const Icon = ICONS[entry.label];
              return (
                <li key={entry.label} role="none">
                  <a
                    role="menuitem"
                    href={entry.href}
                    target={entry.href.startsWith("mailto:") ? undefined : "_blank"}
                    rel={entry.href.startsWith("mailto:") ? undefined : "noopener noreferrer"}
                    onClick={() => setOpen(false)}
                    className="group flex items-center gap-2.5 rounded-[6px] px-3 py-2 transition-colors hover:bg-gum-black/[0.05]"
                  >
                    <Icon className="size-4 shrink-0 text-gum-grey transition-colors group-hover:text-gum-black" />
                    {entry.label}
                  </a>
                </li>
              );
            })}
          </ul>
        </div>
      ) : null}
    </div>
  );
}
