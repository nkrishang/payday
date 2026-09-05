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
 */
export function MerchantAuth({ children }: { children: ReactNode }) {
  return (
    <PrivyProvider
      appId={privyAppId()}
      config={{
        loginMethods: ["email"],
        embeddedWallets: { ethereum: { createOnLogin: "all-users" } },
      }}
    >
      {children}
    </PrivyProvider>
  );
}
