import { AlertTriangle, ArrowUpRight, Info, ShieldAlert } from "lucide-react";
import Link from "next/link";
import type { ReactNode } from "react";
import { StatusDot } from "@/components/ui/status-dot";
import type { CheckoutTone } from "@/lib/checkout-state";
import { cn } from "@/lib/cn";
import { Pager } from "./pager";

/**
 * The building blocks every documentation page is written with. The page
 * body is ordinary HTML — paragraphs, lists, tables, headings with ids —
 * styled by `.docs-prose` in globals.css; these components are the pieces
 * plain HTML has no word for.
 */

export function DocsPage({
  eyebrow,
  title,
  lead,
  children,
}: {
  /** The group the page belongs to, above the title. */
  eyebrow: string;
  title: ReactNode;
  /** One or two sentences under the title saying what the page is for. */
  lead: ReactNode;
  children: ReactNode;
}) {
  return (
    <article className="docs-prose">
      <p className="mb-3 font-heading text-[12px] font-medium tracking-[0.08em] text-brand-green uppercase">
        {eyebrow}
      </p>
      <h1 className="font-heading text-[clamp(30px,4vw,40px)] leading-[1.08] font-medium tracking-[-0.045em] text-balance">
        {title}
      </h1>
      <p className="mt-4 mb-10 max-w-[640px] text-[17px] leading-[1.6] text-muted">{lead}</p>
      {children}
      <Pager />
    </article>
  );
}

/** A section heading with a stable id, so the outline and deep links reach it. */
export function H2({ id, children }: { id: string; children: ReactNode }) {
  return (
    <h2 id={id}>
      {children}
      <a href={`#${id}`} className="docs-anchor" aria-label="Link to this section">
        #
      </a>
    </h2>
  );
}

export function H3({ id, children }: { id: string; children: ReactNode }) {
  return (
    <h3 id={id}>
      {children}
      <a href={`#${id}`} className="docs-anchor" aria-label="Link to this section">
        #
      </a>
    </h3>
  );
}

/** A table in a scrolling, bordered frame. */
export function Table({ children }: { children: ReactNode }) {
  return (
    <div className="docs-table">
      <table>{children}</table>
    </div>
  );
}

type CalloutTone = "note" | "warning" | "danger";

const CALLOUT: Record<CalloutTone, { icon: typeof Info; className: string; label: string }> = {
  note: { icon: Info, className: "border-line-strong", label: "Note" },
  warning: { icon: AlertTriangle, className: "border-warning/50", label: "Careful" },
  danger: { icon: ShieldAlert, className: "border-danger/50", label: "Important" },
};

export function Callout({
  tone = "note",
  title,
  children,
}: {
  tone?: CalloutTone;
  title?: string;
  children: ReactNode;
}) {
  const { icon: Icon, className, label } = CALLOUT[tone];
  return (
    <aside
      className={cn(
        "my-6 rounded-[12px] border bg-surface px-4 py-3.5 text-[14.5px] leading-[1.6] text-ink",
        className,
      )}
    >
      <p
        className={cn(
          "mb-1 flex items-center gap-2 font-heading text-[13px] font-medium",
          tone === "warning" && "text-warning",
          tone === "danger" && "text-danger",
          tone === "note" && "text-muted",
        )}
      >
        <Icon className="size-3.5" />
        {title ?? label}
      </p>
      <div className="[&>p+p]:mt-2 [&_a]:text-brand-green">{children}</div>
    </aside>
  );
}

/** Numbered steps, each with a title and a body that may hold code. */
export function Steps({ children }: { children: ReactNode }) {
  return <ol className="docs-steps my-6 flex flex-col gap-0">{children}</ol>;
}

export function Step({ title, children }: { title: ReactNode; children?: ReactNode }) {
  return (
    <li className="relative grid grid-cols-[28px_minmax(0,1fr)] gap-x-4 pb-8 last:pb-0">
      <span
        aria-hidden="true"
        className="docs-step-marker relative z-10 flex size-7 items-center justify-center rounded-full border border-line-strong bg-surface font-heading text-[12px] font-medium tabular"
      />
      <div className="min-w-0 pt-0.5">
        <p className="font-heading text-[16px] font-medium tracking-[-0.01em] text-ink">{title}</p>
        {children ? (
          <div className="docs-step-body mt-2 text-[14.5px] leading-[1.65] text-muted">
            {children}
          </div>
        ) : null}
      </div>
    </li>
  );
}

