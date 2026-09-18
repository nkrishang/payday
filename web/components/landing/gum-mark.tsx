import type { CSSProperties } from "react";
import { cn } from "@/lib/cn";

/**
 * The gum mark, twice: as a halftone, a dot screen shown through the mark's
 * own shape, and as a solid that fills it from the bottom up (the
 * `.landing-mark-fill` rules). On the page's ground the dots are grey and
 * the fill is pink; on pink, both are white.
 */
export function GumMark({
  tone,
  fill,
  className,
}: {
  tone: "pink" | "white";
  /** Fill on page load, or when the group around it arrives. */
  fill: "load" | "reveal";
  className?: string;
}) {
  const mask: CSSProperties = {
    WebkitMaskImage: "url(/gum/mark.svg)",
    WebkitMaskSize: "contain",
    WebkitMaskRepeat: "no-repeat",
    WebkitMaskPosition: "center",
    maskImage: "url(/gum/mark.svg)",
    maskSize: "contain",
    maskRepeat: "no-repeat",
    maskPosition: "center",
  };
  const dots = tone === "pink" ? "#6f6e6e" : "rgb(247 247 245 / 0.7)";

  return (
    <span aria-hidden="true" className={cn("relative block aspect-square", className)}>
      <span
        className="absolute inset-0"
        style={{
          ...mask,
          background: `radial-gradient(circle, ${dots} 1.1px, transparent 1.4px) 0 0 / 5px 5px`,
        }}
      />
      <span
        data-fill={fill}
        className={cn(
          "landing-mark-fill absolute inset-0",
          tone === "pink" ? "bg-gum-pink" : "bg-gum-white",
        )}
        style={mask}
      />
    </span>
  );
}
