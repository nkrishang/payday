import { redirect } from "next/navigation";

/**
 * A request is read in its own row on the dashboard now, not on a page of its
 * own. Links already sent out — from the CLI, from a colleague — still land
 * somewhere useful.
 */
export default function InvoiceDetailPage() {
  redirect("/dashboard");
}
