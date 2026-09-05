import { cloneElement, isValidElement, useId, type ReactNode } from "react";

/**
 * One control in its label, with the field's own problem under it.
 *
 * The hint and the error sit outside the label — as they do in `ui/field` —
 * so the control's accessible name stays the label alone, and they are
 * announced through `aria-describedby` instead.
 */

interface Described {
  "aria-describedby"?: string;
  "aria-required"?: true;
}

export function Labeled({
  label,
  hint,
  error,
  required,
  children,
}: {
  label: string;
  hint?: string | undefined;
  error?: string | undefined;
  /**
   * Marks a control the step will not accept empty. Worth setting only where
   * that is not already obvious — a field whose requirement depends on another
   * answer, or one that arrives filled in and can be blanked by mistake.
   */
  required?: boolean | undefined;
  children: ReactNode;
}) {
  const noteId = useId();
  const note = error ?? hint;
  const attrs: Described = {
    ...(note ? { "aria-describedby": noteId } : {}),
    ...(required ? { "aria-required": true as const } : {}),
  };
  const control =
    Object.keys(attrs).length > 0 && isValidElement<Described>(children)
      ? cloneElement(children, attrs)
      : children;
  return (
    <div>
      <label className="block">
        <span className="block text-[12px] font-medium text-muted">
          {label}
          {/* Hidden from the accessible name — `aria-required` says the same
              thing to assistive tech without lengthening the label. */}
          {required ? (
            <span aria-hidden="true" className="ml-2 font-normal text-faint">
              Required
            </span>
          ) : null}
        </span>
        <span className="mt-1.5 block">{control}</span>
      </label>
      {error ? (
        <p id={noteId} role="alert" className="mt-1.5 text-[12px] text-danger">
          {error}
        </p>
      ) : hint ? (
        <p id={noteId} className="mt-1.5 text-[12px] text-faint">
          {hint}
        </p>
      ) : null}
    </div>
  );
}
