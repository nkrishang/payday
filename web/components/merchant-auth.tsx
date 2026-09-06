"use client";

import { PrivyProvider } from "@privy-io/react-auth";
import type { ReactNode } from "react";
import { privyAppId } from "@/lib/merchant-payday";

/**
 * Privy, for everything a merchant does: the landing page's sign-in dialog
 * and the whole dashboard sit under this. The checkout does not — a payer is
 * not a merchant and never gets a Privy user, so `/pay/{id}` never loads the
 * SDK.
 *
 * Email is the only way in, and every account gets an embedded EVM wallet on
 * its first sign-in; that wallet is where deposits settle by default. The
 * sign-in UI is Payday's own (`useLoginWithEmail`), so Privy's modal is never
 * shown and its appearance settings do not matter here.
 *
 * `createOnLogin` is deliberately "off": per Privy's docs
 * (basics/react/advanced/automatic-wallet-creation), it "does not trigger
 * wallet creation for users who authenticate through direct login methods
 * like loginWithCode" — exactly what signup-dialog.tsx uses — so it would be
 * a no-op here regardless of its value. The dialog calls `useCreateWallet`
 * itself instead, which is the one thing that actually ever creates it.
 */
export function MerchantAuth({ children }: { children: ReactNode }) {
  return (
    <PrivyProvider
      appId={privyAppId()}
      config={{
        loginMethods: ["email"],
        embeddedWallets: { ethereum: { createOnLogin: "off" } },
      }}
    >
      {children}
    </PrivyProvider>
  );
}
