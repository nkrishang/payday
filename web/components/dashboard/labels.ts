import type { PayerPolicyMode, PaymentStatus } from "@payday/sdk";
import type { ComposerMode } from "./create-payment";
import type { CheckoutTone } from "@/lib/checkout-state";

/** Merchant-facing words for the API's status values, in lifecycle order. */
export const STATUSES: ReadonlyArray<{ value: PaymentStatus; label: string; tone: CheckoutTone }> =
  [
    { value: "awaiting_payment", label: "Awaiting payment", tone: "neutral" },
    { value: "partially_paid", label: "Partially paid", tone: "progress" },
    { value: "paid", label: "Received", tone: "progress" },
    { value: "settled", label: "Settled", tone: "success" },
    { value: "expired", label: "Expired", tone: "neutral" },
    { value: "returned", label: "Returned", tone: "warning" },
    { value: "needs_attention", label: "Needs attention", tone: "warning" },
  ];

export function statusLabel(status: PaymentStatus): string {
  return STATUSES.find((entry) => entry.value === status)?.label ?? status;
}

export function statusTone(status: PaymentStatus): CheckoutTone {
  return STATUSES.find((entry) => entry.value === status)?.tone ?? "neutral";
}

/**
 * The two presets the composer offers, in order. The third mode,
 * `merchant_session`, is deliberately absent: it exists for an application
 * that has signed its user in and can hand them the client secret, which a
 * request composed by hand in the dashboard has no way to do. It is created
 * through the API only, and the dashboard shows it once it exists.
 */
export const MODES: ReadonlyArray<{ value: ComposerMode; label: string; description: string }> = [
  {
    value: "permissionless",
    label: "Permissionless",
    description:
      "Anyone holding a link to the deposit request can view payment details and pay it.",
  },
  {
    value: "verified_email",
    label: "Verified email",
    description:
      "The payer must prove ownership of the expected email before the amount, details, and address are shown.",
  },
];

/** Every mode's merchant-facing name, including the API-only one. */
const MODE_LABELS: Record<PayerPolicyMode, string> = {
  permissionless: "Permissionless",
  verified_email: "Verified email",
  merchant_session: "Your app's sign-in",
};

export function modeLabel(mode: PayerPolicyMode): string {
  return MODE_LABELS[mode] ?? mode;
}

export function formatDate(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

/**
 * How long ago, or how long until — "in 6 days", "2 hours ago". A deadline and
 * a milestone are both read as distances from now, and a distance is what the
 * merchant is actually asking for; the exact stamp sits beside it where it
 * matters.
 */
export function formatRelative(iso: string, now = Date.now()): string {
  const at = new Date(iso).getTime();
  if (Number.isNaN(at)) return iso;
  const seconds = Math.round((at - now) / 1000);
  const units: ReadonlyArray<[Intl.RelativeTimeFormatUnit, number]> = [
    ["year", 31_536_000],
    ["month", 2_592_000],
    ["day", 86_400],
    ["hour", 3_600],
    ["minute", 60],
  ];
  const format = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
  for (const [unit, size] of units) {
    if (Math.abs(seconds) >= size) return format.format(Math.round(seconds / size), unit);
  }
  return format.format(seconds, "second");
}

/**
 * The date alone, for a table cell. A row is scanned, not read: the minute a
 * request was issued belongs on its detail page, where there is room for it.
 */
export function formatShortDate(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleDateString(undefined, { dateStyle: "medium" });
}
