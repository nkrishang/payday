"use client";

import { usePathname } from "next/navigation";
import { useEffect, useState } from "react";
import { cn } from "@/lib/cn";

interface Heading {
  id: string;
  text: string;
  level: 2 | 3;
}

/**
 * The page's own headings, read from the article after it renders, with the
 * one currently in view marked. Pages write ordinary `h2`/`h3` elements with
 * ids; nothing declares its outline twice.
 */
export function Toc() {
  const pathname = usePathname();
  const [headings, setHeadings] = useState<Heading[]>([]);
  const [active, setActive] = useState<string | null>(null);

  useEffect(() => {
    // The article is a sibling rendered by the server; it is read once it has
    // painted, which is also what keeps this from setting state mid-effect.
    let observer: IntersectionObserver | null = null;
    const frame = requestAnimationFrame(() => {
      const article = document.querySelector("article.docs-prose");
      if (!article) return;
      const found = Array.from(article.querySelectorAll<HTMLHeadingElement>("h2[id], h3[id]")).map(
        (element) => ({
          id: element.id,
          // The heading's own words, without the "#" anchor that follows them.
          text: Array.from(element.childNodes)
            .filter(
              (node) => !(node instanceof HTMLElement && node.classList.contains("docs-anchor")),
            )
            .map((node) => node.textContent ?? "")
            .join(" ")
            .replace(/\s+/g, " ")
            .trim(),
          level: element.tagName === "H2" ? (2 as const) : (3 as const),
        }),
      );
      setHeadings(found);
      setActive(found[0]?.id ?? null);

      // The heading nearest the top of the viewport, below the sticky header.
      observer = new IntersectionObserver(
        (entries) => {
          const visible = entries
            .filter((entry) => entry.isIntersecting)
            .sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top);
          const first = visible[0];
          if (first) setActive(first.target.id);
        },
        { rootMargin: "-80px 0px -70% 0px", threshold: 0 },
      );
      for (const heading of found) {
        const element = document.getElementById(heading.id);
        if (element) observer.observe(element);
      }
    });
    return () => {
      cancelAnimationFrame(frame);
      observer?.disconnect();
    };
  }, [pathname]);

  if (headings.length === 0) return null;

  return (
    <nav aria-label="On this page">
      <p className="mb-3 font-heading text-[11px] font-medium tracking-[0.08em] text-brand-grey uppercase">
        On this page
      </p>
      <ul className="flex flex-col gap-0.5 border-l border-brand-grey/20">
        {headings.map((heading) => (
          <li key={heading.id}>
            <a
              href={`#${heading.id}`}
              className={cn(
                "-ml-px block border-l py-1 text-[13px] leading-snug transition-colors",
                heading.level === 3 ? "pl-6" : "pl-3",
                active === heading.id
                  ? "border-brand-green text-brand-white"
                  : "border-transparent text-[#b0afa9] hover:text-brand-white",
              )}
            >
              {heading.text}
            </a>
          </li>
        ))}
      </ul>
    </nav>
  );
}
