"use client";

import { type Customer, PaydayError } from "@payday/sdk";
import { Loader2 } from "lucide-react";
import { useRouter } from "next/navigation";
import { useState, type FormEvent } from "react";
import { Button } from "@/components/ui/button";
import { Field, Input, Problem, Textarea } from "@/components/ui/field";
import { describeError } from "@/lib/attachment-upload";
import { useMerchant } from "./session";

/**
 * Creates a customer, or replaces an existing one's editable fields — the API
 * treats PATCH as a full replacement, so an emptied field clears.
 */
export function CustomerForm({
  customer,
  onSaved,
}: {
  customer?: Customer;
  onSaved?: (customer: Customer) => void;
}) {
  const router = useRouter();
  const { client, signOut } = useMerchant();
  const [name, setName] = useState(customer?.name ?? "");
  const [email, setEmail] = useState(customer?.email ?? "");
  const [details, setDetails] = useState(customer?.details ?? "");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    setSaved(false);
    try {
      if (customer) {
        const updated = await client.customers.update(customer.id, {
          name: name.trim(),
          email: email.trim() || null,
          details: details.trim() || null,
        });
        setSaved(true);
        onSaved?.(updated);
      } else {
        const created = await client.customers.create({
          name: name.trim(),
          ...(email.trim() ? { email: email.trim() } : {}),
          ...(details.trim() ? { details: details.trim() } : {}),
        });
        router.push(`/dashboard/customers/${encodeURIComponent(created.id)}`);
        return;
      }
    } catch (cause) {
      if (cause instanceof PaydayError && cause.status === 401) {
        signOut();
        return;
      }
      setError(describeError(cause));
    }
    setBusy(false);
  };

  return (
    <form onSubmit={submit} className="grid gap-4 rounded-[16px] border border-line bg-surface p-5">
      <Field label="Name">
        <Input
          required
          maxLength={255}
          value={name}
          onChange={(event) => setName(event.target.value)}
        />
      </Field>
      <Field label="Email">
        <Input type="email" value={email} onChange={(event) => setEmail(event.target.value)} />
      </Field>
      <Field label="Details" hint="Free text shown verbatim on invoices billed to this customer.">
        <Textarea
          maxLength={4000}
          value={details}
          onChange={(event) => setDetails(event.target.value)}
        />
      </Field>
      <Problem>{error}</Problem>
      <div className="flex items-center gap-3">
        <Button type="submit" size="sm" disabled={busy}>
          {busy ? <Loader2 className="size-4 animate-spin" /> : null}
          {customer ? "Save changes" : "Create customer"}
        </Button>
        {saved ? (
          <span role="status" className="text-[13px] text-success">
            Saved
          </span>
        ) : null}
      </div>
    </form>
  );
}
