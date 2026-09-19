"use client";

import { AccountSection } from "./account-section";
import { ApiKeySection } from "./api-key-manager";
import { DepositTable } from "./deposit-table";
import { LoadProblem } from "./load-problem";
import { useInvalidate, useResource } from "./session";

/**
 * The dashboard.
 *
 * Gum is API-first: deposit requests, customers, and everything else about
 * payments are created and managed through the API (see /docs). The dashboard
 * is the account's own control panel, and carries the three things there is
 * nothing else to do through the API: the key that calls it, the balance of
 * settled deposits and the flow that pulls it out to a chain of the merchant's
 * choice — with the sign-in email they manage right beside it — and the
 * deposits themselves, read-only and filterable.
 */
export function DashboardHome() {
  // The account itself: the wallet deposits settle to, its email, and the
  // key state the API-key section manages.
  const account = useResource("account", (client) => client.account.get());
  // Something changed on the server: every component showing that kind of
  // thing fetches it again.
  const invalidate = useInvalidate();

  // Congestion and hiccups were retried before anything reaches here (see
  // the cache in session.tsx), so a failure with nothing to show is the page
  // itself; a failed refresh behind data already shown is not worth a word.
  if (account.data === null) {
    if (account.error) {
      return (
        <LoadProblem
          title="Couldn't load your dashboard."
          message={account.error}
          detail={account.detail}
          onRetry={account.reload}
        />
      );
    }
    return <HomeSkeleton />;
  }

  return (
    <div className="dash-enter">
      <div className="dash-rise relative z-20 pt-2">
        <ApiKeySection account={account.data} onChanged={() => invalidate("account")} />
      </div>

      <div className="dash-rise dash-delay-3 mt-14">
        <AccountSection
          account={account.data}
          onChanged={() => invalidate("account")}
        />
      </div>

      <div className="dash-rise dash-delay-4 mt-14">
        <DepositTable />
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
        <div className="h-24 rounded-[12px] border border-line bg-surface" />
        <div className="h-24 rounded-[12px] border border-line bg-surface" />
      </div>
    </div>
  );
}
