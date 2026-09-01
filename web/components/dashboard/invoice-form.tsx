"use client";

import {
  type AttachmentDescriptor,
  type CreatePayment,
  type Customer,
  type Party,
  type PayerPolicy,
  type PayerPolicyMode,
  PaydayError,
} from "@payday/sdk";
import { Loader2 } from "lucide-react";
import { useRouter, useSearchParams } from "next/navigation";
import { useCallback, useId, useRef, useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Field, Fieldset, Input, Problem, Select, Textarea } from "@/components/ui/field";
import { describeError } from "@/lib/attachment-upload";
import { AttachmentUpload } from "./attachment-upload";
import { MODES } from "./labels";
import { useMerchant, useResource } from "./session";

export interface InvoiceFormValues {
  issuerName: string;
  issuerEmail: string;
  issuerDetails: string;
  customerId: string;
  billName: string;
  billEmail: string;
  billDetails: string;
  amount: string;
  payoutAddress: string;
  /** Hours; empty means the API's default deadline. */
  expiresInHours: string;
  heading: string;
  reference: string;
  notes: string;
  mode: PayerPolicyMode;
  expectedEmail: string;
  firstName: string;
  lastName: string;
}

export const EMPTY_VALUES: InvoiceFormValues = {
  issuerName: "",
  issuerEmail: "",
  issuerDetails: "",
  customerId: "",
  billName: "",
  billEmail: "",
  billDetails: "",
  amount: "",
  payoutAddress: "",
  expiresInHours: "",
  heading: "",
  reference: "",
  notes: "",
  mode: "permissionless",
  expectedEmail: "",
  firstName: "",
  lastName: "",
};

function party(name: string, email: string, details: string): Party {
  return {
    name: name.trim(),
    ...(email.trim() ? { email: email.trim() } : {}),
    ...(details.trim() ? { details: details.trim() } : {}),
  };
}

function policy(values: InvoiceFormValues): PayerPolicy {
  const expected_email = values.expectedEmail.trim();
  switch (values.mode) {
    case "permissionless":
      return { mode: "permissionless" };
    case "verified_email":
      return { mode: "verified_email", expected_email };
    case "verified_identity":
      return {
        mode: "verified_identity",
        expected_email,
        expected_identity: {
          first_name: values.firstName.trim(),
          last_name: values.lastName.trim(),
        },
      };
    case "verified_identity_unattributed":
      return { mode: "verified_identity_unattributed", expected_email };
  }
}

/**
 * The request body, exactly as the SDK sends it. Only the shape is built here;
 * limits and policy rules are the API's to enforce, and its message is shown
 * verbatim when it refuses.
 */
export function buildCreatePayment(
  values: InvoiceFormValues,
  attachmentId: string | null,
): CreatePayment {
  const optional = (value: string) => (value.trim() ? { value: value.trim() } : null);
  const heading = optional(values.heading);
  const reference = optional(values.reference);
  const notes = optional(values.notes);
  const hours = Number(values.expiresInHours);
  const expiresIn =
    values.expiresInHours.trim() && Number.isFinite(hours) && hours > 0
      ? Math.round(hours * 3600)
      : null;

  return {
    amount: values.amount.trim(),
    payout_address: values.payoutAddress.trim(),
    issuer: party(values.issuerName, values.issuerEmail, values.issuerDetails),
    bill_to: party(values.billName, values.billEmail, values.billDetails),
    payer_policy: policy(values),
    ...(values.customerId ? { customer_id: values.customerId } : {}),
    ...(heading ? { heading: heading.value } : {}),
    ...(reference ? { reference: reference.value } : {}),
    ...(notes ? { notes: notes.value } : {}),
    ...(attachmentId ? { attachment_id: attachmentId } : {}),
    ...(expiresIn ? { expires_in: expiresIn } : {}),
  };
}

