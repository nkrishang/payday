"use client";

import { type Issuer, PaydayError } from "@payday/sdk";
import { Check, ChevronRight, Loader2, Plus, X } from "lucide-react";
import { useState, type FormEvent } from "react";
import { AddButton } from "@/components/ui/add-button";
import { Button } from "@/components/ui/button";
import { controlStyles } from "@/components/ui/field";
import { describeError } from "@/lib/attachment-upload";
import { cn } from "@/lib/cn";
import { truncateAddress } from "@/lib/format";
import { duplicateName, EMAIL, LABEL, PAYOUT_ADDRESS } from "./field-rules";
import { Labeled } from "./labeled";
import { useMerchant } from "./session";

/**
 * Managing issuer identities, on the dashboard beside everything else.
 *
 * A line each, so a merchant with several reads the list rather than scrolls
 * it: the name, whether it can be issued under, its contact address, and
 * whether it has saved wallets of its own. Opening a line gives the rest and
 * the actions — rename it, move its contact address (which unproves it,
 * because a different mailbox is a different claim), prove the one it has,
 * and attach or drop saved wallets. Deposits settle to the account's own
 * Payday wallet unless a saved one is chosen on the request, so an identity
 * needs none to be issued under. Nothing here can reach a deposit request already
 * issued; those carry their own snapshot.
 */

export function IssuerManager({
  identities,
  onChanged,
  onAdd,
}: {
  identities: Issuer[];
  /** Reloads the list after any change lands. */
  onChanged: () => void;
  onAdd: () => void;
}) {
  return (
    // Named, so the section is a landmark of its own: the word "Verified"
    // means something different in the table above it.
    <section aria-label="Issuer identities">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h2 className="font-heading text-[24px] leading-tight font-medium tracking-[-0.04em]">
            Issuer identities<span className="text-brand-yellow">.</span>
          </h2>
          <p className="mt-1.5 text-[13px] text-muted">
            Manage the identity used to issue deposit requests. By default, deposits settle to your
            Payday wallet, unless you override this for an identity.
          </p>
        </div>
        <AddButton label="New issuer identity" onClick={onAdd} />
      </div>

      <ul className="mt-6 grid gap-2">
        {identities.map((issuer) => (
          <li key={issuer.id}>
            <IssuerRow issuer={issuer} identities={identities} onChanged={onChanged} />
          </li>
        ))}
      </ul>
    </section>
  );
}

/** A wallet on the staged list: one already saved, or one about to be. */
type StagedWallet =
  | { kind: "saved"; id: string; address: string; label: string | null }
  | { kind: "new"; key: string; address: string; label: string | null };

function staged(issuer: Issuer): StagedWallet[] {
  return issuer.payout_addresses.map((entry) => ({
    kind: "saved",
    id: entry.id,
    address: entry.address,
    label: entry.label,
  }));
}

/** What the server holds, which "changed" is measured against. */
interface Baseline {
  name: string;
  contactEmail: string;
  wallets: StagedWallet[];
  emailVerified: boolean;
}

function baselineOf(issuer: Issuer): Baseline {
  return {
    name: issuer.name,
    contactEmail: issuer.contact_email,
    wallets: staged(issuer),
    emailVerified: issuer.email_verified,
  };
}

const walletKey = (list: StagedWallet[]) =>
  list.map((entry) => (entry.kind === "saved" ? entry.id : `new:${entry.address}`)).join(",");

/** Identifies one staged entry across a re-render. */
const keyOf = (entry: StagedWallet) => (entry.kind === "saved" ? entry.id : entry.key);

const signature = (state: Baseline) =>
  `${state.name}\u0000${state.contactEmail}\u0000${state.emailVerified}\u0000${walletKey(state.wallets)}`;

