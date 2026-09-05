"use client";

import type { AccountMetadata, Customer, Issuer, Payment } from "@payday/sdk";
import { ArrowUpRight } from "lucide-react";
import Image from "next/image";
import { useRouter, useSearchParams } from "next/navigation";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { CopyButton } from "@/components/ui/copy-button";
import { Problem } from "@/components/ui/field";
import { cn } from "@/lib/cn";
import { formatDisplayAmount } from "@/lib/format";
import { AccountSection } from "./account-section";
import { ApiKeySection } from "./api-key-manager";
import { CustomerTable } from "./customer-table";
import { InvoiceTable } from "./invoice-table";
import { IssuerManager } from "./issuer-manager";
import { IssuerSetup } from "./issuer-setup";
import { OnboardingSuccess } from "./onboarding-success";
import { OnboardingWalkthrough } from "./onboarding-walkthrough";
import { RequestComposer } from "./request-composer";
import { useResource } from "./session";

/**
 * The dashboard.
 *
 * Everything a merchant does day to day is on this one page: the deposit
 * requests, the identities they are issued under, the customers they are
 * billed to, and the flow that issues a new one. The only other dashboard
 * pages are the detail of one record.
 *
 * A new account is not shown a description of the product — it is put to work
 * on the first thing the product needs from it. An identity must exist, and
 * its contact mailbox must be proven, before a request can carry it; so with
 * nothing set up the page *is* that flow, and issuing follows straight out of
 * it. Where a request settles needs no setting up: every account has its own
 * wallet from the moment it signs in, and that is the default.
 */

type View = "overview" | "setup" | "compose" | "issued" | "onboarding";

/** Usable on an invoice once its mailbox is proven. */
function usable(issuer: Issuer): boolean {
  return issuer.email_verified;
}

