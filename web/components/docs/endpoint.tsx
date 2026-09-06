import type { ReactNode } from "react";
import { cn } from "@/lib/cn";
import type { Method } from "./api/types";

/**
 * The reference's vocabulary: a method badge, the fields a route takes or
 * returns, and the answers it gives.
 */

const METHOD_TEXT: Record<Method, string> = {
  GET: "text-brand-green",
  POST: "text-term-id",
  PUT: "text-brand-yellow",
  PATCH: "text-[#f0a35e]",
  DELETE: "text-danger",
};

const METHOD_TINT: Record<Method, string> = {
  GET: "bg-brand-green/12",
  POST: "bg-term-id/12",
  PUT: "bg-brand-yellow/12",
  PATCH: "bg-[#f0a35e]/12",
  DELETE: "bg-danger/12",
};

const METHOD_BORDER: Record<Method, string> = {
  GET: "border-brand-green/40",
  POST: "border-term-id/40",
  PUT: "border-brand-yellow/40",
  PATCH: "border-[#f0a35e]/40",
  DELETE: "border-danger/40",
};

/**
 * `outline` sits beside a path in a heading; `tint` is the sidebar's, a
 * filled chip of fixed width so the titles beside it line up.
 */
export function MethodBadge({
  method,
  variant = "outline",
  className,
}: {
  method: Method;
  variant?: "outline" | "tint";
  className?: string;
}) {
  return (
    <span
      className={cn(
        "inline-flex shrink-0 items-center justify-center font-mono font-semibold tracking-[0.04em]",
        variant === "outline"
          ? cn("h-6 rounded-[6px] border px-2 text-[11px]", METHOD_BORDER[method])
          : cn("h-[19px] w-[50px] rounded-[5px] text-[9.5px]", METHOD_TINT[method]),
        METHOD_TEXT[method],
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
    <div className="my-4 overflow-hidden rounded-[12px] border border-line bg-surface">
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

/** The status codes a route answers with. */
export function Answers({
  rows,
  bare = false,
}: {
  rows: Array<{ status: number; code?: string; when: ReactNode }>;
  /** Without the "Answers" chrome bar, under a heading that already says so. */
  bare?: boolean;
}) {
  return (
    <div className="my-4 overflow-hidden rounded-[12px] border border-line bg-surface">
      {bare ? null : (
        <p className="border-b border-line px-4 py-2.5 font-heading text-[12px] font-medium tracking-[0.06em] text-muted uppercase">
          Answers
        </p>
      )}
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
