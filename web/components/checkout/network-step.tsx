"use client";

import { GumError } from "@gum/sdk";
import type { UnlockedPayerDepositRequest } from "@/lib/checkout-state";
import { Loader2 } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Problem } from "@/components/ui/field";
import { payerClient } from "@/lib/gum";
import { NetworkSelect } from "./network-select";

/**
 * The network step for a request without the wallet attestation add-on: the
 * payer picks which chain to pay on and the checkout registers the one-time
 * address through the payer API's `network` endpoint — no wallet connected,
 * no signature. The address it registers is payable from any wallet, which
 * is exactly why the payer may also skip this page's wallet entirely and use
 * the QR or the copied address.
 *
 * A request offering one network has no choice to ask for: the merchant
 * pinned it, so this registers it on mount rather than showing a button that
 * says nothing. The server's answer is the newest payer view; `onRegistered`
 * makes the checkout re-read it, and the page moves on to the pay screen.
 */
export function NetworkStep({
  payment,
  payerSession,
  onRegistered,
}: {
  payment: UnlockedPayerDepositRequest;
  payerSession: string | null;
  onRegistered: () => void;
}) {
  const [selected, setSelected] = useState<string | null>(
    payment.networks[0]?.chain.id ?? null,
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The auto-registration for a pinned network runs once per mount; the
  // payment id is the only input it has, and re-reads arrive through polling.
  const auto = useRef<string | null>(null);

  const register = useCallback(
    async (chainId: string) => {
      setBusy(true);
      setError(null);
      try {
        await payerClient.depositRequests.selectNetwork(payment.id, chainId, {
          ...(payerSession === null ? {} : { payerSession }),
        });
        onRegistered();
      } catch (cause) {
        setError(describe(cause));
        setBusy(false);
      }
    },
    [onRegistered, payerSession, payment.id],
  );

  const pinned = payment.networks.length === 1 ? (payment.networks[0]?.chain.id ?? null) : null;
  useEffect(() => {
    if (pinned === null || auto.current === payment.id) return;
    auto.current = payment.id;
    void register(pinned);
  }, [pinned, payment.id, register]);

  return (
    <section aria-label="Choose the network to pay on" className="px-5 py-6 sm:px-6">
      <h2 className="text-[15px] font-semibold tracking-tight">Choose the network to pay on</h2>
      <p className="mt-2 text-[13px] leading-relaxed text-muted">
        This deposit can be paid on any of the networks below. Pick one and Gum creates a unique,
        one-time payment address on it; you can pay from any wallet.
      </p>

      <div className="mt-6">
        <NetworkSelect
          networks={payment.networks}
          selected={selected}
          onSelect={setSelected}
          disabled={busy}
        />
      </div>

      <div className="mt-5">
        <Button
          size="lg"
          onClick={() => selected && register(selected)}
          disabled={busy || selected === null}
        >
          {busy ? <Loader2 className="size-4 animate-spin" /> : null}
          {busy ? "Creating your address…" : "Get your deposit address"}
        </Button>
      </div>

      <Problem>{error}</Problem>
    </section>
  );
}

function describe(cause: unknown): string {
  if (cause instanceof GumError) {
    switch (cause.code) {
      case "network_already_chosen":
        return "This deposit request is already set to another network. Reload the page to see it.";
      case "unsupported_chain":
        return "This request cannot be paid on that network. Choose another.";
      case "payer_session_invalid":
      case "verification_required":
        return "This tab's verification session expired. Reload the page and verify again.";
      case "deposit_request_not_payable":
        return "This request is no longer open.";
      default:
        return "The network could not be registered. Try again in a moment.";
    }
  }
  return "The network could not be registered. Try again in a moment.";
}