export function InvoiceForm() {
  const router = useRouter();
  const search = useSearchParams();
  const { client, signOut } = useMerchant();
  const customers = useResource("customers", (api) => api.customers.list({ limit: 100 }));

  const modeHelpId = useId();
  const [values, setValues] = useState<InvoiceFormValues>(EMPTY_VALUES);
  const [attachment, setAttachment] = useState<AttachmentDescriptor | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // One key per distinct submission: a retry of the same values replays, and
  // any edit starts a new key so the API never sees a conflict of our making.
  const idempotencyKey = useRef<string | null>(null);

  const update = useCallback(
    <K extends keyof InvoiceFormValues>(key: K, value: InvoiceFormValues[K]) => {
      idempotencyKey.current = null;
      setValues((current) => ({ ...current, [key]: value }));
    },
    [],
  );

  const prefill = useCallback((customer: Customer | null) => {
    setValues((current) => ({
      ...current,
      customerId: customer?.id ?? "",
      ...(customer
        ? {
            billName: customer.name,
            billEmail: customer.email ?? "",
            billDetails: customer.details ?? "",
          }
        : {}),
    }));
  }, []);

  // A customer's page links here with `?customer=`; once the list arrives the
  // match is applied once, during render, the way a derived default is.
  const preselect = search.get("customer");
  const [preselected, setPreselected] = useState<string | null>(null);
  const match =
    preselect && preselected !== preselect
      ? customers.data?.customers.find((customer) => customer.id === preselect)
      : undefined;
  if (match) {
    setPreselected(preselect);
    prefill(match);
  }

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    idempotencyKey.current ??= crypto.randomUUID();
    try {
      const payment = await client.payments.create(
        buildCreatePayment(values, attachment?.id ?? null),
        idempotencyKey.current,
      );
      router.push(`/dashboard/invoices/${encodeURIComponent(payment.id)}`);
    } catch (cause) {
      if (cause instanceof PaydayError && cause.status === 401) {
        signOut();
        return;
      }
      setError(describeError(cause));
      setBusy(false);
    }
  };

  const gated = values.mode !== "permissionless";

  return (
    <form onSubmit={submit} className="grid gap-5">
      <div className="grid gap-5 lg:grid-cols-2">
        <Fieldset legend="Issuer" description="Who this invoice is from.">
          <Field label="Issuer name">
            <Input
              required
              maxLength={255}
              value={values.issuerName}
              onChange={(event) => update("issuerName", event.target.value)}
            />
          </Field>
          <Field label="Issuer email">
            <Input
              type="email"
              value={values.issuerEmail}
              onChange={(event) => update("issuerEmail", event.target.value)}
            />
          </Field>
          <Field
            label="Issuer details"
            hint="Free text shown verbatim: address, registration, contact."
          >
            <Textarea
              maxLength={4000}
              value={values.issuerDetails}
              onChange={(event) => update("issuerDetails", event.target.value)}
            />
          </Field>
        </Fieldset>

        <Fieldset
          legend="Bill to"
          description="The invoiced counterparty. A customer pre-fills it."
        >
          <Field label="Customer">
            <Select
              value={values.customerId}
              onChange={(event) => {
                idempotencyKey.current = null;
                prefill(
                  customers.data?.customers.find(
                    (customer) => customer.id === event.target.value,
                  ) ?? null,
                );
              }}
            >
              <option value="">No customer</option>
              {customers.data?.customers.map((customer) => (
                <option key={customer.id} value={customer.id}>
                  {customer.name}
                </option>
              ))}
            </Select>
          </Field>
          <Field label="Bill to name">
            <Input
              required
              maxLength={255}
              value={values.billName}
              onChange={(event) => update("billName", event.target.value)}
            />
          </Field>
          <Field label="Bill to email">
            <Input
              type="email"
              value={values.billEmail}
              onChange={(event) => update("billEmail", event.target.value)}
            />
          </Field>
          <Field label="Bill to details">
            <Textarea
              maxLength={4000}
              value={values.billDetails}
              onChange={(event) => update("billDetails", event.target.value)}
            />
          </Field>
        </Fieldset>
      </div>

      <Fieldset
        legend="Invoice"
        description="The amount is used directly; there are no line items."
      >
        <div className="grid gap-4 sm:grid-cols-3">
          <Field label="Amount (USDC)">
            <Input
              required
              inputMode="decimal"
              placeholder="25.00"
              value={values.amount}
              onChange={(event) => update("amount", event.target.value)}
            />
          </Field>
          <Field label="Payout address" className="sm:col-span-2">
            <Input
              required
              placeholder="0x…"
              className="font-mono"
              value={values.payoutAddress}
              onChange={(event) => update("payoutAddress", event.target.value)}
            />
          </Field>
        </div>
        <div className="grid gap-4 sm:grid-cols-3">
          <Field
            label="Heading"
            hint="Shown before verification on gated invoices."
            className="sm:col-span-2"
          >
            <Input
              maxLength={200}
              value={values.heading}
              onChange={(event) => update("heading", event.target.value)}
            />
          </Field>
          <Field label="Expires in (hours)" hint="Blank uses the default deadline.">
            <Input
              inputMode="numeric"
              value={values.expiresInHours}
              onChange={(event) => update("expiresInHours", event.target.value)}
            />
          </Field>
        </div>
        <Field label="Reference" hint="Your invoice number or external reference.">
          <Input
            maxLength={128}
            value={values.reference}
            onChange={(event) => update("reference", event.target.value)}
          />
        </Field>
        <Field label="Notes">
          <Textarea
            maxLength={4000}
            value={values.notes}
            onChange={(event) => update("notes", event.target.value)}
          />
        </Field>
        <AttachmentUpload onChange={setAttachment} />
      </Fieldset>

      <Fieldset
        legend="Payer policy"
        description="Who may pay, and what they must prove first. Fixed once the invoice is issued."
      >
        <div role="radiogroup" aria-label="Payer policy mode" className="grid gap-2">
          {MODES.map((entry) => (
            <label
              key={entry.value}
              className="flex cursor-pointer gap-3 rounded-[10px] border border-line px-3.5 py-3 has-checked:border-line-strong has-checked:bg-raised"
            >
              <input
                type="radio"
                name="mode"
                value={entry.value}
                checked={values.mode === entry.value}
                onChange={() => update("mode", entry.value)}
                // The name is the mode alone; the sentence under it describes.
                aria-label={entry.label}
                aria-describedby={`${modeHelpId}-${entry.value}`}
                className="mt-1 accent-ink"
              />
              <span>
                <span className="block text-[14px] font-medium">{entry.label}</span>
                <span id={`${modeHelpId}-${entry.value}`} className="block text-[13px] text-muted">
                  {entry.description}
                </span>
              </span>
            </label>
          ))}
        </div>

        {gated ? (
          <Field
            label="Expected payer email"
            hint="The payer sees only a masked hint of this address until they verify it."
          >
            <Input
              type="email"
              required
              value={values.expectedEmail}
              onChange={(event) => update("expectedEmail", event.target.value)}
            />
          </Field>
        ) : null}

        {values.mode === "verified_identity" ? (
          <div className="grid gap-4 sm:grid-cols-2">
            <Field label="Expected first name">
              <Input
                required
                maxLength={255}
                value={values.firstName}
                onChange={(event) => update("firstName", event.target.value)}
              />
            </Field>
            <Field label="Expected last name">
              <Input
                required
                maxLength={255}
                value={values.lastName}
                onChange={(event) => update("lastName", event.target.value)}
              />
            </Field>
          </div>
        ) : null}
      </Fieldset>

      <Problem>{error}</Problem>

      <div className="flex items-center justify-end gap-3">
        <Button type="submit" disabled={busy}>
          {busy ? <Loader2 className="size-4 animate-spin" /> : null}
          Issue invoice
        </Button>
      </div>
    </form>
  );
}
