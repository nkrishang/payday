import Link from "next/link";
import type { Metadata } from "next";
import { AddressDerivationDiagram, LifecycleDiagram } from "@/components/docs/diagrams";
import { RoutingFigure } from "@/components/docs/figures";
import {
  Callout,
  Compare,
  CompareItem,
  Def,
  Defs,
  DocsPage,
  Figure,
  H2,
  H3,
  Pill,
  StatusPill,
  Table,
} from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Deposit requests and deposits",
  description:
    "Payday's two primitives, the one-time address, every public status, and where each unit of USDC goes.",
};

export default function ConceptsPage() {
  return (
    <DocsPage
      eyebrow="Getting started"
      title="Deposit requests and deposits"
      lead="Payday has two primitives. A deposit request is the ask; a deposit is the funds that answer it. Everything else on this page follows from keeping the two apart."
    >
      <Compare>
        <CompareItem title="Deposit request" badge={<Pill tone="green">the document</Pill>}>
          <p>
            What you issue: an issuer and a payer, one amount, an optional heading, reference,
            notes, and metadata, a payer policy, a deadline, and at most one PDF. It is the API
            resource, it has a <code>dr_</code> id, and it is immutable once issued.
          </p>
        </CompareItem>
        <CompareItem title="Deposit" badge={<Pill tone="yellow">the funds</Pill>}>
          <p>
            What answers it on-chain: the one-time address, the USDC transfers that reach it, and
            the settlement that moves the amount to your wallet. You read its state from the
            request: <code>status</code>, <code>received</code>, <code>transfers</code>.
          </p>
        </CompareItem>
      </Compare>

      <p>
        Payday models no line items, quantities, discounts, tax, or fiat. The amount is
        authoritative. A PDF you attach is stored, shown to the payer, and hashed into the address,
        but never parsed or reconciled against the amount. To change anything about an issued
        request, cancel it and issue another; that is what keeps a Proof of Payment meaningful.
      </p>

      <Defs>
        <Def term="issuer">
          The party asking. Usually saved once as an{" "}
          <Link href="/docs/api/issuers">issuer identity</Link> with a proven contact mailbox, so
          the payer can write back.
        </Def>
        <Def term="payer">
          The party expected to pay. Optionally linked to a saved{" "}
          <Link href="/docs/api/customers">customer</Link>; the request still stores its own
          snapshot.
        </Def>
        <Def term="amount">
          A USDC decimal with up to six fractional digits. Exactly this settles to your payout
          address, no more and no less.
        </Def>
        <Def term="payout_address">
          Your wallet. By default the wallet Payday created for your account at sign-in; any EVM
          address you save works too.
        </Def>
        <Def term="payer_policy">
          Who may pay and what they prove first. See{" "}
          <Link href="/docs/payer-verification">Verifying the payer</Link>.
        </Def>
        <Def term="expires_at">
          The deadline, decided by chain time. Default 24 hours; 10 minutes to 366 days.
        </Def>
      </Defs>

      <H2 id="one-request-one-address-one-wallet">One request, one address, one payer wallet</H2>
      <p>
        Every deposit request gets its own EVM address, on one network, and that address belongs
        to one payer wallet. It does not exist when the request is issued. It exists once the
        payer, on the hosted checkout, chooses the network they will pay on and signs a short
        message from the wallet they intend to pay from. Payday derives the address from the
        issued document, the chosen chain, and that signature together, which is why{" "}
        <code>chain</code>, <code>token</code>, and <code>address</code> are <code>null</code>{" "}
        until then. The merchant never picks a network; the request offers every supported one.
      </p>

      <Figure caption="Where the one-time address comes from. The salt depends on both the document and the payer's signature, and the address commits to the settlement terms, so none of them can change afterwards.">
        <AddressDerivationDiagram />
      </Figure>

      <p>
        The address is <strong>counterfactual</strong>: it is calculated before any contract is
        deployed at it, so USDC can arrive the moment it is shown. When Payday later deploys and
        executes the contract, the funds can only move under the terms the address already commits
        to: this token, this amount, your payout address, this deadline, and the payer&apos;s wallet
        as the recovery destination. Anyone may execute it; nobody can redirect it.
      </p>
      <p>Two things follow, and both are worth internalising:</p>
      <ul>
        <li>
          <strong>Only transfers from the attested wallet are the payer&apos;s.</strong> USDC from
          any other wallet still counts toward the amount and settles, but the request is flagged{" "}
          <code>likely_unsolicited</code> and no Proof of Payment will claim the payer paid it.
        </li>
        <li>
          <strong>Everything Payday returns goes to the payer&apos;s own wallet, on-chain.</strong>{" "}
          An overpayment remainder, an expired balance, a late transfer. Nothing is held by Payday,
          and no one has to ask for a refund address.
        </li>
      </ul>

      <Callout tone="warning" title="Treat the address as single-use">
        Share the <code>deposit_url</code> rather than the bare address, stop presenting the address
        once the request is no longer payable, and never reuse it for another order. The hosted
        checkout does all three for you.
      </Callout>

      <H2 id="lifecycle">Lifecycle</H2>
      <p>
        A request has one public <code>status</code>. Clients should branch only on the values below
        and tolerate new fields.
      </p>

      <Figure caption="Every transition a deposit request can make. Cancellation is not a status: it is recorded on the request and asks clients to stop presenting it, but cannot disable the address.">
        <LifecycleDiagram />
      </Figure>

      <Table>
        <thead>
          <tr>
            <th>Status</th>
            <th>Meaning</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>
              <StatusPill tone="neutral">awaiting_deposit</StatusPill>
            </td>
            <td>No finalized, on-time USDC has been credited yet.</td>
          </tr>
          <tr>
            <td>
              <StatusPill tone="progress">partially_deposited</StatusPill>
            </td>
            <td>Some finalized USDC is credited, but less than the amount.</td>
          </tr>
          <tr>
            <td>
              <StatusPill tone="progress">deposited</StatusPill>
            </td>
            <td>
              Finalized credits reached the amount. Settlement is queued. Not yet payout finality;
              fulfil according to your own risk policy.
            </td>
          </tr>
          <tr>
            <td>
              <StatusPill tone="success">settled</StatusPill>
            </td>
            <td>
              Exactly the amount reached your payout address. Any remainder went back to the payer.
              The strongest state; the Proof of Payment is available.
            </td>
          </tr>
          <tr>
            <td>
              <StatusPill tone="neutral">expired</StatusPill>
            </td>
            <td>The deadline passed before settlement. The return to the payer is pending.</td>
          </tr>
          <tr>
            <td>
              <StatusPill tone="warning">returned</StatusPill>
            </td>
            <td>The whole balance went back to the payer&apos;s wallet after expiry.</td>
          </tr>
          <tr>
            <td>
              <StatusPill tone="warning">needs_attention</StatusPill>
            </td>
            <td>
              Automatic movement paused. Follow <code>attention.action</code> on the request, or
              contact support. Funds are neither lost nor delivered until an operator releases it.
            </td>
          </tr>
        </tbody>
      </Table>

      <p>
        Separate from the status, a request also reports <strong>verification</strong>: whether the
        payer policy has been satisfied, and by whom. A gated request can be funded before its payer
        has verified, which is why the two are different facts and filter separately.
      </p>

      <H2 id="where-the-usdc-goes">Where the USDC goes</H2>
      <p>
        Finalized transfers accumulate while a request is open. Chain time, not the payer&apos;s
        clock or the moment they pressed send, decides whether a transfer and the eventual execution
        are on time. Execution at exactly the deadline is on time; a later block is not.
      </p>

      <Figure caption="Every situation and its routing. Green is your payout address; yellow is the payer's attested wallet. Payday itself never appears, because it never holds the funds.">
        <RoutingFigure />
      </Figure>

      <p>
        Each return is recorded against its request in the <code>recovered_funds</code> ledger and
        raises a <code>deposit_request.recovered_funds</code> webhook naming the amount, the reason
        (<code>overpayment</code>, <code>expired</code>, or <code>late_transfer</code>), and the
        transaction. Use it to explain to a payer where a difference went.
      </p>

      <Callout tone="warning" title="Leave room before the deadline">
        Inclusion, finality, and the settlement transaction all take time. A payer who sends at the
        last minute may find the request expired by the time execution happens, and the whole
        balance returned. Ask payers not to pay at the boundary, and set deadlines with margin.
      </Callout>

      <H2 id="finality-and-freshness">Finality and freshness</H2>
      <p>
        Payday credits only <strong>finalized</strong> transfers of the configured Circle-issued
        USDC contract. A wallet may show a transaction as submitted, included, or confirmed before{" "}
        <code>received</code> changes. There is intentionally no endpoint to mark a request
        deposited by hand.
      </p>
      <p>Every read of a request tells you how current it is:</p>
      <ul>
        <li>
          <code>as_of</code>: the block and time through which the request&apos;s state is
          committed;
        </li>
        <li>
          <code>indexer_freshness</code>: the last indexed and finalized block positions;
        </li>
        <li>
          <code>transfers</code>: every finalized transfer to the address with sender, transaction,
          amount, block, and whether it was credited, late, or zero-value.
        </li>
      </ul>
      <p>
        The authenticated <Link href="/docs/api/account#get-status">status route</Link> reports the
        same for the service as a whole.
      </p>

      <H3 id="amounts">Amounts</H3>
      <p>
        USDC has six decimals. Every amount is returned twice: as a decimal string (
        <code>&quot;25.000000&quot;</code>) for display, and as an integer string of base units (
        <code>&quot;25000000&quot;</code>) for arithmetic. Do arithmetic on base units with integer
        or decimal types; never with binary floating point.
      </p>

      <H2 id="proof-of-payment">Proof of Payment</H2>
      <p>
        A settled request can be exported as a Proof of Payment: the canonical document, the
        payer&apos;s wallet attestation, the salt, the addresses, the credited transfers, the
        settlement transaction, and a Payday-signed statement of the verification facts. From it
        anyone can recompute the address and check the transfers with no access to Payday. See{" "}
        <Link href="/docs/proof-of-payment">Proof of Payment</Link>.
      </p>

      <H2 id="safety-boundaries">Safety boundaries</H2>
      <ul>
        <li>
          Only the exact <code>token.address</code> on the chosen <code>chain.id</code> is
          monitored. Bridged USDC, look-alike tokens, and native gas do not count and may be
          unrecoverable. The address commits to its chain: on any other network it refuses to
          settle, and the funds can only be returned to the payer&apos;s wallet, by hand.
        </li>
        <li>
          A deposit link grants read access to the payer page and nothing else. It never exposes
          your payout address, metadata, customer, or policy assertions.
        </li>
        <li>
          <code>needs_attention</code> pauses settlement and recovery, including for later
          transfers, until an operator resolves it.
        </li>
      </ul>
    </DocsPage>
  );
}