export function DashboardHome() {
  const router = useRouter();
  const search = useSearchParams();
  // The account itself: the wallet a request settles to by default, and the
  // key state the sections at the foot of the page manage.
  const account = useResource("account", (client) => client.account.get());
  const issuers = useResource("issuers", (client) => client.issuers.list({ limit: 50 }));
  // Only ever asks whether anything exists: the table below loads its own page
  // with its own filter and cursor.
  const anything = useResource("home", (client) => client.payments.list({ limit: 1 }));
  // The filters and the composer both pick from these; loaded once, here.
  const customers = useResource("customers", (client) => client.customers.list({ limit: 100 }));

  const [view, setView] = useState<View>("overview");
  const [leavingTo, setLeavingTo] = useState<View | null>(null);
  const [issued, setIssued] = useState<Payment | null>(null);
  // The identity that just finished onboarding, and the one real deposit
  // request the walkthrough issues under it. `onboardingPayment` becoming
  // non-null is what actually switches the walkthrough over to its success
  // screen — `view` alone does not, so a stale re-render can never hijack it.
  const [onboardingIssuer, setOnboardingIssuer] = useState<Issuer | null>(null);
  const [onboardingPayment, setOnboardingPayment] = useState<Payment | null>(null);
  // A customer's page links here to bill them; the composer opens on them.
  const [billed, setBilled] = useState<string | null>(null);
  // A request to open once the overview comes back, so "Track this request"
  // lands on the request itself rather than on the top of the list.
  const [tracking, setTracking] = useState<string | null>(null);

  // One view leaves before the next arrives; the swap is a single movement
  // rather than a cut, and the page returns to the top with the new state.
  useEffect(() => {
    if (leavingTo === null) return;
    const timer = setTimeout(() => {
      setView(leavingTo);
      setLeavingTo(null);
      window.scrollTo({ top: 0, behavior: "smooth" });
    }, 140);
    return () => clearTimeout(timer);
  }, [leavingTo]);

  const show = (next: View) => {
    if (next !== view && leavingTo === null) setLeavingTo(next);
  };

  // "Bill this customer" is an instruction, not a place: once it has been
  // acted on the parameter goes, so reloading later — or coming back here
  // after issuing — lands on the dashboard rather than back in the composer.
  const asked = search.get("customer");
  useEffect(() => {
    if (billed !== null && asked !== null) router.replace("/dashboard", { scroll: false });
  }, [billed, asked, router]);

  const failure = account.error ?? issuers.error ?? anything.error ?? customers.error;
  if (failure) {
    return (
      <div className="grid gap-3">
        <Problem>{failure}</Problem>
        <div>
          <Button
            variant="secondary"
            size="sm"
            onClick={() => {
              account.reload();
              issuers.reload();
              anything.reload();
              customers.reload();
            }}
          >
            Try again
          </Button>
        </div>
      </div>
    );
  }

  if (
    account.data === null ||
    issuers.data === null ||
    anything.data === null ||
    customers.data === null
  ) {
    return <HomeSkeleton />;
  }

  const accountWallet = account.data.wallet_address;
  const identities = issuers.data.issuers;
  const ready = identities.filter(usable);
  const started = anything.data.payments.length > 0;
  // Setting up gates issuing, never looking: an account that has issued from
  // the CLI or the full form must still see what it has. With neither an
  // identity nor a request there is nothing to look at, so the page is the
  // setup itself.
  const forcedSetup = ready.length === 0 && !started;
  // Issuing needs an identity with a proven mailbox and somewhere to settle.
  const compose = () => show(ready.length === 0 ? "setup" : "compose");

  // Applied once the lists have arrived, the way a derived default is: the
  // composer needs the customer to exist before it can open on them.
  if (asked && asked !== billed && ready.length > 0) {
    setBilled(asked);
    setView("compose");
  }

  return (
    <div key={view} className={leavingTo === null ? "dash-enter" : "dash-leave"}>
      {onboardingPayment ? (
        <OnboardingSuccess
          payment={onboardingPayment}
          onDone={() => {
            anything.reload();
            issuers.reload();
            customers.reload();
            setOnboardingIssuer(null);
            setOnboardingPayment(null);
            show("overview");
          }}
        />
      ) : onboardingIssuer ? (
        <OnboardingWalkthrough
          issuer={onboardingIssuer}
          payoutAddress={accountWallet ?? onboardingIssuer.payout_addresses[0]?.address ?? null}
          onWalletChanged={account.reload}
          onIssued={setOnboardingPayment}
        />
      ) : view === "setup" || forcedSetup ? (
        <IssuerSetup
          issuers={identities}
          onCancel={forcedSetup ? null : () => show("overview")}
          onDone={(issuer) => {
            issuers.reload();
            if (forcedSetup) {
              // A first-time identity gets a guided tour, not the blank composer.
              // Set directly rather than through `show()`: that helper's 140ms
              // leave/enter delay would leave `view` still "overview" for a
              // moment after `issuers.reload()` resolves, and a freshly-ready
              // issuer flips `forcedSetup` false mid-transition — the ternary
              // above would fall all the way through to the real Overview for
              // one frame. `onboardingIssuer` alone must gate this branch, and
              // `view` must change in the same tick as it's set.
              setOnboardingIssuer(issuer);
              setView("onboarding");
              window.scrollTo({ top: 0, behavior: "smooth" });
            } else {
              // Adding another identity later: straight on to the thing it was for.
              show("compose");
            }
          }}
        />
      ) : view === "compose" ? (
        <RequestComposer
          issuers={ready}
          accountWallet={accountWallet}
          customers={customers.data.customers}
          billed={billed ?? undefined}
          onCancel={() => show("overview")}
          onIssued={(payment) => {
            setIssued(payment);
            anything.reload();
            customers.reload();
            show("issued");
          }}
        />
      ) : view === "issued" && issued ? (
        <Issued
          payment={issued}
          onTrack={() => {
            setTracking(issued.id);
            show("overview");
          }}
          onDone={() => show("overview")}
        />
      ) : (
        <Overview
          account={account.data}
          identities={identities}
          customers={customers.data.customers}
          openRequest={tracking ?? undefined}
          onCompose={compose}
          onAddIdentity={() => show("setup")}
          onIdentitiesChanged={issuers.reload}
          onAccountChanged={account.reload}
        />
      )}
    </div>
  );
}

/**
 * Set up but with nothing issued yet, or issuing already: the same page either
 * way, leading with the requests and carrying the identities behind them, and
 * ending on the account itself — its wallet and its API key.
 */
