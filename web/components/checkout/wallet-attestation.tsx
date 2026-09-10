"use client";

import { PaydayError, type Network } from "@payday/sdk";
import type { UnlockedPayerDepositRequest } from "@/lib/checkout-state";
import { Loader2, PenLine, Wallet } from "lucide-react";
import { useCallback, useState } from "react";
import { useAccount, useDisconnect, useSignTypedData, useSwitchChain } from "wagmi";
import { Button } from "@/components/ui/button";
import { Problem } from "@/components/ui/field";
import { wagmiChain } from "@/lib/chain";
import { truncateAddress } from "@/lib/format";
import { payerAttestationDefinition } from "@/lib/payer-attestation";
import { payerClient } from "@/lib/payday";
import { ConnectSheet } from "./connect-sheet";
import { walletErrorMessage } from "./wallet-errors";

/**
 * The wallet step: the payer connects the wallet they will pay from and signs
 * the request's attestation with it, on the network they chose. The signature
 * is what creates the one-time address (the API derives it from the request,
 * the chosen chain, and this signature) and it is what the indexer later
 * holds transfers against: money from any other wallet, or on any other
 * chain, is not credited as the payer's.
 *
 * Nothing the payer signs is composed here. The API mints the document
 * (`wallet.challenge`) for the connected address under the chosen chain's
 * domain, the wallet renders it, and only the signature goes back. Wallets
 * refuse typed data whose domain names another chain than the one they are
 * on, so the wallet is switched to the chosen network first; that switch is
 * the payer's own confirmation of the choice. The session the challenge
 * belongs to is handed up and stored like the email verification's, so the
 * tab keeps its standing across a reload.
 */
export function WalletAttestation({
  payment,
  network,
  payerSession,
  onSession,
  onBound,
}: {
  payment: UnlockedPayerDepositRequest;
  /** The network the payer chose, or null while they have not. */
  network: Network | null;
  payerSession: string | null;
  onSession: (token: string | null) => void;
  onBound: () => void;
}) {
  const { address, isConnected, chainId } = useAccount();
  const { disconnect } = useDisconnect();
  const { switchChainAsync, isPending: switching } = useSwitchChain();
  const { signTypedDataAsync, isPending: signing } = useSignTypedData();
  const [connectOpen, setConnectOpen] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const target = network ? wagmiChain(network.chain.id) : null;

  const attest = useCallback(async () => {
    setError(null);
    if (!network || !target) return;
    if (!isConnected || !address) {
      setConnectOpen(true);
      return;
    }
    setSubmitting(true);
    try {
      if (chainId !== target.id) {
        await switchChainAsync({ chainId: target.id });
      }
      const challenge = await payerClient.wallet.challenge(payment.id, address, network.chain.id, {
        ...(payerSession === null ? {} : { payerSession }),
      });
      onSession(challenge.payer_session);
      const signature = await signTypedDataAsync(
        payerAttestationDefinition(challenge.typed_data),
      );
      await payerClient.wallet.attest(payment.id, address, signature, challenge.payer_session);
      onBound();
    } catch (cause) {
      setError(describe(cause));
    } finally {
      setSubmitting(false);
    }
  }, [
    address,
    chainId,
    isConnected,
    network,
    onBound,
    onSession,
    payerSession,
    payment.id,
    signTypedDataAsync,
    switchChainAsync,
    target,
  ]);

  const busy = switching || signing || submitting;
  const label = (() => {
    if (!network) return "Choose a network first";
    if (!isConnected) return "Connect the wallet you will pay from";
    if (switching) return `Switching to ${target?.name ?? network.chain.name}…`;
    if (signing) return "Sign in your wallet…";
    if (submitting) return "Creating your address…";
    return `Sign to get your deposit address on ${network.chain.name}`;
  })();

  return (
    <div>
      <ConnectSheet open={connectOpen} onOpenChange={setConnectOpen} />

      <Button size="lg" onClick={attest} disabled={busy || network === null || target === null}>
        {busy ? (
          <Loader2 className="size-4 animate-spin" />
        ) : isConnected ? (
          <PenLine className="size-4" />
        ) : (
          <Wallet className="size-4" />
        )}
        {label}
      </Button>

      {isConnected && address ? (
        <div className="mt-2.5 flex items-center justify-center gap-2 text-[12px] text-faint">
          <span className="font-mono">{truncateAddress(address)}</span>
          <span aria-hidden>·</span>
          <button
            type="button"
            onClick={() => disconnect()}
            className="rounded font-medium transition-colors hover:text-ink"
          >
            Disconnect
          </button>
        </div>
      ) : null}

      <p className="mt-4 text-[12.5px] leading-relaxed text-faint">
        This signature costs no gas and moves no funds. Your wallet is switched to the network you
        chose before signing; the address it creates is only payable there.
      </p>

      <Problem>{error}</Problem>
    </div>
  );
}

function describe(cause: unknown): string {
  if (cause instanceof PaydayError) {
    switch (cause.code) {
      case "wallet_already_bound":
        return "Another wallet already signed for this request. Pay from that wallet, or ask the merchant for a new link.";
      case "wallet_signature_invalid":
        return "The signature did not come from the connected wallet. Smart-contract wallets are not supported yet; sign with a regular wallet.";
      case "wallet_challenge_required":
        return "The signing window closed. Try again.";
      case "unsupported_chain":
        return "This request cannot be paid on that network. Choose another.";
      case "payer_session_invalid":
      case "verification_required":
        return "This tab's verification session expired. Reload the page and verify again.";
      case "deposit_request_not_payable":
        return "This request is no longer open.";
      default:
        return "The signature could not be submitted. Try again in a moment.";
    }
  }
  return walletErrorMessage(cause);
}
