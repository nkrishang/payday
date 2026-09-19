import { DepositRowDetail } from "@/components/dashboard/deposit-row-detail";

/**
 * One deposit request, read-only, on a page of its own — the dashboard's
 * table opens the same detail in place, but a link to a specific request
 * (from a webhook processor, a colleague, a bookmark) should land on it
 * directly rather than at the top of a list. The shell draws the back link.
 */
export default async function DepositDetailPage({
  params,
}: {
  params: Promise<{ id: string }>;
}) {
  const { id } = await params;
  return <DepositRowDetail id={id} />;
}
