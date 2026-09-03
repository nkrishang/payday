import { PaydayError, type PayerPayment } from "@payday/sdk";
import type { Metadata } from "next";
import { notFound } from "next/navigation";
import { Checkout } from "@/components/checkout/checkout";
import { payerClient } from "@/lib/payday";

/**
 * The payment is fetched on the server so the page arrives complete — amount,
 * address, QR and status are in the first paint, with no spinner and no layout
 * shift. The client component takes over from there to keep it live.
 */
export const dynamic = "force-dynamic";

export const metadata: Metadata = {
  title: "Payment",
  // A payment link is public, but it is not something search engines should hold.
  robots: { index: false, follow: false },
};

async function loadPayment(id: string): Promise<PayerPayment> {
  try {
    return await payerClient.payments.get(id);
  } catch (error) {
    // gatewayd answers 401 `invalid_payment_link` for an unknown or malformed
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
  const payment = await loadPayment(id);

  // Rendered without any payer session: the server never sees one, so a
  // gated invoice always arrives locked and unlocks only in the browser.
  return <Checkout initial={payment} />;
}
