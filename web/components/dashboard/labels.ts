import type { PayerPolicyMode, PaymentStatus } from "@payday/sdk";
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

/** The four presets, in the order the form offers them. */
export const MODES: ReadonlyArray<{ value: PayerPolicyMode; label: string; description: string }> =
  [
    {
      value: "permissionless",
      label: "Permissionless",
      description: "Anyone holding the link can view and pay it. Content is visible immediately.",
    },
    {
      value: "verified_email",
      label: "Verified email",
      description:
        "The payer must prove ownership of the expected email before the amount, details, and address are shown.",
    },
    {
      value: "verified_identity",
      label: "Verified identity",
      description:
        "Email ownership plus a document and liveness check that must match the named person.",
    },
    {
      value: "verified_identity_unattributed",
      label: "Verified identity, unattributed",
      description:
        "Email ownership plus a document and liveness check, without asserting who the payer is.",
    },
  ];

export function modeLabel(mode: PayerPolicyMode): string {
  return MODES.find((entry) => entry.value === mode)?.label ?? mode;
}

export function formatDate(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return iso;
  return date.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}
