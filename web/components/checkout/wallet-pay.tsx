"use client";

import type { ReadyPayerPayment } from "@/lib/checkout-state";
import { Loader2, Wallet } from "lucide-react";
import { useCallback, useEffect, useState } from "react";
import { erc20Abi, type Hex } from "viem";
import {
  useAccount,
  useBalance,
  useDisconnect,
  useReadContract,
  useSwitchChain,
  useWaitForTransactionReceipt,
  useWriteContract,
} from "wagmi";
import { Button } from "@/components/ui/button";
import { paydayChain } from "@/lib/chain";
import { config } from "@/lib/config";
import { formatBaseUnits, formatDisplayAmount, truncateAddress } from "@/lib/format";
import { ConnectSheet } from "./connect-sheet";
import { walletErrorMessage } from "./wallet-errors";

const ZERO = "0x0000000000000000000000000000000000000000" as const;

/**
 * Paying in the page is a plain ERC-20 transfer to the payment's one-time
 * address — no approval, no contract call, nothing that can redirect funds.
 *
 * Two invariants are enforced here rather than trusted:
 *
 * 1. The amount is `remaining_base_units` read straight off the newest payment
 *    at the moment of signing. It is never re-derived from the display string
 *    and never taken from a stale render.
 * 2. The chain and token contract are checked against this deployment's
 *    configured values before the button will do anything, so a wrong or
 *    tampered response cannot get a signature for an unexpected token.
 * 3. The connected wallet must be the one the payer attested. The address
 *    commits to that wallet and the indexer credits only its transfers, so
 *    the button refuses to send from any other rather than let money arrive
 *    that will not count.
 */
export function WalletPay({
  payment,
  onSent,
}: {
  payment: ReadyPayerPayment;
  onSent: (hash: string) => void;
}) {
  const { address, isConnected, chainId } = useAccount();
  const { disconnect } = useDisconnect();
  const { switchChainAsync, isPending: switching } = useSwitchChain();
  const { writeContractAsync, isPending: signing } = useWriteContract();

  const [connectOpen, setConnectOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [txHash, setTxHash] = useState<Hex | null>(null);

  const chainMatches = payment.chain.id === String(config.chainId);
  const tokenMatches = payment.token.address.toLowerCase() === config.usdcAddress;
  const supported = chainMatches && tokenMatches;
  const walletMatches =
    !isConnected || !address || address.toLowerCase() === payment.payer_wallet.toLowerCase();

  const usdc = useReadContract({
    address: payment.token.address as Hex,
    abi: erc20Abi,
    functionName: "balanceOf",
    args: [address ?? ZERO],
    chainId: paydayChain.id,
    query: { enabled: Boolean(address) && supported },
  });

  const gas = useBalance({
    address,
    chainId: paydayChain.id,
    query: { enabled: Boolean(address) && supported },
  });

  const receipt = useWaitForTransactionReceipt({
    hash: txHash ?? undefined,
    chainId: paydayChain.id,
    query: { enabled: txHash !== null },
  });

  const confirmed = txHash !== null && receipt.data?.status === "success";
  const reverted = txHash !== null && receipt.data?.status === "reverted";

  // The only side effect: tell the checkout a transfer landed, so it can show
  // the finality wait and start polling fast. Local state is left alone —
  // once the checkout knows, it stops rendering this component.
  useEffect(() => {
    if (confirmed && txHash) onSent(txHash);
  }, [confirmed, txHash, onSent]);

  const due = BigInt(payment.remaining_base_units);
  const holdsEnough = usdc.data === undefined || usdc.data >= due;
  const hasGas = gas.data === undefined || gas.data.value > 0n;
  const waiting = txHash !== null && receipt.isLoading;
  const busy = switching || signing || waiting;

  const pay = useCallback(async () => {
    setError(null);

    if (!isConnected) {
      setConnectOpen(true);
      return;
    }
    if (!walletMatches) return;

    try {
      setTxHash(null);

      if (chainId !== paydayChain.id) {
        await switchChainAsync({ chainId: paydayChain.id });
      }

      // Re-read the outstanding amount at signing time: a partial payment may
      // have landed while this page was open.
      const amount = BigInt(payment.remaining_base_units);
      if (amount <= 0n) return;

      const hash = await writeContractAsync({
        chainId: paydayChain.id,
        address: payment.token.address as Hex,
        abi: erc20Abi,
        functionName: "transfer",
        args: [payment.address as Hex, amount],
      });
      setTxHash(hash);
    } catch (cause) {
      setError(walletErrorMessage(cause));
    }
  }, [
    chainId,
    isConnected,
    payment.address,
    payment.remaining_base_units,
    payment.token.address,
    switchChainAsync,
    walletMatches,
    writeContractAsync,
  ]);

  if (!supported) {
    return (
      <p className="rounded-[10px] border border-line px-3.5 py-3 text-[13px] leading-relaxed text-muted">
        This checkout is configured for {config.chainName}, but the payment asks for{" "}
        {payment.chain.name} and {truncateAddress(payment.token.address)}. Pay by scanning the code
        or copying the address instead.
      </p>
    );
  }

  const label = (() => {
    if (!isConnected) return "Pay with wallet";
    if (switching) return `Switching to ${paydayChain.name}…`;
    if (signing) return "Confirm in your wallet…";
    if (waiting) return "Waiting for confirmation…";
    if (chainId !== paydayChain.id) return `Switch to ${paydayChain.name} and pay`;
    return `Pay ${formatDisplayAmount(payment.remaining)} ${payment.token.symbol}`;
  })();

  const blocked = isConnected && (!walletMatches || !holdsEnough || !hasGas);
  const message = (() => {
    if (error) return error;
    if (reverted) return "The transfer was reverted on-chain. Nothing was sent.";
    return null;
  })();

  return (
    <div>
      <ConnectSheet open={connectOpen} onOpenChange={setConnectOpen} />

      <Button size="lg" onClick={pay} disabled={busy || blocked}>
        {busy ? <Loader2 className="size-4 animate-spin" /> : <Wallet className="size-4" />}
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

      {isConnected && !walletMatches ? (
        <p role="alert" className="mt-2.5 text-center text-[13px] text-warning">
          This request is bound to{" "}
          <span className="font-mono">{truncateAddress(payment.payer_wallet)}</span>. Switch to
          that wallet: transfers from any other are not credited to you.
        </p>
      ) : null}

      {isConnected && walletMatches && !holdsEnough && usdc.data !== undefined ? (
        <p className="mt-2.5 text-center text-[13px] text-warning">
          This wallet holds {formatBaseUnits(usdc.data, payment.token.decimals)}{" "}
          {payment.token.symbol}, less than the {formatDisplayAmount(payment.remaining)} due.
        </p>
      ) : null}

      {isConnected && walletMatches && holdsEnough && !hasGas ? (
        <p className="mt-2.5 text-center text-[13px] text-warning">
          This wallet has no {config.nativeSymbol} to pay for gas on {paydayChain.name}.
        </p>
      ) : null}

      {message ? (
        <p role="alert" className="mt-2.5 text-center text-[13px] text-danger">
          {message}
        </p>
      ) : null}
    </div>
  );
}
