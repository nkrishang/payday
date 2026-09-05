"use client";

import { Check, ChevronDown } from "lucide-react";
import { useEffect, useId, useRef, useState, type ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * A filter control: a button that opens a list of options.
 *
 * The platform `<select>` renders its options with the operating system's own
 * chrome, which on a dark page arrives as a white sheet nothing here can style
 * — and it cannot carry a status dot beside a label. This is the same control
 * built out of a button and a listbox: full keyboard handling (arrows, Home,
 * End, Enter, Escape), dismissal on outside click, and options that can hold
 * whatever the filter is actually about.
 */

export interface MenuOption {
  value: string;
  label: string;
  /** Rendered before the label — a status dot, an avatar, an icon. */
  badge?: ReactNode;
  /** Secondary text, shown after the label. */
  detail?: string;
}

export function MenuSelect({
  label,
  value,
  options,
  onChange,
  className,
}: {
  label: string;
  value: string;
  options: MenuOption[];
  onChange: (value: string) => void;
  className?: string;
}) {
  const listId = useId();
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);

  const selected = options.find((option) => option.value === value) ?? options[0];

  // Dismissal is a document-level concern: a click anywhere else, or Escape
  // from anywhere, closes it and hands focus back to the button.
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (event: PointerEvent) => {
      if (!root.current?.contains(event.target as Node)) setOpen(false);
    };
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      setOpen(false);
      trigger.current?.focus();
    };
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  const show = () => {
    setActive(
      Math.max(
        0,
        options.findIndex((option) => option.value === value),
      ),
    );
    setOpen(true);
  };

  const choose = (next: string) => {
    onChange(next);
    setOpen(false);
    trigger.current?.focus();
  };

  const onTriggerKey = (event: React.KeyboardEvent) => {
    if (["ArrowDown", "ArrowUp", "Enter", " "].includes(event.key)) {
      event.preventDefault();
      show();
    }
  };

  const onListKey = (event: React.KeyboardEvent) => {
    if (event.key === "ArrowDown" || event.key === "ArrowUp") {
      event.preventDefault();
      const step = event.key === "ArrowDown" ? 1 : -1;
      setActive((current) => (current + step + options.length) % options.length);
      return;
    }
    if (event.key === "Home" || event.key === "End") {
      event.preventDefault();
      setActive(event.key === "Home" ? 0 : options.length - 1);
      return;
    }
    if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      const option = options[active];
      if (option) choose(option.value);
    }
  };

  return (
    // Raised while open: the entrance animations around it give each row its
    // own stacking context, and a later sibling would otherwise paint over the
    // list however high its own z-index.
    <div ref={root} className={cn("relative", open && "z-30", className)}>
      <span id={`${listId}-label`} className="block text-[12px] font-medium text-muted">
        {label}
      </span>
      <button
        ref={trigger}
        type="button"
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-labelledby={`${listId}-label`}
        onClick={() => (open ? setOpen(false) : show())}
        onKeyDown={onTriggerKey}
        className={cn(
          "mt-1.5 flex h-10 w-full items-center gap-2 rounded-[10px] border bg-surface pr-3 pl-3.5",
          "text-[13.5px] transition-colors hover:border-line-strong",
          open ? "border-brand-green/60" : "border-line-strong",
        )}
      >
        {selected?.badge}
        <span className="truncate">{selected?.label}</span>
        <ChevronDown
          aria-hidden="true"
          className={cn(
            "ml-auto size-4 shrink-0 text-faint transition-transform duration-200",
            open && "rotate-180",
          )}
        />
      </button>

      {open ? (
        <ul
          role="listbox"
          aria-labelledby={`${listId}-label`}
          aria-activedescendant={`${listId}-${active}`}
          tabIndex={-1}
          ref={(node) => node?.focus()}
          onKeyDown={onListKey}
          className="dash-menu absolute z-30 mt-1.5 max-h-[320px] w-full min-w-[200px] overflow-y-auto rounded-[10px] border border-line-strong bg-raised p-1 shadow-xl outline-none"
        >
          {options.map((option, index) => (
            <li
              key={option.value}
              id={`${listId}-${index}`}
              role="option"
              aria-selected={option.value === value}
              onPointerEnter={() => setActive(index)}
              onClick={() => choose(option.value)}
              className={cn(
                "flex cursor-pointer items-center gap-2 rounded-[7px] px-2.5 py-2 text-[13.5px]",
                index === active ? "bg-surface text-ink" : "text-muted",
              )}
            >
              {option.badge}
              <span className="truncate">{option.label}</span>
              {option.detail ? (
                <span className="truncate text-[12px] text-faint">{option.detail}</span>
              ) : null}
              {option.value === value ? (
                <Check className="ml-auto size-3.5 shrink-0 text-brand-green" />
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
    </div>
  );
}
