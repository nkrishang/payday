"use client";

import { PaydayError } from "@payday/sdk";
import type { UnlockedPayerDepositRequest } from "@/lib/checkout-state";
import { Loader2, PenLine, Wallet } from "lucide-react";
import { useCallback, useState } from "react";
import { useAccount, useDisconnect, useSignTypedData, useSwitchChain } from "wagmi";
import { Button } from "@/components/ui/button";
import { Problem } from "@/components/ui/field";
import { paydayChain } from "@/lib/chain";
import { config } from "@/lib/config";
import { truncateAddress } from "@/lib/format";
import { payerAttestationDefinition } from "@/lib/payer-attestation";
import { payerClient } from "@/lib/payday";
import { ConnectSheet } from "./connect-sheet";
import { walletErrorMessage } from "./wallet-errors";

/**
 * The wallet step: the payer connects the wallet they will pay from and signs
 * the request's attestation with it. The signature is what creates the
 * one-time address (the API derives it from the request and this signature)
 * and it is what the indexer later holds transfers against: money from any
 * other wallet is not credited as the payer's.
 *
 * Nothing the payer signs is composed here. The API mints the document
 * (`wallet.challenge`) for the connected address, the wallet renders it, and
 * only the signature goes back. The session the challenge belongs to is
 * handed up and stored like the email verification's, so the tab keeps its
 * standing across a reload.
 */
export function WalletAttestation({
  payment,
  payerSession,
  onSession,
  onBound,
}: {
  payment: UnlockedPayerDepositRequest;
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

  const chainMatches = payment.chain.id === String(config.chainId);

  const attest = useCallback(async () => {
    setError(null);
    if (!isConnected || !address) {
      setConnectOpen(true);
      return;
    }
    setSubmitting(true);
    try {
      // Wallets refuse typed data whose domain names another chain than the
      // one they are on, so line the wallet up first.
      if (chainId !== paydayChain.id) {
        await switchChainAsync({ chainId: paydayChain.id });
      }
      const challenge = await payerClient.wallet.challenge(payment.id, address, {
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
    onBound,
    onSession,
    payerSession,
    payment.id,
    signTypedDataAsync,
    switchChainAsync,
  ]);

  if (!chainMatches) {
    return (
      <p className="rounded-[10px] border border-line px-3.5 py-3 text-[13px] leading-relaxed text-muted">
        This checkout is configured for {config.chainName}, but the deposit request asks for{" "}
        {payment.chain.name}. It cannot take the wallet signature here; ask the merchant for a
        link on the right network.
      </p>
    );
  }

  const busy = switching || signing || submitting;
  const label = (() => {
    if (!isConnected) return "Connect the wallet you will pay from";
    if (switching) return `Switching to ${paydayChain.name}…`;
    if (signing) return "Sign in your wallet…";
    if (submitting) return "Creating your address…";
    return "Sign to get your deposit address";
  })();

  return (
    <div>
      <ConnectSheet open={connectOpen} onOpenChange={setConnectOpen} />

      <Button size="lg" onClick={attest} disabled={busy}>
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
        This signature costs no gas and moves no funds.
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
