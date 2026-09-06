import Link from "next/link";
import type { Metadata } from "next";
import { DocsPage, H2 } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "FAQ",
  description: "Short answers to the questions merchants ask about Payday.",
};

export default function FaqPage() {
  return (
    <DocsPage
      eyebrow="Trust"
      title="FAQ"
      lead="Short answers, each linking to the page with the long one."
    >
      <H2 id="request-vs-deposit">
        What is the difference between a deposit request and a deposit?
      </H2>
      <p>
        The request is the document you issue. The deposit is the funds that answer it at a one-time
        address. The API resource is the request, and you read the deposit&apos;s state from it. See{" "}
        <Link href="/docs/concepts">Deposit requests and deposits</Link>.
      </p>

      <H2 id="line-items">Can I add line items or tax?</H2>
      <p>
        No. State the amount directly and attach a PDF for an itemised breakdown or any
        jurisdiction-specific content. Payday stores and hashes the file but never parses it, and
        does not model line items, tax, or fiat.
      </p>

      <H2 id="which-asset">Which asset and network can pay?</H2>
      <p>
        One exact Circle-issued USDC contract on one chain per environment: Monad mainnet in
        production, Monad testnet in the sandbox. Read <code>chain</code> and <code>token</code>{" "}
        from the request. Bridged USDC, look-alike tokens, another network&apos;s USDC, and gas
        tokens do not count and may be unrecoverable. See{" "}
        <Link href="/docs/environments">Environments</Link>.
      </p>

      <H2 id="why-null-address">Why is the address null after I create a request?</H2>
      <p>
        Because it does not exist yet. The address is derived from the payer&apos;s wallet
        signature, so it appears once the payer signs on the hosted page. Wait for{" "}
        <code>deposit_request.ready</code> or poll until <code>address</code> is set. See{" "}
        <Link href="/docs/concepts#one-request-one-address-one-wallet">
          One request, one address
        </Link>
        .
      </p>

      <H2 id="what-to-give-the-payer">What should I give the payer?</H2>
      <p>
        The <code>deposit_url</code>. It shows everything they need, withholds what they should not
        see, and handles the wallet step. If you build your own page, reproduce the safety guidance
        in <Link href="/docs/checkout">Hosted checkout</Link>.
      </p>

      <H2 id="not-appeared">A payer says they paid. Why has nothing changed?</H2>
      <p>
        Payday credits only finalized transfers. A wallet can show a transaction as confirmed before
        the chain finalizes it and before Payday&apos;s indexer reaches it. Check <code>as_of</code>
        , <code>indexer_freshness</code>, and the request&apos;s <code>transfers</code>; if the
        transfer went to the right address from the right token, it will be credited. If it was the
        wrong token or the wrong network, it may be unrecoverable.
      </p>

      <H2 id="partial-excess">What happens with partial, excess, or late funds?</H2>
      <p>
        Partial transfers accumulate. Excess goes back to the payer&apos;s wallet at settlement.
        Funds still short at the deadline, or arriving after it, go back to the payer&apos;s wallet.
        Nothing is held by Payday. See{" "}
        <Link href="/docs/concepts#where-the-usdc-goes">Where the USDC goes</Link>.
      </p>

      <H2 id="cancel-refund">Can I cancel or refund through Payday?</H2>
      <p>
        Cancelling is advisory: it stops Payday&apos;s pages from presenting the request but cannot
        disable an address. There is no refund endpoint. Funds that settled to your wallet are yours
        to refund through your own process; funds Payday routes back to the payer go automatically,
        and <code>deposit_request.recovered_funds</code> tells you when.
      </p>

      <H2 id="wrong-wallet">
        The payer sent from a different wallet than the one they signed with.
      </H2>
      <p>
        The funds count and settle, but the request is flagged likely unsolicited and no Proof of
        Payment is issued, because the attested wallet did not pay. Anything returned goes to the
        attested wallet, not the sending one. Treat the flag as a prompt to check who actually paid.
      </p>

      <H2 id="poll-or-webhooks">Should I poll or use webhooks?</H2>
      <p>
        Webhooks for automation, long polling for a screen that is open now. Verify the signature
        over the raw body, reject stale timestamps, and deduplicate on the event id. See{" "}
        <Link href="/docs/webhooks">Webhooks</Link>.
      </p>

      <H2 id="needs-attention">What does needs_attention mean?</H2>
      <p>
        Automatic movement stopped because Payday could not safely continue. Do not send more funds.
        Follow <code>attention.action</code> and contact support with the request id and the API
        request id. Funds are neither lost nor delivered until an operator releases it.
      </p>

      <H2 id="proof">What is Proof of Payment and who can verify it?</H2>
      <p>
        A JSON record of the document, the payer&apos;s wallet signature, the address derivation,
        the transfers, and the settlement transaction. Anyone can recompute it offline, and the
        checks are published. See <Link href="/docs/proof-of-payment">Proof of Payment</Link>.
      </p>

      <H2 id="keys">How should I store and rotate API keys?</H2>
      <p>
        In a secret manager, never in a browser, a repository, a URL, or a log. Roll a key from the
        dashboard and the old one keeps working for 24 hours; revoke for a compromise. A key cannot
        mint another key. See <Link href="/docs/api#authentication">Authentication</Link>.
      </p>

      <H2 id="amounts">How are amounts and deadlines encoded?</H2>
      <p>
        Amounts are decimal strings with six decimals beside integer base-unit strings; never use
        floating point. Deadlines are RFC 3339 in UTC; set them with <code>expires_in</code> seconds
        or <code>expires_at</code>, default 24 hours, between 10 minutes and 366 days.
      </p>

      <H2 id="sandbox">Is there a sandbox?</H2>
      <p>
        Yes: <code>https://api.sandbox.payday.sh</code> with <code>payday_test_</code> keys, on
        Monad testnet with Circle&apos;s test USDC and the same real indexing and finality. See{" "}
        <Link href="/docs/environments">Environments</Link>.
      </p>

      <H2 id="help">Where can I get help?</H2>
      <p>
        Email <a href="mailto:support@payday.sh">support@payday.sh</a> with the request id and the{" "}
        <code>dr_</code> id. Never send an API key, a verification code, a webhook secret, or more
        data than the question needs.
      </p>
    </DocsPage>
  );
}
