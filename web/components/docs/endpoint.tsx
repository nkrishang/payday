import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * The reference pages' vocabulary: a route, the fields it takes, and the
 * answers it gives. The route heading carries an id so the page's outline
 * and deep links reach it.
 */

type Method = "GET" | "POST" | "PUT" | "PATCH" | "DELETE";

const METHOD_CLASS: Record<Method, string> = {
  GET: "text-brand-green border-brand-green/40",
  POST: "text-term-id border-term-id/40",
  PUT: "text-brand-yellow border-brand-yellow/40",
  PATCH: "text-brand-yellow border-brand-yellow/40",
  DELETE: "text-danger border-danger/40",
};

export function slugForRoute(method: Method, path: string): string {
  return `${method.toLowerCase()}-${path
    .replace(/^\/v1\//, "")
    .replace(/[{}]/g, "")
    .replace(/[^a-z0-9]+/gi, "-")
    .replace(/^-|-$/g, "")
    .toLowerCase()}`;
}

/** A route heading: `POST /v1/deposit-requests`. */
export function Endpoint({
  method,
  path,
  id,
  children,
}: {
  method: Method;
  path: string;
  /** Overrides the id derived from the method and path. */
  id?: string;
  /** One line on what the route does. */
  children?: ReactNode;
}) {
  const anchor = id ?? slugForRoute(method, path);
  return (
    <div className="mt-12 mb-4 first:mt-0">
      <h3
        id={anchor}
        className="docs-endpoint flex scroll-mt-24 flex-wrap items-center gap-x-3 gap-y-2 font-mono text-[15px] font-normal tracking-normal"
      >
        <MethodBadge method={method} />
        <span className="text-ink">{path}</span>
        <a href={`#${anchor}`} aria-label="Link to this route" className="docs-anchor">
          #
        </a>
      </h3>
      {children ? <p className="mt-2 text-[15px] leading-[1.65] text-muted">{children}</p> : null}
    </div>
  );
}

export function MethodBadge({ method, className }: { method: Method; className?: string }) {
  return (
    <span
      className={cn(
        "inline-flex h-6 items-center rounded-[6px] border px-2 font-mono text-[11px] font-semibold tracking-[0.04em]",
        METHOD_CLASS[method],
        className,
      )}
    >
      {method}
    </span>
  );
}

export interface ParamRow {
  name: string;
  type: string;
  required?: boolean;
  description: ReactNode;
}

/** The fields a body, a query string, or a response carries. */
export function Params({ title, rows }: { title?: string; rows: ParamRow[] }) {
  return (
    <div className="my-5 overflow-hidden rounded-[12px] border border-line bg-surface">
      {title ? (
        <p className="border-b border-line px-4 py-2.5 font-heading text-[12px] font-medium tracking-[0.06em] text-muted uppercase">
          {title}
        </p>
      ) : null}
      <dl className="divide-y divide-line">
        {rows.map((row) => (
          <div
            key={row.name}
            className="grid grid-cols-1 gap-x-6 gap-y-1.5 px-4 py-3 sm:grid-cols-[minmax(160px,220px)_1fr]"
          >
            <dt className="flex flex-wrap items-baseline gap-x-2 gap-y-1">
              <code className="docs-param">{row.name}</code>
              <span className="font-mono text-[11px] text-faint">{row.type}</span>
              {row.required ? (
                <span className="font-mono text-[10.5px] tracking-[0.04em] text-brand-yellow uppercase">
                  required
                </span>
              ) : null}
            </dt>
            <dd className="text-[14px] leading-[1.6] text-muted">{row.description}</dd>
          </div>
        ))}
      </dl>
    </div>
  );
}

/** The status codes a route answers with, beyond the success it documents. */
export function Answers({
  rows,
}: {
  rows: Array<{ status: number; code?: string; when: ReactNode }>;
}) {
  return (
    <div className="my-5 overflow-hidden rounded-[12px] border border-line bg-surface">
      <p className="border-b border-line px-4 py-2.5 font-heading text-[12px] font-medium tracking-[0.06em] text-muted uppercase">
        Answers
      </p>
      <ul className="divide-y divide-line">
        {rows.map((row, index) => (
          <li
            key={index}
            className="grid grid-cols-[52px_minmax(0,1fr)] gap-x-4 px-4 py-2.5 text-[14px] leading-[1.6]"
          >
            <span
              className={cn(
                "font-mono text-[13px] tabular",
                row.status < 300
                  ? "text-brand-green"
                  : row.status < 500
                    ? "text-brand-yellow"
                    : "text-danger",
              )}
            >
              {row.status}
            </span>
            <span className="text-muted">
              {row.code ? (
                <>
                  <code className="docs-param">{row.code}</code>{" "}
                </>
              ) : null}
              {row.when}
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}