function IssuerRow({
  issuer,
  identities,
  onChanged,
}: {
  issuer: Issuer;
  /** The rest of the list, so a rename cannot land on a name already taken. */
  identities: Issuer[];
  onChanged: () => void;
}) {
  const { client, signOut } = useMerchant();
  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);

  // What the server last told us. Measuring against this rather than against
  // the prop is what lets a save settle: the API's own answer becomes the new
  // baseline immediately, so a wallet that was just created stops reading as
  // an unsaved edit.
  const [baseline, setBaseline] = useState<Baseline>(() => baselineOf(issuer));
  const [name, setName] = useState(issuer.name);
  const [contactEmail, setContactEmail] = useState(issuer.contact_email);
  const [wallets, setWallets] = useState<StagedWallet[]>(() => staged(issuer));
  // The wallet being added, held here until Save so one button commits the
  // whole row rather than each edit landing on its own.
  const [adding, setAdding] = useState(false);
  const [address, setAddress] = useState("");
  const [label, setLabel] = useState("");
  const [verifying, setVerifying] = useState(false);
  const [otp, setOtp] = useState("");

  // Checked as they are typed, so a button never leads to a refusal that was
  // knowable before it was pressed.
  const nameError = !name.trim()
    ? "Required."
    : duplicateName(name, identities, issuer.id)
      ? "Another identity already uses this name."
      : null;
  const emailError = !contactEmail.trim()
    ? "Required."
    : EMAIL.test(contactEmail.trim())
      ? null
      : "Not a valid email address.";
  const addressError = !address.trim()
    ? "Required."
    : PAYOUT_ADDRESS.test(address.trim())
      ? null
      : "Not a valid address: 0x and 40 hex characters.";
  const labelError = !label.trim() || LABEL.test(label.trim()) ? null : "Not a valid label.";

  // Save is lit only by an actual difference from what is stored.
  const changed =
    name.trim() !== baseline.name ||
    contactEmail.trim() !== baseline.contactEmail ||
    walletKey(wallets) !== walletKey(baseline.wallets);
  const valid = nameError === null && emailError === null;

  /** Takes the server's answer as the new truth, editing state included. */
  const adopt = (next: Issuer) => {
    setBaseline(baselineOf(next));
    setName(next.name);
    setContactEmail(next.contact_email);
    setWallets(staged(next));
  };

  // Fresh data from the list — a reload, or a change made elsewhere — is
  // adopted whenever nothing here is unsaved, so the row never shows something
  // the server has moved past.
  if (!changed && signature(baselineOf(issuer)) !== signature(baseline)) {
    adopt(issuer);
  }

  const reset = () => {
    adopt(issuer);
    setAdding(false);
    setAddress("");
    setLabel("");
    setVerifying(false);
    setOtp("");
    setFailure(null);
  };

  const failed = (cause: unknown) => {
    if (cause instanceof PaydayError && cause.status === 401 && cause.code === "unauthorized") {
      signOut();
      return;
    }
    setFailure(describeError(cause));
    setBusy(false);
  };

  /** Commits the row: the party first, then the wallets it settles to. */
  const save = async (event: FormEvent) => {
    event.preventDefault();
    if (!changed || !valid) return;
    setBusy(true);
    setFailure(null);
    try {
      let latest = issuer;
      if (name.trim() !== baseline.name || contactEmail.trim() !== baseline.contactEmail) {
        latest = await client.issuers.update(issuer.id, {
          name: name.trim(),
          contact_email: contactEmail.trim(),
        });
      }
      if (walletKey(wallets) !== walletKey(baseline.wallets)) {
        const ids: string[] = [];
        for (const entry of wallets) {
          if (entry.kind === "saved") {
            ids.push(entry.id);
            continue;
          }
          const created = await client.payoutAddresses.create({
            address: entry.address,
            ...(entry.label ? { label: entry.label } : {}),
          });
          ids.push(created.id);
        }
        latest = await client.issuers.setPayoutAddresses(issuer.id, ids);
      }
      adopt(latest);
      setBusy(false);
      setAdding(false);
      setAddress("");
      setLabel("");
      onChanged();
    } catch (cause) {
      failed(cause);
    }
  };

  const sendCode = async () => {
    setBusy(true);
    setFailure(null);
    try {
      await client.issuers.startEmailVerification(issuer.id);
      setVerifying(true);
      setBusy(false);
    } catch (cause) {
      failed(cause);
    }
  };

  const confirm = async () => {
    setBusy(true);
    setFailure(null);
    try {
      adopt(await client.issuers.confirmEmailVerification(issuer.id, otp.trim()));
      setVerifying(false);
      setOtp("");
      setBusy(false);
      onChanged();
    } catch (cause) {
      if (cause instanceof PaydayError && cause.code === "otp_invalid") {
        setFailure("That code is not valid. Check the email, or send a new one.");
        setBusy(false);
        return;
      }
      failed(cause);
    }
  };

  const stageWallet = () => {
    if (addressError || labelError) return;
    const entry: StagedWallet = {
      kind: "new",
      key: crypto.randomUUID(),
      address: address.trim(),
      label: label.trim() || null,
    };
    setWallets((current) => [...current, entry]);
    setAdding(false);
    setAddress("");
    setLabel("");
  };

  /** Dropping a saved wallet leaves the identity on the account's own. */
  const dropWallet = (entry: StagedWallet) => {
    setFailure(null);
    setWallets((current) => current.filter((other) => other !== entry));
  };

  const count = issuer.payout_addresses.length;

  return (
    <div
      className={cn(
        "overflow-hidden rounded-[12px] border bg-surface transition-colors",
        open ? "border-line-strong" : "border-line",
      )}
    >
      <button
        type="button"
        aria-expanded={open}
        onClick={() => {
          if (open) reset();
          setOpen((current) => !current);
        }}
        className="flex w-full items-center gap-3 px-4 py-3 text-left transition-colors hover:bg-raised"
      >
        <ChevronRight
          aria-hidden="true"
          className={cn(
            "size-4 shrink-0 text-faint transition-transform duration-200",
            open && "rotate-90",
          )}
        />
        <span className="truncate text-[14px] font-medium">{issuer.name}</span>
        {issuer.email_verified ? (
          <span className="inline-flex shrink-0 items-center gap-1 rounded-full border border-brand-green/40 px-2 py-0.5 text-[11px] text-brand-green">
            <Check className="size-3" />
            Verified
          </span>
        ) : (
          <span className="shrink-0 rounded-full border border-warning/40 px-2 py-0.5 text-[11px] text-warning">
            Unverified
          </span>
        )}
        <span className="ml-auto hidden truncate text-[13px] text-muted sm:block">
          {issuer.contact_email}
        </span>
        <span className="tabular shrink-0 text-[12px] text-faint">
          {count === 0 ? "Payday wallet" : `${count} saved wallet${count === 1 ? "" : "s"}`}
        </span>
      </button>

      {/* Kept in the tree so its height can animate, and inert while closed so
          nothing inside it is reachable by tab or by a screen reader. */}
      <div className="dash-expand" data-open={open}>
        <div inert={!open}>
          <form onSubmit={save} className="border-t border-line px-4 pt-4 pb-5">
            <div className="flex flex-wrap items-center gap-2">
              {wallets.length === 0 && !adding ? (
                <span className="text-[12px] text-muted">
                  Settles to your Payday wallet. Add a saved wallet to offer another destination.
                </span>
              ) : null}
              {wallets.map((entry) => {
                const name = entry.label ?? truncateAddress(entry.address);
                return (
                  <span
                    key={keyOf(entry)}
                    className={cn(
                      "inline-flex items-center gap-2 rounded-[8px] border py-1.5 pr-1.5 pl-3 text-[12px] transition-opacity",
                      entry.kind === "new"
                        ? "border-brand-green/40 bg-brand-green/[0.07]"
                        : "border-line bg-raised",
                    )}
                  >
                    {entry.label ? <span className="text-ink">{entry.label}</span> : null}
                    <span className="font-mono text-faint">{truncateAddress(entry.address)}</span>
                    <button
                      type="button"
                      onClick={() => dropWallet(entry)}
                      disabled={busy}
                      aria-label={`Remove ${name}`}
                      className="rounded p-0.5 text-faint transition-colors hover:text-ink disabled:opacity-40"
                    >
                      <X className="size-3" />
                    </button>
                  </span>
                );
              })}
              {adding ? null : (
                <button
                  type="button"
                  onClick={() => setAdding(true)}
                  className="inline-flex items-center gap-1.5 rounded-[8px] border border-dashed border-line-strong px-3 py-1.5 text-[12px] text-muted transition-colors hover:text-ink"
                >
                  <Plus className="size-3" />
                  Add wallet
                </button>
              )}
            </div>

            {adding ? (
              <div className="dash-step mt-4">
                <div className="grid gap-4 sm:grid-cols-[minmax(0,1fr)_180px_auto]">
                  <Labeled
                    label="Payout address"
                    error={address ? (addressError ?? undefined) : undefined}
                  >
                    <input
                      autoFocus
                      spellCheck={false}
                      placeholder="0x…"
                      value={address}
                      onChange={(event) => setAddress(event.target.value)}
                      className={cn(controlStyles, "h-11 font-mono text-[13px]")}
                    />
                  </Labeled>
                  <Labeled label="Label" error={labelError ?? undefined}>
                    <input
                      maxLength={20}
                      placeholder="Treasury"
                      value={label}
                      onChange={(event) => setLabel(event.target.value)}
                      className={cn(controlStyles, "h-11")}
                    />
                  </Labeled>
                  <div className="flex items-end gap-2">
                    <Button
                      type="button"
                      variant="secondary"
                      onClick={stageWallet}
                      disabled={addressError !== null || labelError !== null}
                    >
                      Add
                    </Button>
                    <Button
                      type="button"
                      variant="ghost"
                      onClick={() => {
                        setAdding(false);
                        setAddress("");
                        setLabel("");
                      }}
                    >
                      Cancel
                    </Button>
                  </div>
                </div>
              </div>
            ) : null}

            <div className="mt-5 grid gap-4 border-t border-line pt-5 sm:grid-cols-2">
              <Labeled label="Issued by" error={name ? (nameError ?? undefined) : undefined}>
                <input
                  maxLength={255}
                  value={name}
                  onChange={(event) => setName(event.target.value)}
                  className={cn(controlStyles, "h-11")}
                />
              </Labeled>
              <Labeled
                label="Contact address"
                error={contactEmail ? (emailError ?? undefined) : undefined}
                hint="A changed address must be confirmed again before it goes on a request."
              >
                <input
                  type="email"
                  value={contactEmail}
                  onChange={(event) => setContactEmail(event.target.value)}
                  className={cn(controlStyles, "h-11")}
                />
              </Labeled>
            </div>

            {verifying ? (
              <div className="dash-step mt-4 flex flex-wrap items-end gap-3">
                <Labeled label="One-time code" hint={`Sent to ${issuer.contact_email}.`}>
                  <input
                    autoFocus
                    inputMode="numeric"
                    autoComplete="one-time-code"
                    maxLength={6}
                    placeholder="000000"
                    value={otp}
                    onChange={(event) => setOtp(event.target.value.replace(/\D/g, "").slice(0, 6))}
                    className={cn(
                      controlStyles,
                      "h-11 w-[180px] text-center font-mono text-[16px] tracking-[0.35em] [text-indent:0.35em]",
                    )}
                  />
                </Labeled>
                <Button type="button" onClick={confirm} disabled={busy || otp.length < 6}>
                  {busy ? <Loader2 className="size-4 animate-spin" /> : null}
                  Confirm code
                </Button>
              </div>
            ) : null}

            <Problem>{failure}</Problem>

            <div className="mt-5 flex flex-wrap items-center justify-end gap-3">
              {issuer.email_verified || verifying ? null : (
                <Button type="button" variant="secondary" onClick={sendCode} disabled={busy}>
                  {busy ? <Loader2 className="size-4 animate-spin" /> : null}
                  Verify email
                </Button>
              )}
              <Button type="button" variant="ghost" onClick={reset} disabled={busy || !changed}>
                Cancel
              </Button>
              <Button type="submit" disabled={busy || !changed || !valid}>
                {busy ? <Loader2 className="size-4 animate-spin" /> : null}
                Save
              </Button>
            </div>
          </form>
        </div>
      </div>
    </div>
  );
}

function Problem({ children }: { children: React.ReactNode }) {
  if (!children) return null;
  return (
    <p role="alert" className="text-[13px] text-danger">
      {children}
    </p>
  );
}
