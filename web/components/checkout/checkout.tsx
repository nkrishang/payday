"use client";

import type { PayerDepositRequest } from "@payday/sdk";
import { useCallback, useEffect, useState, useSyncExternalStore } from "react";
import { checkoutView, readyDepositRequest, unlockedDepositRequest } from "@/lib/checkout-state";
import { takePreviewSession, usePayerSession } from "@/lib/payer-session";
import { StatusDot } from "@/components/ui/status-dot";
import { AddressRow } from "./address-row";
import { AmountDue, ReceivedProgress } from "./amount";
import { AssetNotice } from "./asset-notice";
import { Countdown } from "./countdown";
import { CheckoutFrame } from "./frame";
import { RequestDetails } from "./request-details";
import { ClientSecretExchange, type ClientSecretStatus } from "./merchant-session";
import { NetworkSelect } from "./network-select";
import { WalletProviders } from "./providers";
import { QrPanel } from "./qr-panel";
import { Resolved } from "./resolved";
import { useDepositRequest, useSecondsRemaining } from "./use-deposit-request";
import { VerificationGate } from "./verification-gate";
import { WalletAttestation } from "./wallet-attestation";
import { WalletPay } from "./wallet-pay";

export function Checkout({
  initial,
  embedded,
}: {
  initial: PayerDepositRequest;
  /** See `CheckoutFrame`: renders without page-owning chrome. */
  embedded?: boolean | undefined;
}) {
  return (
    <WalletProviders>
      <CheckoutBody initial={initial} embedded={embedded} />
    </WalletProviders>
  );
}

/** Components reading a chosen network through the hook, told when it changes. */
const networkListeners = new Set<() => void>();

function subscribeNetwork(listener: () => void): () => void {
  networkListeners.add(listener);
  return () => {
    networkListeners.delete(listener);
  };
}

/** The choices made in this page, for browsers whose storage is unavailable. */
const chosenNetworks = new Map<string, string>();

function readChosenNetwork(key: string): string | null {
  try {
    return window.sessionStorage.getItem(key) ?? chosenNetworks.get(key) ?? null;
  } catch {
    return chosenNetworks.get(key) ?? null;
  }
}

/**
 * The network the payer picked for this request, remembered in this tab so a
 * reload between choosing and signing does not lose it. Server rendering and
 * hydration see no choice, so the markup agrees on both sides; the client
 * then reads the tab's storage. Nothing about it is trusted: the API only
 * ever binds the chain the challenge was minted for.
 */
function useChosenNetwork(id: string): [string | null, (chainId: string) => void] {
  const key = `payday:network:${id}`;
  const chosen = useSyncExternalStore(
    subscribeNetwork,
    () => readChosenNetwork(key),
    () => null,
  );
  const choose = useCallback(
    (chainId: string) => {
      chosenNetworks.set(key, chainId);
      try {
        window.sessionStorage.setItem(key, chainId);
      } catch {
        // Storage may be unavailable; the choice then lives in memory only.
      }
      for (const listener of networkListeners) listener();
    },
    [key],
  );
  return [chosen, choose];
}

