import Link from "next/link";
import type { Metadata } from "next";
import { Callout, DocsPage, H2, Table } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Dashboard",
  description:
    "The Gum dashboard: your API key, the balance of settled deposits and the flow that withdraws it, your sign-in email, and every deposit read-only.",
};

export default function DashboardPage() {
  return (
    <DocsPage
      eyebrow="Using Gum"
      title="Dashboard"
      lead="The dashboard at gum.money/dashboard is the account's control panel: the key that calls the API, the balance of settled deposits and the flow that pulls it out to a chain of your choice, and every deposit itself, read-only."
    >
      <p>
        Gum is API-first: deposit requests, customers, attachments, webhooks — everything about
        payments — is created and managed through the API (see the{" "}
        <Link href="/docs/api">API reference</Link> and the{" "}
        <Link href="/docs/quickstart">quickstart</Link>). The dashboard creates nothing. It shows
        the account, its money, and its key.
      </p>

      <H2 id="signing-in">Signing in</H2>
      <p>
        Choose <strong>Start Building</strong> on the landing page and enter your email. A one-time
        code arrives; entering it signs you in. There is no separate registration: the first
        accepted code creates the account, every later one signs into it. Each account gets its own
        EVM wallet at that first sign-in. It is yours — Gum never holds its key. A deposit request
        settles to the <code>payout_address</code> its creation names, so the dashboard&apos;s
        balances and withdrawals cover exactly the requests paid to this wallet&apos;s address.
      </p>
      <p>
        The session lives in your browser and refreshes while you stay signed in. No API key ever
        exists in the browser; the dashboard authenticates with the session itself, and the API
        accepts that on every merchant route.
      </p>

      <H2 id="api-key">API key</H2>
      <p>
        The top of the page is the <strong>API key</strong> section: the key your own server calls
        the API with.
      </p>
      <ul>
        <li>
          <strong>Generate</strong> shows the key exactly once. Store it in a secret manager.
        </li>
        <li>
          <strong>Roll</strong> issues a replacement while the previous key keeps working for 24
          hours, so a deployment can move over without downtime.
        </li>
        <li>
          <strong>Revoke</strong> invalidates the current key and any key in its grace window at
          once. Use it for a suspected compromise.
        </li>
      </ul>
      <Callout title="A key cannot mint another key">
        Only a signed-in dashboard session can generate, roll, or revoke. An API key presenting
        itself to those routes is refused, so whoever holds a key can never widen their own access.
      </Callout>

      <H2 id="balance">Balance and withdrawals</H2>
      <p>
        The middle of the page is the account: the sign-in email, the Gum wallet in full with its
        balance in every stablecoin each network serves, read from the public chain, and a
        sign-out.
      </p>
      <p>
        <strong>Change</strong> beside the sign-in email moves the account to a new mailbox: a code
        is sent to the new address, and the email changes only once you enter it. Until then the
        old address is still the signed-in one.
      </p>
      <p>
        <strong>Withdraw</strong> beneath the balances moves everything the wallet holds in one
        currency to one address you name, on a chain of your choice, offering only the networks
        that serve it: USDC from every network at once, USDT from the destination network alone;
        you sign once per leg and the page tracks each until it lands (
        <Link href="/docs/withdrawals">Withdrawals</Link>). <strong>Export wallet key</strong>{" "}
        shows you the wallet&apos;s key, once, for withdrawing from your own server.
      </p>

      <H2 id="deposits">Deposits</H2>
      <p>
        The foot of the page is a read-only table of every deposit request, five at a time, newest
        first, with filters for status, verification, and customer. Each filter is the API&apos;s
        own parameter, so it narrows the query rather than the page. A row shows the heading or
        reference, the payer, the issuer id the request was created with, the amount with the
        token&apos;s mark, the status, the policy mode, the verification state, and a paperclip
        when a PDF is attached.
      </p>
      <p>
        Clicking a row opens it in place. The open row leads with two things a list cannot show: a
        bar for how much of the amount has arrived, and a three-point rail for how far through its
        life the request is. Under that, grouped by what you came for:
      </p>
      <Table>
        <thead>
          <tr>
            <th>Section</th>
            <th>What it holds</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>Document</td>
            <td>
              The payer&apos;s address and details, the notes, the customer id, and the issuer id
              the request carries.
            </td>
          </tr>
          <tr>
            <td>Verification</td>
            <td>
              For gated requests: the policy, your assertion, the verdict, and every attempt the
              payer made with its status and time. Never the code or the session.
            </td>
          </tr>
          <tr>
            <td>Deposit</td>
            <td>
              The one-time address, the payout address, network and token, the funded time, the
              settlement transaction, and any attention message. Addresses link to the explorer.
            </td>
          </tr>
          <tr>
            <td>Recovered funds</td>
            <td>
              Anything that went back to the payer&apos;s wallet, with its transaction and reason.
            </td>
          </tr>
          <tr>
            <td>Payer&apos;s view</td>
            <td>
              Exactly what the payer sees, read through the public route. For a gated request,
              exactly what an unverified visitor would see, so you can check before sending. The
              shareable link sits beside it, and a preview opens the unlocked page for you alone.
            </td>
          </tr>
          <tr>
            <td>Files</td>
            <td>
              The attached PDF, Gum&apos;s own PDF rendering of the request, and, once settled,
              the Proof of Payment as JSON.
            </td>
          </tr>
        </tbody>
      </Table>
      <p>
        A deposit request has no page of its own beyond its row — links to a specific request open
        the same read-only detail at <code>/dashboard/deposits/&#123;id&#125;</code>.
      </p>

      <H2 id="indicators">Reading the indicators</H2>
      <Table>
        <thead>
          <tr>
            <th>Indicator</th>
            <th>Meaning</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>Status</td>
            <td>
              The deposit&apos;s lifecycle state, from awaiting deposit to settled or returned. See
              the <Link href="/docs/concepts#lifecycle">lifecycle</Link>.
            </td>
          </tr>
          <tr>
            <td>Verification</td>
            <td>
              Not required, pending, or verified with a time. Separate from status: a gated request
              can be funded before its payer has verified.
            </td>
          </tr>
          <tr>
            <td>Payer wallet</td>
            <td>
              The wallet the payer signed with, shown beside the address once it exists. Only that
              wallet&apos;s transfers are the payer&apos;s.
            </td>
          </tr>
          <tr>
            <td>Likely unsolicited</td>
            <td>
              Finalized funds arrived from a wallet other than the attested one. They count and
              settle, but no Proof of Payment will claim the payer paid them. Check who actually
              paid.
            </td>
          </tr>
          <tr>
            <td>Returned to the payer</td>
            <td>
              Present only when something went back: an overpayment remainder, an expired balance,
              or a late transfer, each with its transaction.
            </td>
          </tr>
        </tbody>
      </Table>
    </DocsPage>
  );
}
