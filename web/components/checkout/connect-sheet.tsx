"use client";

import * as Dialog from "@radix-ui/react-dialog";
import { Loader2, Wallet, X } from "lucide-react";
import { useState } from "react";
import { useConnect, useConnectors } from "wagmi";
import { walletErrorMessage } from "./wallet-errors";

/**
 * The wallet picker.
 *
 * wagmi's EIP-6963 discovery announces one connector per installed extension,
 * so this lists what the payer actually has rather than a catalogue of wallets
 * they do not. WalletConnect, when configured, appears alongside them and opens
 * its own QR and deep-link flow for phone wallets.
 */
export function ConnectSheet({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const connectors = useConnectors();
  const { connect, isPending, variables } = useConnect();
  const [error, setError] = useState<string | null>(null);
  const [wasOpen, setWasOpen] = useState(open);

  // Reopening the sheet starts clean; a failure from a previous attempt is not
  // news the second time.
  if (open !== wasOpen) {
    setWasOpen(open);
    if (open) setError(null);
  }

  // Two connectors can announce the same wallet; the discovered one wins.
  const seen = new Set<string>();
  const choices = connectors.filter((connector) => {
    const key = connector.name.toLowerCase();
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });

  return (
    <Dialog.Root open={open} onOpenChange={onOpenChange}>
      <Dialog.Portal>
        <Dialog.Overlay className="fixed inset-0 z-50 bg-black/40 backdrop-blur-[2px]" />
        <Dialog.Content className="fixed bottom-0 left-1/2 z-50 w-full max-w-[400px] -translate-x-1/2 rounded-t-[16px] border border-line bg-surface p-5 sm:top-1/2 sm:bottom-auto sm:-translate-y-1/2 sm:rounded-[16px]">
          <div className="flex items-center justify-between">
            <Dialog.Title className="text-[15px] font-semibold tracking-tight">
              Connect a wallet
            </Dialog.Title>
            <Dialog.Close
              className="rounded-md p-1 text-faint transition-colors hover:bg-raised hover:text-ink"
              aria-label="Close"
            >
              <X className="size-4" />
            </Dialog.Close>
          </div>
          <Dialog.Description className="mt-1 text-[13px] text-muted">
            Payday never asks for a signature beyond the transfer itself.
          </Dialog.Description>

          <ul className="mt-4 space-y-1.5">
            {choices.map((connector) => {
              const connecting = isPending && variables?.connector === connector;
              return (
                <li key={connector.uid}>
                  <button
                    type="button"
                    disabled={isPending}
                    onClick={() =>
                      connect(
                        { connector },
                        {
                          onSuccess: () => onOpenChange(false),
                          onError: (cause) => setError(walletErrorMessage(cause)),
                        },
                      )
                    }
                    className="flex w-full items-center gap-3 rounded-[10px] border border-transparent px-3 py-2.5 text-left transition-colors hover:border-line hover:bg-raised disabled:opacity-50"
                  >
                    {connector.icon ? (
                      // eslint-disable-next-line @next/next/no-img-element
                      <img
                        src={connector.icon}
                        alt=""
                        width={24}
                        height={24}
                        className="size-6 rounded-md"
                      />
                    ) : (
                      <span className="flex size-6 items-center justify-center rounded-md border border-line text-muted">
                        <Wallet className="size-3.5" />
                      </span>
                    )}
                    <span className="flex-1 text-[14px] font-medium">{connector.name}</span>
                    {connecting ? <Loader2 className="size-4 animate-spin text-muted" /> : null}
                  </button>
                </li>
              );
            })}
          </ul>

          {choices.length === 0 ? (
            <p className="mt-4 rounded-[10px] border border-line bg-raised px-3.5 py-3 text-[13px] leading-relaxed text-muted">
              No wallet was detected in this browser. Install one, or scan the code below with a
              wallet on your phone.
            </p>
          ) : null}

          {error ? (
            <p role="alert" className="mt-3 text-[13px] text-danger">
              {error}
            </p>
          ) : null}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
