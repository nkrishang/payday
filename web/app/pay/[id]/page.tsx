import { PaydayError, type PayerDepositRequest } from "@payday/sdk";
import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { Checkout } from "@/components/checkout/checkout";
import { payerClient } from "@/lib/payday";

/**
 * The deposit request is fetched on the server so the page arrives complete — amount,
 * address, QR and status are in the first paint, with no spinner and no layout
 * shift. The client component takes over from there to keep it live.
 */
export const dynamic = "force-dynamic";

export const metadata: Metadata = {
  title: "Deposit Request",
  // A deposit link is public, but it is not something search engines should hold.
  robots: { index: false, follow: false },
};

async function loadDepositRequest(id: string): Promise<PayerDepositRequest> {
  try {
    return await payerClient.depositRequests.get(id);
  } catch (error) {
    // gatewayd answers 401 `invalid_deposit_link` for an unknown or malformed
    // id, so an unusable link is a not-found, not a server fault. Anything else
    // reaches error.tsx, which offers a retry.
    if (error instanceof PaydayError && (error.status === 401 || error.status === 404)) {
      notFound();
    }
    throw error;
  }
}

export default async function PayPage({ params }: { params: Promise<{ id: string }> }) {
  const { id } = await params;
  const payment = await loadDepositRequest(id);

  // Rendered without any payer session: the server never sees one, so a
  // gated deposit request always arrives locked and unlocks only in the browser.
  return <Checkout initial={payment} />;
}
