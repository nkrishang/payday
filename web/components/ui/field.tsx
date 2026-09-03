import * as React from "react";
import { cn } from "@/lib/cn";

/**
 * Form primitives for the dashboard, in the checkout's visual language. A
 * `Field` wraps one control in its label so assistive tech and tests find it
 * by name; the control classes are exported for the odd case that needs a
 * bare element.
 */

export const controlStyles =
  "w-full rounded-[10px] border border-line-strong bg-surface px-3 text-[14px] text-ink placeholder:text-faint disabled:opacity-50";

export function Field({
  label,
  hint,
  className,
  children,
}: {
  label: string;
  hint?: string;
  className?: string;
  children: React.ReactNode;
}) {
  const hintId = React.useId();
  // The hint sits outside the label so the control's accessible name is the
  // label alone; it is still announced, through aria-describedby.
  const control =
    hint && React.isValidElement<{ "aria-describedby"?: string }>(children)
      ? React.cloneElement(children, { "aria-describedby": hintId })
      : children;
  return (
    <div className={className}>
      <label className="block">
        <span className="block text-[12px] font-medium text-muted">{label}</span>
        <span className="mt-1.5 block">{control}</span>
      </label>
      {hint ? (
        <p id={hintId} className="mt-1.5 text-[12px] text-faint">
          {hint}
        </p>
      ) : null}
    </div>
  );
}

export function Input({ className, ...props }: React.InputHTMLAttributes<HTMLInputElement>) {
  return <input className={cn(controlStyles, "h-10", className)} {...props} />;
}

export function Textarea({
  className,
  ...props
}: React.TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return (
    <textarea
      className={cn(controlStyles, "min-h-20 py-2 leading-relaxed", className)}
      {...props}
    />
  );
}

export function Select({ className, ...props }: React.SelectHTMLAttributes<HTMLSelectElement>) {
  return <select className={cn(controlStyles, "h-10", className)} {...props} />;
}

/** A heading for a group of fields. */
export function Fieldset({
  legend,
  description,
  children,
}: {
  legend: string;
  description?: string;
  children: React.ReactNode;
}) {
  return (
    <fieldset className="rounded-[16px] border border-line bg-surface p-5">
      <legend className="px-1 text-[13px] font-semibold tracking-tight">{legend}</legend>
      {description ? <p className="mb-4 text-[13px] text-muted">{description}</p> : null}
      <div className="grid gap-4">{children}</div>
    </fieldset>
  );
}

/** Inline problem text, announced. */
export function Problem({ children }: { children: React.ReactNode }) {
  if (!children) return null;
  return (
    <p role="alert" className="text-[13px] text-danger">
      {children}
    </p>
  );
}
