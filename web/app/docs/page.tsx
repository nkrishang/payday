import Link from "next/link";
import type { Metadata } from "next";
import { DepositFlowDiagram } from "@/components/docs/diagrams";
import {
  Callout,
  Card,
  Cards,
  Compare,
  CompareItem,
  DocsPage,
  Figure,
  H2,
  Pill,
  Step,
  Steps,
} from "@/components/docs/prose";

export const metadata: Metadata = {
  // A layout's template only reaches child segments, so the index names
  // itself the way the rest of the docs are named.
  title: { absolute: "Gum Docs · Introduction" },
  description:
    "What Gum does, how a deposit flows from request to settlement, and where to start.",
};

export default function IntroductionPage() {
  return (
    <DocsPage
      eyebrow="Getting started"
      title={<>Stablecoin deposits that settle themselves.</>}
      lead="Gum turns USDC and USDT transfers into verified customer deposits. You state what you are owed and by whom; Gum gives that one payer a one-time address, watches the chain, and moves exactly the requested amount to your wallet."
    >
      <p>
        You create a <strong>deposit request</strong> through a small API or the dashboard. Gum
        gives you a link. The payer opens it, proves whatever your policy asks of them, and signs
        once from the wallet they will pay from. That signature creates the one-time address they
        pay into. When finalized funds reach it, Gum settles exactly the amount you asked for to
        your wallet, and anything left over goes straight back to the payer&apos;s own wallet
        on-chain.
      </p>

      <Figure caption="One deposit, end to end. Solid arrows are money and messages; the dashed arrow is the webhooks that tell your server what happened. Gum never holds the funds at any point.">
        <DepositFlowDiagram />
      </Figure>

      <H2 id="how-a-deposit-works">How a deposit works</H2>
      <Steps>
        <Step title="You issue a deposit request">
          <p>
            An amount in USDC or USDT, who is asking, who should pay, a deadline, and a payer
            policy. From the dashboard, or with one API call. You get back an id and a{" "}
            <code>deposit_url</code>.
          </p>
        </Step>
        <Step title="The payer opens the link and satisfies the policy">
          <p>
            Permissionless requests open at once. A verified-email request asks the payer to type a
            code sent to the mailbox you named. A merchant-session request opens unlocked when your
            own application hands the payer in.
          </p>
        </Step>
        <Step title="The payer signs from the wallet they will pay from">
          <p>
            One signature, no transaction. It binds that wallet to this request, and Gum derives
            the one-time deposit address from it. Only transfers from that wallet are the
            payer&apos;s, and anything returned goes back to it.
          </p>
        </Step>
        <Step title="Funds arrive and are finalized">
          <p>
            Gum credits only finalized transfers of the request&apos;s currency, as its
            issuer&apos;s exact contract on the chosen chain. Partial transfers accumulate. The
            request moves through <code>awaiting_deposit</code>, <code>partially_deposited</code>,
            and <code>deposited</code>, and webhooks report each step.
          </p>
        </Step>
        <Step title="Gum settles">
          <p>
            Exactly the requested amount goes to your payout address in one transaction. An
            overpayment remainder, an expired balance, or a late transfer goes back to the
            payer&apos;s attested wallet. The request is <code>settled</code>, and a Proof of
            Payment is available that anyone can verify offline.
          </p>
        </Step>
      </Steps>

      <H2 id="two-ways-in">Two ways in</H2>
      <Compare>
        <CompareItem title="The dashboard" badge={<Pill tone="green">no code</Pill>}>
          <p>
            Sign in with an emailed code. Set up the identity you issue under, create requests in
            four steps, share the link, and watch each one settle. Every account gets its own
            wallet, where deposits settle by default.
          </p>
        </CompareItem>
        <CompareItem title="The API and SDK" badge={<Pill>server-side</Pill>}>
          <p>
            One bearer key. Create requests from your own system, attach PDFs, keep customers,
            register webhooks, and download proofs. A zero-dependency TypeScript client wraps every
            call.
          </p>
        </CompareItem>
        <CompareItem title="Both together" badge={<Pill tone="yellow">typical</Pill>}>
          <p>
            Your application issues requests and listens for webhooks; your operations team follows
            along in the dashboard, which shows the same requests, the same statuses, and the same
            proofs.
          </p>
        </CompareItem>
      </Compare>

      <H2 id="what-gum-is-not">What Gum deliberately is not</H2>
      <ul>
        <li>
          <strong>Not a custodian.</strong> The requested amount moves from the one-time address to
          your wallet. Returns move from the address to the payer&apos;s wallet. Nothing sits with
          Gum, so there is nothing to withdraw and nothing to freeze.
        </li>
        <li>
          <strong>Not an invoicing system.</strong> A request has one amount, used exactly as given.
          There are no line items, tax fields, or fiat conversions. Attach a PDF when you need an
          itemised breakdown; Gum stores and hashes it but never parses it.
        </li>
      </ul>

      <Callout title="Currencies">
        USDC on every supported network, and USDT (as Tether&apos;s USDT0) on Monad and Arbitrum
        One. A request is denominated in one currency, USDC unless you say otherwise, and Gum
        never gives you a rate worse than 1:1: USDC bridges through Circle&apos;s CCTP at exactly
        1:1, so a USDC request may be paid on any network and withdrawn to whichever you choose;
        USDT has no such path, so a USDT request pins its network. Bridged or wrapped versions and
        look-alike tokens do not count. See <Link href="/docs/environments">Environments</Link>.
      </Callout>

      <Callout title="Free while in beta">
        Gum is free to use today. When pricing is introduced it will not disrupt an integration
        you have already built.
      </Callout>

      <H2 id="where-next">Where next</H2>
      <Cards>
        <Card href="/docs/quickstart" title="Quickstart">
          Issue your first deposit request in five minutes, from the shell or from TypeScript.
        </Card>
        <Card href="/docs/concepts" title="Deposit requests and deposits">
          The two primitives, every status, and where each unit ends up.
        </Card>
        <Card href="/docs/payer-verification" title="Verifying the payer">
          Choose between open links, emailed codes, and your own sign-in.
        </Card>
        <Card href="/docs/dashboard" title="Dashboard">
          Everything a merchant does day to day, without writing code.
        </Card>
        <Card href="/docs/api" title="API reference">
          Every route, field, and error code, with examples.
        </Card>
        <Card href="/docs/security" title="How Gum works">
          The architecture and the security properties, for due diligence.
        </Card>
      </Cards>
    </DocsPage>
  );
}
