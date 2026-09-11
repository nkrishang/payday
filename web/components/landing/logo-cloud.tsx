"use client";

import Image from "next/image";
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * Logos as coins that arrive with a little theatre.
 *
 * A `LogoBurst` holds a group of `Coin`s. Until the group scrolls into view
 * the coins sit stacked at its centre, small and faded; then each one
 * travels up and out to its place in the layout with a touch of overshoot,
 * staggered so the group unfolds rather than snaps. Settled, they bob
 * gently, each on its own phase, and a hovered coin lifts and wiggles.
 *
 * The start of each coin's journey is the group's centre expressed as an
 * offset from where the coin lands, measured from layout positions so the
 * transforms themselves never skew the measurement. The motion is all CSS
 * (see `.landing-chip` in globals.css); this only measures and flips the
 * `data-settled` switch.
 */
export function LogoBurst({ className, children }: { className?: string; children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);
  const [settled, setSettled] = useState(false);

  useLayoutEffect(() => {
    const group = ref.current;
    if (!group) return;
    const place = () => {
      const cx = group.clientWidth / 2;
      const cy = group.clientHeight / 2;
      group.querySelectorAll<HTMLElement>("[data-chip]").forEach((chip, index) => {
        chip.style.setProperty("--dx", `${cx - (chip.offsetLeft + chip.offsetWidth / 2)}px`);
        chip.style.setProperty("--dy", `${cy - (chip.offsetTop + chip.offsetHeight / 2) + 28}px`);
        chip.style.setProperty("--i", String(index));
      });
    };
    place();
    const observer = new ResizeObserver(place);
    observer.observe(group);
    return () => observer.disconnect();
  }, []);

  useEffect(() => {
    const group = ref.current;
    if (!group) return;
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry?.isIntersecting) {
          setSettled(true);
          observer.disconnect();
        }
      },
      { threshold: 0.45 },
    );
    observer.observe(group);
    return () => observer.disconnect();
  }, []);

  return (
    <div ref={ref} data-settled={settled || undefined} className={cn("landing-burst", className)}>
      {children}
    </div>
  );
}

/**
 * One logo on a coin: a bevelled tile with an edge and a soft drop, the
 * mark centred on its face, and a label under it when there is one.
 */
export function Coin({
  src,
  name,
  label,
  size = "sm",
  wide = false,
  className,
}: {
  src: string;
  name: string;
  /** Shown under the coin; without it the name is the image's alt text. */
  label?: string;
  size?: "sm" | "lg";
  /** A wordmark rather than a mark: the coin stretches to hold it. */
  wide?: boolean;
  className?: string;
}) {
  const large = size === "lg";
  return (
    <div data-chip title={label ? undefined : name} className={cn("landing-chip", className)}>
      <div className="landing-chip-body flex flex-col items-center gap-3">
        <div
          className={cn(
            "landing-coin flex items-center justify-center",
            large ? "size-[88px] rounded-[22px]" : "h-[52px] rounded-[15px]",
            !large && (wide ? "w-[128px] px-4" : "w-[52px]"),
          )}
        >
          <Image
            src={src}
            width={large ? 64 : 96}
            height={large ? 64 : 28}
            alt={label ? "" : name}
            className={cn("w-auto", large ? "h-11" : wide ? "h-5" : "h-[26px]")}
          />
        </div>
        {label ? <span className="text-[13px] font-medium">{label}</span> : null}
      </div>
    </div>
  );
}
