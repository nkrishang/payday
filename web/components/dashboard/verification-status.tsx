import type { PayerPolicy, PayerPolicyMode } from "@payday/sdk";
import { AlertTriangle } from "lucide-react";
import { StatusDot } from "@/components/ui/status-dot";
import { formatDate, modeLabel } from "./labels";

/**
 * Verification is a separate fact from payment. A gated invoice can be funded
 * before its payer has verified — that is the "likely unsolicited" case — and
 * a verified invoice can still be unpaid. Both components keep the two apart.
 */

type Facts = {
  mode: PayerPolicyMode;
  completedAt: string | null;
  unsolicitedAt: string | null;
};

function verdict({ mode, completedAt }: Facts): {
  label: string;
  tone: "neutral" | "success" | "warning";
} {
  if (mode === "permissionless") return { label: "Not required", tone: "neutral" };
  if (completedAt) return { label: "Verified", tone: "success" };
  return { label: "Pending", tone: "warning" };
}

/** One line for a table cell. */
export function VerificationBadge(facts: Facts) {
  const { label, tone } = verdict(facts);
  return (
    // Wraps rather than overflows: the unsolicited flag is a second fact, and
    // in a narrow column it belongs under the verdict, not past the cell.
    <span className="flex flex-wrap items-center gap-x-2 gap-y-1 text-[13px]">
      <StatusDot tone={tone} />
      {label}
      {facts.unsolicitedAt ? (
        <span
          className="inline-flex items-center gap-1 rounded-md border border-warning/40 px-1.5 py-0.5 text-[11px] font-medium text-warning"
          title={`Finalized funds arrived ${formatDate(facts.unsolicitedAt)}, before verification completed`}
        >
          <AlertTriangle className="size-3" />
          Likely unsolicited
        </span>
      ) : null}
    </span>
  );
}

/** The full picture for the detail page, including the merchant's assertion. */
export function VerificationStatus({
  policy,
  completedAt,
  unsolicitedAt,
}: {
  policy: PayerPolicy;
  completedAt: string | null;
  unsolicitedAt: string | null;
}) {
  const { label, tone } = verdict({ mode: policy.mode, completedAt, unsolicitedAt });

  return (
    <div>
      <dl className="grid grid-cols-[auto_minmax(0,1fr)] gap-x-4 gap-y-2 text-[13px]">
        <dt className="text-faint">Policy</dt>
        <dd className="font-medium">{modeLabel(policy.mode)}</dd>

        {policy.mode !== "permissionless" ? (
          <>
            <dt className="text-faint">Expected email</dt>
            <dd className="font-mono break-all">{policy.expected_email}</dd>
          </>
        ) : null}

        <dt className="text-faint">Verification</dt>
        <dd className="flex items-center gap-2">
          <StatusDot tone={tone} />
          <span className="font-medium">{label}</span>
          {completedAt ? <span className="text-muted">· {formatDate(completedAt)}</span> : null}
        </dd>
      </dl>

      {unsolicitedAt ? (
        <p
          role="note"
          className="mt-4 flex gap-2 rounded-[10px] border border-warning/40 px-3.5 py-3 text-[13px] leading-relaxed text-warning"
        >
          <AlertTriangle className="mt-0.5 size-4 shrink-0" />
          <span>
            <span className="font-medium">Likely unsolicited.</span> Finalized funds arrived{" "}
            {formatDate(unsolicitedAt)}, before the expected payer verified. The address is not
            quarantined; settlement waits for verification, and unverified funds are recovered at
            expiry.
          </span>
        </p>
      ) : null}
    </div>
  );
}