function Overview({
  account,
  identities,
  customers,
  openRequest,
  onCompose,
  onAddIdentity,
  onIdentitiesChanged,
  onAccountChanged,
}: {
  account: AccountMetadata;
  identities: Issuer[];
  customers: Customer[];
  /** A request whose row opens on arrival. */
  openRequest?: string | undefined;
  onCompose: () => void;
  onAddIdentity: () => void;
  onIdentitiesChanged: () => void;
  onAccountChanged: () => void;
}) {
  const router = useRouter();
  return (
    <div>
      {/* Raised over the sections below: each entrance animation is its own
          stacking context, so a later one would paint over an open filter. */}
      <div className="dash-hero dash-rise relative z-20 pt-2">
        <InvoiceTable
          identities={identities}
          customers={customers}
          initialOpen={openRequest}
          onCompose={onCompose}
        />
      </div>

      <div className="dash-rise dash-delay-3 mt-14">
        <IssuerManager
          identities={identities}
          onAdd={onAddIdentity}
          onChanged={onIdentitiesChanged}
        />
      </div>

      <div className="dash-rise dash-delay-4 mt-14">
        <CustomerTable onAdd={() => router.push("/dashboard/customers/new")} />
      </div>

      <div className="dash-rise dash-delay-5 mt-14">
        <AccountSection account={account} onChanged={onAccountChanged} />
      </div>

      <div className="dash-rise dash-delay-5 mt-14">
        <ApiKeySection account={account} onChanged={onAccountChanged} />
      </div>
    </div>
  );
}

/** The link is the deliverable, so it is what the page hands over first. */
function Issued({
  payment,
  onTrack,
  onDone,
}: {
  payment: Payment;
  onTrack: () => void;
  onDone: () => void;
}) {
  return (
    <div className="dash-hero mx-auto max-w-[620px] pt-6 text-center sm:pt-12">
      <svg viewBox="0 0 64 64" className="mx-auto size-14" fill="none" aria-hidden="true">
        <circle
          className="dash-check-ring"
          cx="32"
          cy="32"
          r="27"
          stroke="#a3d277"
          strokeWidth="2.5"
          strokeLinecap="round"
          transform="rotate(-90 32 32)"
        />
        <path
          className="dash-check-mark"
          d="M21 33.5 28.5 41 43 24"
          stroke="#a3d277"
          strokeWidth="3"
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>

      <h1 className="dash-rise dash-delay-2 font-heading mt-6 text-[30px] leading-tight font-medium tracking-[-0.045em]">
        Deposit request issued<span className="text-brand-yellow">.</span>
      </h1>
      <p className="dash-rise dash-delay-3 mx-auto mt-3 max-w-[420px] text-[15px] text-muted">
        <span className="tabular inline-flex items-center gap-1 text-ink">
          {formatDisplayAmount(payment.amount)}
          <Image
            src="/payment-icons/usdc.svg"
            width={64}
            height={64}
            alt=""
            className="size-4 shrink-0 rounded-full"
          />
          {payment.token.symbol}
        </span>{" "}
        from {payment.bill_to.name}
      </p>

      <div className="dash-rise dash-delay-4 mt-7 flex items-center gap-2 rounded-[12px] border border-line bg-surface py-2 pr-2 pl-4 text-left">
        <span className="min-w-0 flex-1 truncate font-mono text-[13px] text-muted">
          {payment.payment_url.replace(/^https?:\/\//, "")}
        </span>
        <CopyButton value={payment.payment_url} label="payment link" className="bg-raised" />
      </div>

      <div className="dash-rise dash-delay-5 mt-5 flex flex-wrap items-center justify-center gap-2.5">
        <a
          href={payment.payment_url}
          target="_blank"
          rel="noreferrer noopener"
          className={cn(
            "inline-flex h-11 items-center justify-center gap-2 rounded-[10px] bg-inverse px-4",
            "text-[15px] font-medium text-inverse-ink transition-opacity hover:opacity-90",
          )}
        >
          Open the payer&apos;s view
          <ArrowUpRight className="size-4" />
        </a>
        <button
          type="button"
          onClick={onTrack}
          className="inline-flex h-11 items-center justify-center rounded-[10px] border border-line-strong bg-surface px-4 text-[15px] font-medium transition-colors hover:bg-raised"
        >
          Track this request
        </button>
        <Button variant="ghost" onClick={onDone}>
          Done
        </Button>
      </div>
    </div>
  );
}

function HomeSkeleton() {
  return (
    <div role="status" aria-label="Loading your dashboard" className="dash-enter pt-6">
      <div className="mx-auto h-3 w-40 rounded-full bg-raised" />
      <div className="mx-auto mt-6 h-9 w-[min(420px,80%)] rounded-lg bg-raised" />
      <div className="mx-auto mt-4 h-4 w-[min(520px,90%)] rounded-full bg-raised" />
      <div className="mt-10 grid gap-4">
        <div className="h-24 rounded-[16px] border border-line bg-surface" />
        <div className="h-24 rounded-[16px] border border-line bg-surface" />
      </div>
    </div>
  );
}