/** A grid of link cards, for landing and cross-reference sections. */
export function Cards({ children }: { children: ReactNode }) {
  return <div className="my-6 grid grid-cols-1 gap-3 sm:grid-cols-2">{children}</div>;
}

export function Card({
  href,
  title,
  children,
  external = false,
}: {
  href: string;
  title: string;
  children: ReactNode;
  external?: boolean;
}) {
  const className =
    "group flex flex-col gap-1.5 rounded-[12px] border border-line bg-surface p-4 transition-colors hover:border-line-strong";
  const body = (
    <>
      <span className="flex items-center justify-between gap-2 font-heading text-[15px] font-medium text-ink">
        {title}
        <ArrowUpRight className="size-3.5 text-faint transition-colors group-hover:text-brand-green" />
      </span>
      <span className="text-[13.5px] leading-[1.55] text-muted">{children}</span>
    </>
  );
  return external ? (
    <a href={href} target="_blank" rel="noopener noreferrer" className={className}>
      {body}
    </a>
  ) : (
    <Link href={href} className={className}>
      {body}
    </Link>
  );
}

/** A diagram or illustration with a caption beneath it. */
export function Figure({ caption, children }: { caption?: ReactNode; children: ReactNode }) {
  return (
    <figure className="my-7">
      <div className="overflow-x-auto rounded-[16px] border border-line bg-surface p-5 sm:p-6">
        {children}
      </div>
      {caption ? (
        <figcaption className="mt-2.5 px-1 text-[13px] leading-[1.55] text-muted">
          {caption}
        </figcaption>
      ) : null}
    </figure>
  );
}

/** A deposit request status, the way the dashboard shows it. */
export function StatusPill({ tone, children }: { tone: CheckoutTone; children: ReactNode }) {
  return (
    <span className="inline-flex items-center gap-2 font-mono text-[12.5px] whitespace-nowrap text-ink">
      <StatusDot tone={tone} />
      {children}
    </span>
  );
}

/** A small label in the checkout's pill style; for modes, event names, and the like. */
export function Pill({
  tone = "neutral",
  children,
}: {
  tone?: "neutral" | "green" | "yellow";
  children: ReactNode;
}) {
  return (
    <span
      className={cn(
        "inline-flex h-6 items-center rounded-full border px-2.5 font-mono text-[11.5px] whitespace-nowrap",
        tone === "neutral" && "border-line-strong text-muted",
        tone === "green" && "border-brand-green/50 text-brand-green",
        tone === "yellow" && "border-brand-yellow/50 text-brand-yellow",
      )}
    >
      {children}
    </span>
  );
}

/** A definition list: term on the left, meaning on the right. */
export function Defs({ children }: { children: ReactNode }) {
  return (
    <dl className="my-6 grid grid-cols-1 gap-x-6 gap-y-3 rounded-[12px] border border-line bg-surface p-4 sm:grid-cols-[minmax(140px,max-content)_1fr]">
      {children}
    </dl>
  );
}

export function Def({ term, children }: { term: ReactNode; children: ReactNode }) {
  return (
    <>
      <dt className="font-mono text-[13px] text-ink sm:pt-0.5">{term}</dt>
      <dd className="text-[14px] leading-[1.6] text-muted">{children}</dd>
    </>
  );
}

/** A comparison of two or three things side by side. */
export function Compare({ children }: { children: ReactNode }) {
  return (
    <div className="my-6 grid grid-cols-1 gap-3 md:grid-cols-2 lg:grid-cols-3">{children}</div>
  );
}

export function CompareItem({
  title,
  badge,
  children,
}: {
  title: ReactNode;
  badge?: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-3 rounded-[12px] border border-line bg-surface p-4">
      <div className="flex items-start justify-between gap-2">
        <p className="font-heading text-[15px] font-medium text-ink">{title}</p>
        {badge}
      </div>
      <div className="text-[13.5px] leading-[1.6] text-muted [&>p+p]:mt-2 [&_ul]:list-disc [&_ul]:pl-4 [&_ul>li+li]:mt-1">
        {children}
      </div>
    </div>
  );
}
