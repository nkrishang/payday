"use client";

import {
  type AttachmentDescriptor,
  type Customer,
  type Issuer,
  type PayerPolicyMode,
  type Payment,
  PaydayError,
} from "@payday/sdk";
import { ArrowLeft, ArrowRight, Loader2 } from "lucide-react";
import { useRef, useState, type FormEvent, type ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { controlStyles } from "@/components/ui/field";
import { describeError } from "@/lib/attachment-upload";
import { cn } from "@/lib/cn";
import { formatDisplayAmount, truncateAddress } from "@/lib/format";
import { MenuSelect, type MenuOption } from "@/components/ui/menu-select";
import { AttachmentUpload } from "./attachment-upload";
import { buildCreatePayment, EMPTY_VALUES } from "./create-payment";
import { AMOUNT, EMAIL } from "./field-rules";
import { Labeled } from "./labeled";
import { formatDate, MODES, modeLabel } from "./labels";
import { useMerchant } from "./session";

/**
 * Issuing a deposit request without leaving the page.
 *
 * This is the only way to issue one. It asks everything `POST /v1/payments`
 * takes, but in the order a merchant answers it — how much, who owes it, what
 * they must prove, then a look before it is issued — and keeps a running
 * preview beside the questions, because an issued request is immutable and the
 * review is the last moment anything can change.
 *
 * The merchant's own side is chosen, never retyped: the issuer identity and
 * the wallet come from what they set up, and the invoice still carries its own
 * snapshot of both.
 */

interface Draft {
  amount: string;
  /** A saved customer, or "" while the billed party is being typed fresh. */
  customerId: string;
  /** The chosen issuer identity, whose party the invoice snapshots. */
  issuerId: string;
  /** One of that identity's saved wallets. */
  payoutAddressId: string;
  /** One of the presets, in hours, or `CUSTOM` for a chosen moment. */
  expiry: string;
  /** A `datetime-local` value in the merchant's own zone, when custom. */
  expiresAt: string;
  billName: string;
  billEmail: string;
  heading: string;
  reference: string;
  notes: string;
  mode: PayerPolicyMode;
  expectedEmail: string;
  firstName: string;
  lastName: string;
}

type Field = keyof Draft;
type Errors = Partial<Record<Field, string>>;

const STEPS = ["Amount", "Billing", "Verification", "Review"] as const;

/** The deadline is a moment the merchant picks rather than one of the presets. */
const CUSTOM = "custom";

const EXPIRIES = [
  { value: "24", label: "24 hours" },
  { value: "168", label: "7 days" },
  { value: "720", label: "30 days" },
  { value: CUSTOM, label: "Custom" },
] as const;

/** The window the API accepts, checked here only to save a round trip. */
const MIN_LEAD_MS = 10 * 60 * 1000;
const MAX_LEAD_MS = 366 * 24 * 60 * 60 * 1000;

/** `datetime-local` speaks local wall-clock time, as `YYYY-MM-DDTHH:mm`. */
function localMoment(at: Date): string {
  const pad = (value: number) => String(value).padStart(2, "0");
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}T${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

/** The chosen moment as the API takes it, or "" when there is not a valid one. */
function chosenMoment(draft: Draft): string {
  if (draft.expiry !== CUSTOM || !draft.expiresAt) return "";
  const at = new Date(draft.expiresAt);
  return Number.isNaN(at.getTime()) ? "" : at.toISOString();
}

/** How the deadline reads once set, for the preview and the review. */
function expiryLabel(draft: Draft): string {
  if (draft.expiry !== CUSTOM) {
    return EXPIRIES.find((option) => option.value === draft.expiry)?.label ?? "";
  }
  const moment = chosenMoment(draft);
  return moment ? formatDate(moment) : "";
}

/**
 * Opens on the only sensible defaults: the first identity and its first
 * wallet, and the customer the dashboard was asked to bill, when it was.
 */
function emptyDraft(issuers: Issuer[], customers: Customer[], billed?: string | undefined): Draft {
  const first = issuers[0];
  const customer = billed ? customers.find((entry) => entry.id === billed) : undefined;
  return {
    amount: "",
    customerId: customer?.id ?? "",
    issuerId: first?.id ?? "",
    payoutAddressId: first?.payout_addresses[0]?.id ?? "",
    expiry: "168",
    expiresAt: "",
    billName: customer?.name ?? "",
    billEmail: customer?.email ?? "",
    heading: "",
    reference: "",
    notes: "",
    mode: "permissionless",
    expectedEmail: customer?.email ?? "",
    firstName: "",
    lastName: "",
  };
}

/**
 * What this step will not send. Every rule here is one the API enforces too;
 * catching them in the browser only saves a round trip, and the API's message
 * still wins when it refuses.
 */
function validate(draft: Draft, step: number, openedAt: number): Errors {
  const errors: Errors = {};
  if (step === 0) {
    const amount = draft.amount.trim();
    if (!amount) errors.amount = "Required.";
    else if (!AMOUNT.test(amount)) errors.amount = "USDC takes up to six decimals.";
    // A decimal string is above zero exactly when it holds a non-zero digit,
    // which is a test we can make without going through a float.
    else if (!/[1-9]/.test(amount)) errors.amount = "Must be more than zero.";
    if (!draft.issuerId) errors.issuerId = "Required.";
    if (!draft.payoutAddressId) errors.payoutAddressId = "Required.";
    if (draft.expiry === CUSTOM) {
      const at = draft.expiresAt ? new Date(draft.expiresAt).getTime() : Number.NaN;
      if (!Number.isFinite(at)) errors.expiresAt = "Pick a date and time.";
      else if (at < openedAt + MIN_LEAD_MS)
        errors.expiresAt = "Must be at least 10 minutes from now.";
      else if (at > openedAt + MAX_LEAD_MS) errors.expiresAt = "Must be within 366 days.";
    }
  }
  if (step === 1) {
    if (!draft.billName.trim()) errors.billName = "Required.";
    if (draft.billEmail.trim() && !EMAIL.test(draft.billEmail.trim()))
      errors.billEmail = "Not a valid email address.";
  }
  if (step === 2 && draft.mode !== "permissionless") {
    const expected = draft.expectedEmail.trim();
    if (!expected) errors.expectedEmail = "Required for a verified policy.";
    else if (!EMAIL.test(expected)) errors.expectedEmail = "Not a valid email address.";
    if (draft.mode === "verified_identity") {
      if (!draft.firstName.trim()) errors.firstName = "Required.";
      if (!draft.lastName.trim()) errors.lastName = "Required.";
    }
  }
  return errors;
}

export function RequestComposer({
  issuers,
  customers,
  billed,
  onIssued,
  onCancel,
}: {
  /** Identities with a proven mailbox and at least one wallet; never empty. */
  issuers: Issuer[];
  /** Saved counterparties, so a repeat customer is chosen rather than retyped. */
  customers: Customer[];
  /** A customer to open on, when a customer's own page sent us here. */
  billed?: string | undefined;
  onIssued: (payment: Payment) => void;
  onCancel: () => void;
}) {
  const { client, signOut } = useMerchant();
  const [draft, setDraft] = useState<Draft>(() => emptyDraft(issuers, customers, billed));
  const [step, setStep] = useState(0);
  const [direction, setDirection] = useState<"forward" | "back">("forward");
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  // One key per distinct submission: a retry of the same values replays, and
  // any edit starts a new one, exactly as the full form does it.
  const idempotencyKey = useRef<string | null>(null);
  // A customer created for this request, so retrying after a refusal does not
  // save the same counterparty twice.
  const savedCustomer = useRef<string | null>(null);
  // A verified policy checks a mailbox, and the mailbox it usually checks is
  // the billed party's — so it follows that field until someone types their
  // own, after which it is theirs and stops moving.
  const [expectedEdited, setExpectedEdited] = useState(false);
  const [attachment, setAttachment] = useState<AttachmentDescriptor | null>(null);
  const [attachmentBusy, setAttachmentBusy] = useState(false);
  // The custom deadline is measured from when the composer opened, not from
  // every render: a bound that moves under the picker as you use it is not a
  // bound. The API checks the real window when the request arrives.
  const [openedAt] = useState(() => Date.now());

  const set = <K extends Field>(key: K, value: Draft[K]) => {
    idempotencyKey.current = null;
    if (key === "expectedEmail") setExpectedEdited(true);
    setDraft((current) => ({
      ...current,
      [key]: value,
      ...(key === "billEmail" && !expectedEdited ? { expectedEmail: String(value) } : {}),
    }));
    setFailure(null);
  };

  // Checked on every keystroke: the button below does not light up until the
  // step is answerable, and a field says why as soon as it holds something
  // that will not do.
  const errors = validate(draft, step, openedAt);
  // An attachment still uploading holds the step: leaving unmounts the upload,
  // and there is nothing to gain by racing it.
  const answered = Object.keys(errors).length === 0 && !(step === 1 && attachmentBusy);
  /** Shown only once a field holds something, so an untouched form is quiet. */
  const shown = <K extends Field>(key: K): string | undefined =>
    String(draft[key]).length > 0 ? errors[key] : undefined;

  // The one field that arrives filled in, so silence while empty would read as
  // "nothing needed here" rather than "you cleared something the policy wants".
  const expectedProblem = expectedEdited || draft.expectedEmail ? errors.expectedEmail : undefined;

  const issuer = issuers.find((entry) => entry.id === draft.issuerId) ?? issuers[0];
  const payout = issuer?.payout_addresses.find((entry) => entry.id === draft.payoutAddressId);

  /** Choosing a saved customer fills the billed party from it. */
  const chooseCustomer = (id: string) => {
    idempotencyKey.current = null;
    setFailure(null);
    const found = customers.find((entry) => entry.id === id);
    setDraft((current) => ({
      ...current,
      customerId: id,
      billName: found?.name ?? "",
      billEmail: found?.email ?? "",
      ...(expectedEdited ? {} : { expectedEmail: found?.email ?? "" }),
    }));
  };

  /** Custom opens on the deadline the current preset would have given. */
  const chooseExpiry = (value: string) => {
    idempotencyKey.current = null;
    setFailure(null);
    setDraft((current) => ({
      ...current,
      expiry: value,
      ...(value === CUSTOM && !current.expiresAt
        ? { expiresAt: localMoment(new Date(openedAt + Number(current.expiry) * 3_600_000)) }
        : {}),
    }));
  };

  /** Switching identity carries the wallet choice to that identity's own. */
  const chooseIssuer = (next: Issuer) => {
    idempotencyKey.current = null;
    setFailure(null);
    setDraft((current) => ({
      ...current,
      issuerId: next.id,
      payoutAddressId: next.payout_addresses[0]?.id ?? "",
    }));
  };

  const go = (next: number, how: "forward" | "back") => {
    setDirection(how);
    setStep(next);
  };

  const advance = (event: FormEvent) => {
    event.preventDefault();
    if (answered) go(step + 1, "forward");
  };

  const issue = async () => {
    if (attachmentBusy) return;
    setBusy(true);
    setFailure(null);
    idempotencyKey.current ??= crypto.randomUUID();
    try {
      // A billed party typed fresh becomes a customer, so the next request can
      // pick them rather than retype them — and so they appear in the list.
      let customerId = draft.customerId;
      if (!customerId) {
        savedCustomer.current ??= (
          await client.customers.create({
            name: draft.billName.trim(),
            ...(draft.billEmail.trim() ? { email: draft.billEmail.trim() } : {}),
          })
        ).id;
        customerId = savedCustomer.current;
      }
      const payment = await client.payments.create(
        buildCreatePayment(
          {
            ...EMPTY_VALUES,
            amount: draft.amount,
            // The link that survives the identity being renamed; the party
            // below is the snapshot the document keeps.
            issuerId: draft.issuerId,
            customerId,
            // The invoice keeps its own snapshot of the identity, so a later
            // edit to it cannot reach an invoice already issued.
            payoutAddress: payout?.address ?? "",
            expiresInHours: draft.expiry === CUSTOM ? "" : draft.expiry,
            expiresAt: chosenMoment(draft),
            issuerName: issuer?.name ?? "",
            issuerEmail: issuer?.contact_email ?? "",
            issuerDetails: issuer?.details ?? "",
            billName: draft.billName,
            billEmail: draft.billEmail,
            heading: draft.heading,
            reference: draft.reference,
            notes: draft.notes,
            mode: draft.mode,
            expectedEmail: draft.expectedEmail,
            firstName: draft.firstName,
            lastName: draft.lastName,
          },
          attachment?.id ?? null,
        ),
        idempotencyKey.current,
      );
      onIssued(payment);
    } catch (cause) {
      if (cause instanceof PaydayError && cause.status === 401) {
        signOut();
        return;
      }
      setFailure(describeError(cause));
      setBusy(false);
    }
  };

  const gated = draft.mode !== "permissionless";
  const last = step === STEPS.length - 1;

  return (
    <div>
      <div className="flex flex-wrap items-start justify-between gap-4">
        <h1 className="font-heading text-[26px] leading-tight font-medium tracking-[-0.04em] sm:text-[30px]">
          New deposit request<span className="text-brand-yellow">.</span>
        </h1>
        <button
          type="button"
          onClick={onCancel}
          disabled={busy}
          className="rounded-md px-2 py-1.5 text-[13px] font-medium text-muted transition-colors hover:text-ink disabled:opacity-40"
        >
          Cancel
        </button>
      </div>

      <ol className="mt-7 flex gap-2.5">
        {STEPS.map((entry, index) => (
          <li key={entry} className="flex-1">
            <span className="block h-[3px] overflow-hidden rounded-full bg-line">
              <span
                className={cn(
                  "block h-full origin-left rounded-full bg-brand-green transition-transform duration-500 ease-out",
                  index <= step ? "scale-x-100" : "scale-x-0",
                )}
              />
            </span>
            <span
              className={cn(
                "mt-2 block text-[12px] font-medium transition-colors",
                index === step ? "text-ink" : "text-faint",
              )}
            >
              <span className="tabular">{index + 1}</span>
              <span className="ml-1.5 hidden sm:inline">{entry}</span>
            </span>
          </li>
        ))}
      </ol>

      <div className="mt-8 grid items-start gap-6 lg:grid-cols-[minmax(0,1fr)_300px]">
        <form onSubmit={last ? (event) => event.preventDefault() : advance}>
          <div key={step} data-direction={direction} className="dash-step">
            {step === 0 ? (
              <div className="dash-stagger grid gap-5">
                <Labeled
                  label="Amount"
                  error={shown("amount")}
                  hint="USDC, used exactly as written. There are no line items."
                >
                  <span className="relative block">
                    <input
                      autoFocus
                      inputMode="decimal"
                      placeholder="0.00"
                      value={draft.amount}
                      onChange={(event) => set("amount", event.target.value)}
                      aria-invalid={shown("amount") ? true : undefined}
                      className={cn(
                        controlStyles,
                        "tabular h-14 pr-[70px] text-[24px] font-medium tracking-tight",
                      )}
                    />
                    <span
                      aria-hidden="true"
                      className="absolute top-1/2 right-4 -translate-y-1/2 text-[13px] text-faint"
                    >
                      USDC
                    </span>
                  </span>
                </Labeled>

                {issuers.length > 1 ? (
                  <Choice
                    label="Issued by"
                    error={shown("issuerId")}
                    options={issuers.map((entry) => ({
                      id: entry.id,
                      title: entry.name,
                      detail: entry.contact_email,
                    }))}
                    selected={draft.issuerId}
                    onSelect={(id) => {
                      const next = issuers.find((entry) => entry.id === id);
                      if (next) chooseIssuer(next);
                    }}
                  />
                ) : null}

                {issuer && issuer.payout_addresses.length > 1 ? (
                  <Choice
                    label="Settles to"
                    error={shown("payoutAddressId")}
                    options={issuer.payout_addresses.map((entry) => ({
                      id: entry.id,
                      title: entry.label ?? truncateAddress(entry.address),
                      detail: entry.label ? truncateAddress(entry.address) : "",
                      mono: true,
                    }))}
                    selected={draft.payoutAddressId}
                    onSelect={(id) => set("payoutAddressId", id)}
                  />
                ) : null}

                <div>
                  <span className="block text-[12px] font-medium text-muted">Expires in</span>
                  <div
                    role="radiogroup"
                    aria-label="Expires in"
                    className="mt-1.5 inline-flex rounded-[10px] border border-line bg-surface p-1"
                  >
                    {EXPIRIES.map((option) => (
                      <button
                        key={option.value}
                        type="button"
                        role="radio"
                        aria-checked={draft.expiry === option.value}
                        onClick={() => chooseExpiry(option.value)}
                        className={cn(
                          "h-9 rounded-[7px] px-3.5 text-[13px] transition-colors",
                          draft.expiry === option.value
                            ? "bg-brand-green font-medium text-brand-black"
                            : "text-muted hover:text-ink",
                        )}
                      >
                        {option.label}
                      </button>
                    ))}
                  </div>

                  {draft.expiry === CUSTOM ? (
                    <div className="mt-3 max-w-[280px]">
                      <Labeled label="Date and time" error={errors.expiresAt}>
                        <input
                          type="datetime-local"
                          min={localMoment(new Date(openedAt + MIN_LEAD_MS))}
                          max={localMoment(new Date(openedAt + MAX_LEAD_MS))}
                          value={draft.expiresAt}
                          onChange={(event) => set("expiresAt", event.target.value)}
                          aria-invalid={errors.expiresAt ? true : undefined}
                          className={cn(controlStyles, "tabular h-11")}
                        />
                      </Labeled>
                    </div>
                  ) : null}

                  <p className="mt-1.5 text-[12px] text-faint">
                    After this the address stops being payable; late transfers are recovered.
                  </p>
                </div>
              </div>
            ) : null}

            {step === 1 ? (
              <div className="dash-stagger grid gap-5">
                {customers.length > 0 ? (
                  <MenuSelect
                    label="Customer"
                    className="max-w-[320px]"
                    value={draft.customerId}
                    onChange={chooseCustomer}
                    options={[
                      { value: "", label: "Someone new" },
                      ...customers.map((entry): MenuOption => ({
                        value: entry.id,
                        label: entry.name,
                      })),
                    ]}
                  />
                ) : null}

                <div className="grid gap-5 sm:grid-cols-2">
                  <Labeled label="Billed to" error={shown("billName")}>
                    <input
                      autoFocus
                      maxLength={255}
                      placeholder="Globex LLC"
                      value={draft.billName}
                      onChange={(event) => set("billName", event.target.value)}
                      aria-invalid={shown("billName") ? true : undefined}
                      className={cn(controlStyles, "h-11")}
                    />
                  </Labeled>
                  <Labeled
                    label="Their email"
                    error={shown("billEmail")}
                    hint={
                      draft.customerId
                        ? undefined
                        : "Saved as a customer, so the next request can pick them."
                    }
                  >
                    <input
                      type="email"
                      value={draft.billEmail}
                      onChange={(event) => set("billEmail", event.target.value)}
                      aria-invalid={shown("billEmail") ? true : undefined}
                      className={cn(controlStyles, "h-11")}
                    />
                  </Labeled>
                </div>

                <div className="grid gap-5 sm:grid-cols-2">
                  <Labeled
                    label="What it is for"
                    hint="Shown before verification on gated requests."
                  >
                    <input
                      maxLength={200}
                      placeholder="March retainer"
                      value={draft.heading}
                      onChange={(event) => set("heading", event.target.value)}
                      className={cn(controlStyles, "h-11")}
                    />
                  </Labeled>
                  <Labeled label="Reference" hint="Your own invoice number, if you keep one.">
                    <input
                      maxLength={128}
                      placeholder="INV-001"
                      value={draft.reference}
                      onChange={(event) => set("reference", event.target.value)}
                      className={cn(controlStyles, "h-11 font-mono text-[13px]")}
                    />
                  </Labeled>
                </div>

                <Labeled label="Notes" hint="Carried on the request and its PDF.">
                  <textarea
                    maxLength={4000}
                    value={draft.notes}
                    onChange={(event) => set("notes", event.target.value)}
                    className={cn(controlStyles, "min-h-20 py-2 leading-relaxed")}
                  />
                </Labeled>

                <AttachmentUpload
                  value={attachment}
                  onChange={setAttachment}
                  onBusyChange={setAttachmentBusy}
                />
              </div>
            ) : null}

            {step === 2 ? (
              <div className="dash-stagger grid gap-5">
                <div role="radiogroup" aria-label="Payer policy" className="grid gap-2.5">
                  {MODES.map((entry) => (
                    <label
                      key={entry.value}
                      className="flex cursor-pointer gap-3 rounded-[12px] border border-line bg-surface px-4 py-3.5 transition-colors hover:border-line-strong has-checked:border-brand-green/60 has-checked:bg-brand-green/[0.07]"
                    >
                      <input
                        type="radio"
                        name="mode"
                        value={entry.value}
                        checked={draft.mode === entry.value}
                        onChange={() => set("mode", entry.value)}
                        aria-label={entry.label}
                        className="mt-1 accent-[#a3d277]"
                      />
                      <span>
                        <span className="block text-[14px] font-medium">{entry.label}</span>
                        <span className="mt-0.5 block text-[13px] leading-relaxed text-muted">
                          {entry.description}
                        </span>
                      </span>
                    </label>
                  ))}
                </div>

                {gated ? (
                  <div className="grid gap-5">
                    <Labeled
                      label="Expected payer email"
                      required
                      error={expectedProblem}
                      hint="The payer sees only a masked hint of this until they prove it."
                    >
                      <input
                        type="email"
                        value={draft.expectedEmail}
                        onChange={(event) => set("expectedEmail", event.target.value)}
                        aria-invalid={expectedProblem ? true : undefined}
                        className={cn(controlStyles, "h-11")}
                      />
                    </Labeled>
                    {draft.mode === "verified_identity" ? (
                      <div className="grid gap-5 sm:grid-cols-2">
                        <Labeled label="Expected first name" error={shown("firstName")}>
                          <input
                            maxLength={255}
                            value={draft.firstName}
                            onChange={(event) => set("firstName", event.target.value)}
                            aria-invalid={shown("firstName") ? true : undefined}
                            className={cn(controlStyles, "h-11")}
                          />
                        </Labeled>
                        <Labeled label="Expected last name" error={shown("lastName")}>
                          <input
                            maxLength={255}
                            value={draft.lastName}
                            onChange={(event) => set("lastName", event.target.value)}
                            aria-invalid={shown("lastName") ? true : undefined}
                            className={cn(controlStyles, "h-11")}
                          />
                        </Labeled>
                      </div>
                    ) : null}
                  </div>
                ) : null}
              </div>
            ) : null}

            {step === 3 ? (
              <div className="grid gap-4">
                <dl
                  aria-label="Request summary"
                  className="grid gap-3 rounded-[12px] border border-line bg-surface px-4 py-4 text-[13.5px]"
                >
                  <Row label="Amount">
                    <span className="tabular">{formatDisplayAmount(draft.amount.trim())} USDC</span>
                  </Row>
                  <Row label="Issued by">{issuer?.name}</Row>
                  <Row label="Settles to">
                    <span className="font-mono text-[12.5px]">
                      {payout ? truncateAddress(payout.address) : ""}
                    </span>
                  </Row>
                  <Row label="Billed to">
                    {draft.billName.trim()}
                    {draft.billEmail.trim() ? (
                      <span className="text-muted"> · {draft.billEmail.trim()}</span>
                    ) : null}
                  </Row>
                  <Row label="Verification">
                    {/* The expected mailbox follows the billed party even
                        while the policy is open, so it is only part of this
                        request when a policy actually checks it. */}
                    {gated ? modeLabel(draft.mode) : "—"}
                    {gated && draft.expectedEmail.trim() ? (
                      <span className="text-muted"> · {draft.expectedEmail.trim()}</span>
                    ) : null}
                  </Row>
                  <Row label="Expires in">{expiryLabel(draft)}</Row>
                  {attachment ? <Row label="Attachment">{attachment.filename}</Row> : null}
                </dl>
                <p className="text-[13px] leading-relaxed text-muted">
                  Issued requests are immutable: the amount, the parties, and the policy are
                  committed to the address the payer sends to.
                </p>
                {failure ? (
                  <p role="alert" className="text-[13px] text-danger">
                    {failure}
                  </p>
                ) : null}
              </div>
            ) : null}
          </div>

          <div className="mt-8 flex items-center gap-3">
            {step > 0 ? (
              <Button
                type="button"
                variant="ghost"
                onClick={() => go(step - 1, "back")}
                disabled={busy}
              >
                <ArrowLeft className="size-4" />
                Back
              </Button>
            ) : null}
            <div className="flex-1" />
            {last ? (
              <Button type="button" onClick={issue} disabled={busy || attachmentBusy}>
                {busy ? <Loader2 className="size-4 animate-spin" /> : null}
                Issue deposit request
              </Button>
            ) : (
              <Button type="submit" disabled={!answered}>
                Continue
                <ArrowRight className="size-4" />
              </Button>
            )}
          </div>
        </form>

        <Preview
          draft={draft}
          issuerName={issuer?.name ?? ""}
          payoutAddress={payout?.address ?? ""}
          step={step}
          onEdit={(index) => go(index, "back")}
        />
      </div>
    </div>
  );
}

/**
 * The request as it stands, beside the question being asked. It doubles as the
 * review: by the last step every row is filled, and each one leads back to the
 * step that set it.
 */
function Preview({
  draft,
  issuerName,
  payoutAddress,
  step,
  onEdit,
}: {
  draft: Draft;
  issuerName: string;
  payoutAddress: string;
  step: number;
  onEdit: (step: number) => void;
}) {
  const amount = draft.amount.trim();
  const title = draft.heading.trim() || draft.reference.trim();

  return (
    <aside className="rounded-[16px] border border-line bg-surface p-5 lg:sticky lg:top-6">
      <p className="text-[11px] tracking-[0.12em] text-faint uppercase">Deposit request</p>
      <p className="mt-3 flex items-baseline gap-1.5">
        <span
          className={cn(
            "tabular font-heading text-[30px] leading-none font-medium tracking-[-0.03em] transition-colors",
            amount ? "text-ink" : "text-faint/50",
          )}
        >
          {amount && AMOUNT.test(amount) ? formatDisplayAmount(amount) : "0.00"}
        </span>
        <span className="text-[13px] text-faint">USDC</span>
      </p>
      {title ? <p className="mt-1.5 text-[13px] text-muted">{title}</p> : null}

      <dl className="mt-5 grid gap-3 border-t border-line pt-4 text-[13px]">
        <Line label="From" value={issuerName} at={0} step={step} onEdit={onEdit} />
        <Line label="To" value={draft.billName.trim()} at={1} step={step} onEdit={onEdit} />
        {/* Unset until a policy is chosen, and "—" says that better than a
            sentence claiming the open link is itself a kind of verification. */}
        <Line
          label="Verification"
          value={draft.mode === "permissionless" ? "" : modeLabel(draft.mode)}
          at={2}
          step={step}
          onEdit={onEdit}
        />
        <Line label="Expires in" value={expiryLabel(draft)} at={0} step={step} onEdit={onEdit} />
        <Line
          label="Settles to"
          value={payoutAddress ? truncateAddress(payoutAddress) : ""}
          mono
          at={0}
          step={step}
          onEdit={onEdit}
        />
      </dl>
    </aside>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-4">
      <dt className="text-faint">{label}</dt>
      <dd className="min-w-0 truncate text-right">{children}</dd>
    </div>
  );
}

/** A short list of saved things to pick one of; radios, not a dropdown. */
function Choice({
  label,
  error,
  options,
  selected,
  onSelect,
}: {
  label: string;
  error?: string | undefined;
  options: ReadonlyArray<{ id: string; title: string; detail?: string; mono?: boolean }>;
  selected: string;
  onSelect: (id: string) => void;
}) {
  return (
    <div>
      <span className="block text-[12px] font-medium text-muted">{label}</span>
      <div role="radiogroup" aria-label={label} className="mt-1.5 grid gap-2 sm:grid-cols-2">
        {options.map((option) => (
          <label
            key={option.id}
            className="flex cursor-pointer items-center gap-2.5 rounded-[10px] border border-line bg-surface px-3.5 py-2.5 transition-colors hover:border-line-strong has-checked:border-brand-green/60 has-checked:bg-brand-green/[0.07]"
          >
            <input
              type="radio"
              name={label}
              value={option.id}
              checked={selected === option.id}
              onChange={() => onSelect(option.id)}
              aria-label={option.title}
              className="accent-[#a3d277]"
            />
            <span className="min-w-0">
              <span
                className={cn(
                  "block truncate text-[13.5px] font-medium",
                  option.mono ? "font-mono text-[12.5px]" : "",
                )}
              >
                {option.title}
              </span>
              {option.detail ? (
                <span className="block truncate font-mono text-[11.5px] text-faint">
                  {option.detail}
                </span>
              ) : null}
            </span>
          </label>
        ))}
      </div>
      {error ? (
        <p role="alert" className="mt-1.5 text-[12px] text-danger">
          {error}
        </p>
      ) : null}
    </div>
  );
}

function Line({
  label,
  value,
  mono,
  at,
  step,
  onEdit,
}: {
  label: string;
  value: string;
  mono?: boolean;
  at: number;
  step: number;
  onEdit: (step: number) => void;
}) {
  return (
    <div className="flex items-baseline justify-between gap-3">
      <dt className="text-faint">{label}</dt>
      <dd
        className={cn(
          "min-w-0 truncate text-right",
          mono ? "font-mono text-[12px]" : "",
          value ? "text-ink" : "text-faint/50",
        )}
      >
        {value ? (
          // Once a step has been passed, its rows are the way back to it.
          step > at ? (
            <button
              type="button"
              onClick={() => onEdit(at)}
              className="rounded-sm underline decoration-transparent underline-offset-2 transition-colors hover:decoration-current"
            >
              {value}
            </button>
          ) : (
            value
          )
        ) : (
          "—"
        )}
      </dd>
    </div>
  );
}
