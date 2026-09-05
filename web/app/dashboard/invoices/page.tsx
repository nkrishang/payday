import { redirect } from "next/navigation";

/** The list lives on the dashboard itself; this keeps older links working. */
export default function InvoicesIndex() {
  redirect("/dashboard");
}
