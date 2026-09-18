"use client";

import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * The scaffolding under every animated picture on the landing page.
 *
 * A picture is a fixed-size composition that scales to whatever width it is
 * given, so it never reflows on a phone; and it is a pure function of one
 * clock, milliseconds into a loop, so nothing is scheduled and a tab that
 * sleeps and wakes picks up mid-story rather than firing a backlog of timers.
 */

/**
 * Milliseconds into the loop, advancing with the frame rate and wrapping at
 * the end. A frame after a long pause counts as a short one, so a background
 * tab does not skip ahead when it comes back.
 */
export function useLoopClock(total: number, frozenAt: number | null, running: boolean): number {
  const [t, setT] = useState(0);
  // Where the loop is, kept outside the effect so a pause (the picture
  // scrolling out of view) resumes mid-story rather than from the top.
  const elapsedRef = useRef(0);

  useEffect(() => {
    if (frozenAt !== null || !running) return;
    let frame = 0;
    let last = performance.now();
    let shown = elapsedRef.current;
    const tick = (now: number) => {
      const dt = Math.min(now - last, 100);
      last = now;
      const elapsed = (elapsedRef.current + dt) % total;
      elapsedRef.current = elapsed;
      // Thirty frames a second is plenty for typing and a moving cursor.
      if (Math.abs(elapsed - shown) >= 33 || elapsed < shown) {
        shown = elapsed;
        setT(elapsed);
      }
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [total, frozenAt, running]);

  return frozenAt ?? t;
}

/**
 * Whether an element is on screen. Off screen, its loop stops: a phone
 * scrolling past a picture should not be re-rendering it thirty times a
 * second underneath the scroll.
 */
export function useInView(ref: React.RefObject<HTMLElement | null>): boolean {
  const [inView, setInView] = useState(true);
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const observer = new IntersectionObserver(
      ([entry]) => setInView(entry?.isIntersecting ?? true),
      { threshold: 0.05 },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref]);
  return inView;
}

export function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(false);
  useEffect(() => {
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => setReduced(query.matches);
    update();
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, []);
  return reduced;
}

/**
 * A composition laid out at its own size and scaled to the frame. Until the
 * first measurement has been applied it stays invisible, so the first paint
 * on a phone never shows it at full size for a frame before it snaps down;
 * it fades in at the right size instead.
 */
export function Stage({
  width,
  height,
  label,
  className,
  children,
}: {
  width: number;
  height: number;
  label: string;
  className?: string;
  children: ReactNode;
}) {
  const frameRef = useRef<HTMLDivElement>(null);
  const [scale, setScale] = useState<number | null>(null);

  useLayoutEffect(() => {
    const frame = frameRef.current;
    if (!frame) return;
    const fit = (frameWidth: number) => setScale(frameWidth / width);
    fit(frame.clientWidth);
    const observer = new ResizeObserver(([entry]) => {
      if (entry) fit(entry.contentRect.width);
    });
    observer.observe(frame);
    return () => observer.disconnect();
  }, [width]);

  return (
    <div
      ref={frameRef}
      role="img"
      aria-label={label}
      className={cn("relative w-full overflow-hidden", className)}
      style={{ aspectRatio: `${width} / ${height}` }}
    >
      <div
        className={cn(
          "absolute top-0 left-0 origin-top-left",
          scale === null ? "invisible" : "landing-stage-fade-in",
        )}
        style={{ width, height, transform: `scale(${scale ?? 1})` }}
      >
        {children}
      </div>
    </div>
  );
}
