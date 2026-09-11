"use client";

import Image from "next/image";
import { useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * Logos that fall into place.
 *
 * A `LogoDrop` holds a group of `Mark`s at the foot of a card. A beat after
 * the page loads, each mark drops from the top of the card to where it
 * sits, lands with a bounce, and stays put, one mark a little after the
 * last so the group tumbles in rather than landing as one. How far each
 * falls is measured here (its resting spot's distance from the card's top)
 * and handed to CSS as `--fall`; the fall itself is a keyframe in
 * globals.css (`.landing-drop`).
 *
 * The marks are hidden until they are measured, so the first paint never
 * shows them sitting where they have not yet landed.
 */
export function LogoDrop({ className, children }: { className?: string; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const [ready, setReady] = useState(false);

  useLayoutEffect(() => {
    const group = ref.current;
    if (!group) return;
    // The card is the nearest article: its top edge is where the fall begins.
    const card = group.closest("article") ?? group;
    const measure = () => {
      const top = card.getBoundingClientRect().top;
      group.querySelectorAll<HTMLElement>("[data-mark]").forEach((mark, index) => {
        const rect = mark.getBoundingClientRect();
        mark.style.setProperty("--fall", `${rect.top - top + rect.height}px`);
        mark.style.setProperty("--i", String(index));
        // A little lean on the way down, alternating, so they read as things.
        mark.style.setProperty("--tilt", `${((index % 3) - 1) * 9}deg`);
      });
    };
    measure();
    // Measured before any animation applies, so the rectangles are the
    // resting ones; `ready` then starts the drop. Deferred a tick so the
    // effect itself does not set state synchronously.
    void Promise.resolve().then(() => setReady(true));
  }, []);

  return (
    <div ref={ref} data-ready={ready || undefined} className={cn("landing-drop", className)}>
      {children}
    </div>
  );
}

/**
 * One logo, given body: the mark itself, with an extrusion and a drop that
 * follow its own silhouette, and a gloss clipped to its shape. No tile
 * behind it; the logo is the object.
 */
export function Mark({
  src,
  name,
  size = "sm",
  className,
  style,
}: {
  src: string;
  name: string;
  size?: "sm" | "lg";
  className?: string;
  /** Where it comes to rest, for a pile: position, and `--rest` for its lean. */
  style?: React.CSSProperties;
}) {
  const large = size === "lg";
  return (
    <div data-mark title={name} className={cn("landing-mark-drop", className)} style={style}>
      <span
        className="landing-mark relative inline-flex px-1 py-1"
        style={{ "--logo": `url(${src})` } as React.CSSProperties}
      >
        <Image
          src={src}
          width={64}
          height={64}
          alt={name}
          className={cn("relative w-auto", large ? "h-16" : "h-9")}
        />
        <span aria-hidden="true" className="landing-mark-gloss" />
      </span>
    </div>
  );
}
