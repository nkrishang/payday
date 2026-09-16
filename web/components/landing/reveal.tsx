"use client";

import { useEffect, useRef, type ReactNode } from "react";

/**
 * A group of things that arrive as the reader scrolls to them.
 *
 * The group watches itself; once enough of it is on screen it flips
 * `data-in`, and every descendant carrying `data-reveal` transitions in,
 * staggered by its `--i` (see globals.css). The flip is a DOM write rather
 * than state: nothing inside re-renders, and a group that has arrived stays
 * arrived. Before the browser has had a chance to look, the descendants are
 * invisible, which is the point; the hero above the fold uses the load-time
 * `landing-reveal` animation instead, so it never waits on an observer.
 */
export function Reveal({
  as = "section",
  className,
  children,
  ...rest
}: {
  as?: "section" | "div";
  className?: string;
  children: ReactNode;
  "aria-label"?: string;
  "aria-labelledby"?: string;
}) {
  const ref = useRef<HTMLElement>(null);

  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (!entry?.isIntersecting) return;
        element.dataset.in = "true";
        observer.disconnect();
      },
      // Far enough in that the arrival is seen, not lost under the fold.
      { threshold: 0.15, rootMargin: "0px 0px -8% 0px" },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, []);

  if (as === "div") {
    return (
      <div
        ref={ref as React.RefObject<HTMLDivElement>}
        className={className}
        data-in="false"
        {...rest}
      >
        {children}
      </div>
    );
  }
  return (
    <section ref={ref} className={className} data-in="false" {...rest}>
      {children}
    </section>
  );
}
