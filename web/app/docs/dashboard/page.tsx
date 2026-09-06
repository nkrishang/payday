import type { Metadata } from "next";
import { Callout, DocsPage, H2, H3, Step, Steps, Table } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Dashboard",
  description:
    "Issue and track deposit requests from the Payday dashboard: sign-in, issuer identities, the composer, the request detail, customers, and API keys.",
};

export default function DashboardPage() {
  return (
    <DocsPage
      eyebrow="Using Payday"
      title="Dashboard"
      lead="The dashboard at payday.sh/dashboard is the browser face of the same API the SDK uses. It issues deposit requests, keeps customers, uploads the one PDF a request may carry, and shows what happened to each one."
    >
      <p>
        It adds no rules of its own. Every limit, policy check, and status comes from the API; the
        forms only give immediate feedback before the API has the final word. Anything you see in
        the dashboard, your integration can read through the API, and anything your integration
        issues appears in the dashboard.
      </p>

      <H2 id="signing-in">Signing in</H2>
      <p>
        Choose <strong>Start Building</strong> on the landing page and enter your email. A one-time
        code arrives; entering it signs you in. There is no separate registration: the first
        accepted code creates the account, every later one signs into it. Each account gets its own
        EVM wallet at that first sign-in. It is yours, and it is where deposits settle unless you
        choose another address.
      </p>
      <p>
        The session lives in your browser and refreshes while you stay signed in. No API key ever
        exists in the browser; the dashboard authenticates with the session itself, and the API
        accepts that on every merchant route.
      </p>

      <H2 id="first-run">Your first request</H2>
      <p>
        A new account is not shown a tour. It is put to work on the one thing the product needs
        first: an <strong>issuer identity</strong>, the party your requests are issued under. Once
        one exists with a proven mailbox, issuing follows straight out of it.
      </p>
      <Steps>
        <Step title="Name the identity and its contact address">
          <p>
            The name appears on every request as the issuer. The contact address is where payers are
            told to write, so Payday proves it with an emailed code before any request carries it.
          </p>
        </Step>
        <Step title="Confirm the code">
          <p>
            Enter the six digits sent to that mailbox. The identity is now usable. The flow resumes
            from wherever it was left, so an abandoned tab reopens at the unfinished step.
          </p>
        </Step>
        <Step title="Issue">
          <p>
            The composer opens in place. When the API answers, the issued link takes its place,
            ready to copy and send.
          </p>
        </Step>
      </Steps>

      <H2 id="the-page">The dashboard page</H2>
      <p>
        Signing in lands on <code>/dashboard</code>, which is the whole dashboard: the deposit
        requests, the identities they are issued under, the customers they are addressed to, the
        account and its API key, and the flow that issues a new request. There are no section tabs;
        the only other pages are the detail of one record.
      </p>

      <H3 id="deposit-requests">Deposit requests</H3>
      <p>
        A table of every request, five at a time, newest first, with filters for status,
        verification, and customer. Each filter is the API&apos;s own parameter, so it narrows the
        query rather than the page. A row shows the heading or reference, the payer, the amount with
        the token&apos;s mark, the status, the policy mode, the verification state, the identity it
        was issued under, and a paperclip when a PDF is attached.
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
              The payer&apos;s address and details, the notes, and a link to the saved customer.
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
              The attached PDF, Payday&apos;s own PDF rendering of the request, and, once settled,
              the Proof of Payment as JSON.
            </td>
          </tr>
        </tbody>
      </Table>

      <H3 id="new-request">New deposit request</H3>
      <p>
        The only way to issue one by hand, in four steps with a running preview beside them that
        doubles as the review. Every field <code>POST /v1/deposit-requests</code> takes is here.
      </p>
      <ol>
        <li>
          <strong>Amount.</strong> The USDC amount, used directly. The identity when you have more
          than one, the payout wallet when the identity offers alternatives to your Payday wallet,
          and the deadline: 24 hours, 7 days, 30 days, or a moment you pick.
        </li>
        <li>
          <strong>Payer.</strong> Pick a saved customer, or type the payer fresh and it is saved as
          one. Then the heading, a reference, notes, and the PDF. The file uploads straight to
          storage and shows its own stages: uploading, scanning, ready with its size and hash, or
          rejected with the reason.
        </li>
        <li>
          <strong>Verification.</strong> Permissionless, or verified email with the expected
          address, pre-filled from the payer&apos;s email until you type your own. Merchant session
          is not offered here; it is created through the API by an application that can hand the
          payer the secret.
        </li>
        <li>
          <strong>Review.</strong> Every value as it will be sent, with a plain statement that an
          issued request is immutable.
        </li>
      </ol>

      <H3 id="issuer-identities">Issuer identities</H3>
      <p>
        An identity is your side of a request, saved once instead of retyped: the name, the contact
        mailbox, and optionally saved payout wallets. An open row is its own form: rename it, move
        the contact address (which drops the proof, because a different mailbox is a different
        claim), and attach or drop saved wallets. Names are unique within the account. Editing an
        identity never changes a request already issued; each request keeps its own snapshot and
        remembers the identity&apos;s id, so &quot;issued under Acme&quot; stays answerable after a
        rename.
      </p>

      <H3 id="customers">Customers</H3>
      <p>
        A customer is a reusable payer record: name, optional email, optional details. The list
        pages below the requests. A customer&apos;s own page edits it, shows how many requests it
        has and what they collected, and links to a new request pre-filled from it.
      </p>

      <H3 id="account-and-api-key">Account and API key</H3>
      <p>
        The foot of the page is the account: the mailbox you signed in with, your Payday wallet in
        full with its USDC and gas balance read from the public chain, and a sign-out. Below it, the{" "}
        <strong>API key</strong> section generates, rolls, and revokes the key your own server calls
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
              the <a href="/docs/concepts#lifecycle">lifecycle</a>.
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