function CheckoutBody({
  initial,
  embedded,
}: {
  initial: PayerDepositRequest;
  embedded?: boolean | undefined;
}) {
  // This tab's payer session, if it has one. It rides along on every read,
  // so a gated deposit request unlocks here after verification and stays unlocked
  // across a reload; it never reaches the server-rendered page.
  const [payerSession, setPayerSession] = usePayerSession(initial.id);
  const { payment, receivedAt, reconnecting, pendingTxHash, markSent, refresh } = useDepositRequest(
    initial,
    payerSession,
  );
  const [emailCodeSent, setEmailCodeSent] = useState(false);
  const [chosenChain, chooseChain] = useChosenNetwork(initial.id);
  const updateSession = useCallback(
    (token: string | null) => {
      if (token === null) setEmailCodeSent(false);
      setPayerSession(token);
    },
    [setPayerSession],
  );
  // The deposit request's own issuing merchant previews it exactly this way,
  // regardless of payer policy: a session already minted server-side
  // (`depositRequests.previewSession`), carried in the fragment precisely
  // like a merchant-session client secret, but adopted directly — there is
  // nothing to exchange, it is already a valid session.
  useEffect(() => {
    const token = takePreviewSession();
    if (!token) return;
    // Deferred a tick, like the client secret exchange below: an effect
    // should not call setState synchronously within its own body.
    void Promise.resolve().then(() => updateSession(token));
    // Runs once, on mount, exactly like the client secret exchange below.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  // A merchant-session deposit request arrives with its client secret in the URL
  // fragment. The exchange below reads it once, on the client only, and the
  // session it mints takes the same path the email flow's session does.
  const merchantSession = payment.payer_policy.mode === "merchant_session";
  const [clientSecretStatus, setClientSecretStatus] = useState<ClientSecretStatus>(() =>
    merchantSession ? "exchanging" : "none",
  );
  const secondsRemaining = useSecondsRemaining(payment, receivedAt);
  const view = checkoutView(payment, {
    secondsRemaining,
    pendingTxHash,
    emailCodeSent,
    exchangingClientSecret: clientSecretStatus === "exchanging",
  });
  // Null exactly when the phase is one of the locked ones: the same narrowing
  // decides the phase and what may enter the tree. `ready` is null until the
  // payer's wallet is bound and the address exists.
  const unlocked = unlockedDepositRequest(payment);
  const ready = unlocked === null ? null : readyDepositRequest(unlocked);
  // The chosen network, if it is one the request offers.
  const chosenNetwork =
    unlocked?.networks.find((network) => network.chain.id === chosenChain) ?? null;

  return (
    <CheckoutFrame
      embedded={embedded}
      aside={
        reconnecting ? (
          <span className="text-[12px] text-faint" role="status">
            Reconnecting…
          </span>
        ) : null
      }
    >
      <div className="flex items-center justify-between gap-3 border-b border-line px-5 py-3 sm:px-6">
        <span className="flex items-center gap-2 text-[13px] font-medium">
          <StatusDot tone={view.tone} pulse={!view.isTerminal} />
          {view.label}
        </span>
        {view.showInstructions ? <Countdown seconds={secondsRemaining} /> : null}
      </div>

      {merchantSession ? (
        <ClientSecretExchange
          paymentId={payment.id}
          payerSession={payerSession}
          onStatus={setClientSecretStatus}
          onSession={updateSession}
        />
      ) : null}

      {unlocked === null ? (
        <VerificationGate
          payment={payment}
          view={view}
          payerSession={payerSession}
          clientSecretStatus={clientSecretStatus}
          onSession={updateSession}
          onCodeSent={() => setEmailCodeSent(true)}
          onVerified={refresh}
        />
      ) : (
        <>
          <RequestDetails payment={unlocked} payerSession={payerSession} />

          {view.showWalletStep ? (
            <section aria-label={view.title} className="px-5 py-6 sm:px-6">
              <AmountDue payment={unlocked} />

              <h2 className="mt-5 text-[15px] font-semibold tracking-tight">{view.title}</h2>
              <p className="mt-2 text-[13px] leading-relaxed text-muted">{view.detail}</p>

              <div className="mt-6">
                <NetworkSelect
                  networks={unlocked.networks}
                  selected={chosenNetwork?.chain.id ?? null}
                  onSelect={chooseChain}
                />
              </div>

              <div className="mt-5">
                <WalletAttestation
                  payment={unlocked}
                  network={chosenNetwork}
                  payerSession={payerSession}
                  onSession={updateSession}
                  onBound={refresh}
                />
              </div>
            </section>
          ) : view.showInstructions && ready !== null ? (
            <section aria-label={view.title} className="px-5 py-6 sm:px-6">
              <AmountDue payment={ready} />
              <ReceivedProgress payment={ready} />

              <p className="mt-4 text-[13px] leading-relaxed text-muted">{view.detail}</p>

              <div className="mt-6">
                <WalletPay payment={ready} onSent={markSent} />
              </div>

              <div className="my-6 flex items-center gap-3" aria-hidden>
                <span className="h-px flex-1 bg-line" />
                <span className="text-[11px] font-medium tracking-[0.14em] text-faint uppercase">
                  or send it yourself
                </span>
                <span className="h-px flex-1 bg-line" />
              </div>

              <QrPanel payment={ready} payerSession={payerSession} />

              <div className="mt-6 space-y-4">
                <AddressRow payment={ready} />
                <AssetNotice payment={ready} />
              </div>
            </section>
          ) : (
            <Resolved payment={unlocked} view={view} pendingTxHash={pendingTxHash} />
          )}
        </>
      )}
    </CheckoutFrame>
  );
}
