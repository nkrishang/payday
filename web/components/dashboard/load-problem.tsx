"use client";

import { CloudOff, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/cn";

/**
 * A read that could not be completed, once every retry has been spent.
 *
 * This is the last resort, not the first: congestion and hiccups are retried
 * by the client and the resource cache before anything reaches the page, so
 * what shows here is an outage, a lost connection, or a record that is gone.
 * It says so in the merchant's terms and keeps the API's message and request
 * id in the small print, where support can read them off. `compact` is the
 * inline form for a section of a page that otherwise stands; the default is
 * the page itself, in place of its skeleton.
 */
export function LoadProblem({
  title,
  message,
  detail,
  onRetry,
  compact = false,
  className,
}: {
  /** What did not load, as a headline. */
  title: string;
  /** Why, in one sentence. */
  message: string | null;
  /** The API's own words, for the small print. */
  detail?: string | null | undefined;
  onRetry: () => void;
  compact?: boolean;
  className?: string;
}) {
  if (compact) {
    return (
      <div
        role="alert"
        className={cn(
          "flex flex-wrap items-center gap-x-4 gap-y-3 rounded-[12px] border border-line bg-surface px-4 py-3.5",
          className,
        )}
      >
        <CloudOff aria-hidden="true" className="size-4 shrink-0 text-muted" />
        <div className="min-w-0 flex-1">
          <p className="text-[13px] font-medium">{title}</p>
          {message ? <p className="mt-0.5 text-[12.5px] text-muted">{message}</p> : null}
          {detail ? (
            <p className="mt-1 font-mono text-[11px] break-all text-faint">{detail}</p>
          ) : null}
        </div>
        <Button type="button" variant="secondary" size="sm" onClick={onRetry}>
          <RefreshCw aria-hidden="true" className="size-3.5" />
          Try again
        </Button>
      </div>
    );
  }

  return (
    <div
      role="alert"
      className={cn("dash-enter mx-auto max-w-[520px] pt-10 text-center sm:pt-20", className)}
    >
      <span className="mx-auto grid size-12 place-items-center rounded-full border border-line bg-surface">
        <CloudOff aria-hidden="true" className="size-5 text-muted" />
      </span>
      <h1 className="font-heading mt-6 text-[26px] leading-tight font-medium tracking-[-0.04em]">
        {title}
      </h1>
      {message ? (
        <p className="mx-auto mt-3 max-w-[400px] text-[15px] text-muted">{message}</p>
      ) : null}
      <div className="mt-7">
        <Button type="button" variant="secondary" onClick={onRetry}>
          <RefreshCw aria-hidden="true" className="size-4" />
          Try again
        </Button>
      </div>
      {detail ? (
        <p className="mx-auto mt-8 max-w-[440px] font-mono text-[11px] break-all text-faint">
          {detail}
        </p>
      ) : null}
    </div>
  );
}
