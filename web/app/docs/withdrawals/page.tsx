import Link from "next/link";
import type { Metadata } from "next";
import { CopyPromptButton } from "@/components/ui/copy-prompt-button";
import { CodeBlock } from "@/components/docs/code";
import { CodeTabs } from "@/components/docs/code-tabs";
import { Callout, DocsPage, H2, H3, Step, Steps, Table } from "@/components/docs/prose";
import {
  CHECKLIST,
  LEG_EXAMPLE,
  VECTOR_DIGEST,
  VECTOR_NONCE,
  WITHDRAW_CURL,
  WITHDRAW_GO,
  WITHDRAW_RUST,
  WITHDRAW_TS,
  WITHDRAWALS_PROMPT,
} from "@/components/docs/withdrawals-prompt";
import { config } from "@/lib/config";

export const metadata: Metadata = {
  title: "Withdrawals",
  description:
    "Move the Gum wallet's USDC or USDT to one address: from the dashboard, or from your own server with the wallet key.",
};

const EXPORT_NOTE = `GUM_WALLET_KEY=0x…   # exported once from the dashboard; store it in a secret manager`;

export default function WithdrawalsPage() {
  const bridging = config.chains.filter((chain) => chain.cctp !== null);
  return (
    <DocsPage
      eyebrow="Using Gum"
      title="Withdrawals"
      lead="Deposits settle to the Gum wallet on whichever network the payer chose. A withdrawal moves everything it holds in one currency to one address you name, with nothing deducted: USDC from every network at once, USDT from the destination network alone. Gum relays and pays the gas; your signature decides where each leg's funds may land."
    >
      <H2 id="how">How a withdrawal works</H2>
      <p>
        The Gum wallet is an ordinary account with the same address on every network, and its
        key is yours: Privy holds it for you, Gum never has it. So Gum cannot simply move
        your funds, and by design it cannot. Instead a withdrawal is four steps, the same over the
        API as in the dashboard:
      </p>
      <Steps>
        <Step title="Prepare">
          <code>POST /v1/withdrawals</code> names a <code>currency</code> (USDC by default) and a
          destination. For USDC it reads the wallet&apos;s balance on every network and answers
          with one <em>leg</em> per network that holds any: a <code>transfer</code> when the funds
          already sit on the destination network, a <code>bridge</code> when they cross through
          Circle&apos;s CCTP. For USDT it answers with one transfer leg, on the destination network.
          Each leg names its <code>token</code> and carries the EIP-712 document to sign.
        </Step>
        <Step title="Sign">
          The document is an EIP-3009 authorization under the leg&apos;s token, the currency&apos;s
          contract on that network:{" "}
          <code>TransferWithAuthorization</code> naming your destination as the payee, or{" "}
          <code>ReceiveWithAuthorization</code> naming Gum&apos;s <code>WithdrawalForwarder</code>{" "}
          as the payee with a nonce that commits to your destination. The dashboard signs with
          the wallet Privy holds; a server signs with the exported key.
        </Step>
        <Step title="Submit">
          <code>POST /v1/withdrawals/&#123;id&#125;/authorizations</code> takes the signatures.
          Gum verifies each against your wallet and hands the legs to its relayer.
        </Step>
        <Step title="Poll">
          <code>GET /v1/withdrawals/&#123;id&#125;</code> follows every leg to the destination:
          the transfer, or the burn, Circle&apos;s attestation, and the mint.
        </Step>
      </Steps>
      <Callout title="Why the relayer cannot redirect anything">
        EIP-3009&apos;s <code>transferWithAuthorization</code> pays exactly the payee in the
        signature. The forwarder is the payee of a bridge leg, and USDC only lets the payee itself
        submit a{" "}
        <code>receiveWithAuthorization</code>; the forwarder then calls CCTP with the recipient
        it was told, after recomputing the authorization&apos;s nonce from that recipient. Told a
        different recipient, it computes a nonce you never signed, and the token reverts before
        any funds move. CCTP mints to the recipient inside Circle&apos;s attested message. There
        is no step at which a relayer chooses a destination.
      </Callout>

      <H2 id="timing">Timing and cost</H2>
      <Table>
        <thead>
          <tr>
            <th>Leg</th>
            <th>What happens</th>
            <th>Typical time</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>Transfer</td>
            <td>One <code>transferWithAuthorization</code> on the destination network.</td>
            <td>Seconds</td>
          </tr>
          <tr>
            <td>Bridge from Monad</td>
            <td>Burn, Circle attests once the block is final, mint on the destination.</td>
            <td>Under a minute</td>
          </tr>
          <tr>
            <td>Bridge from Base or Arbitrum</td>
            <td>Same, but Circle waits for Ethereum finality (~65 blocks) before attesting.</td>
            <td>15–20 minutes</td>
          </tr>
        </tbody>
      </Table>
      <p>
        You receive the full amount: CCTP&apos;s standard transfer has no protocol fee, and
        Gum pays every transaction&apos;s gas. One withdrawal may be open per account; its
        authorizations are valid for 24 hours, and a leg left unsigned that long expires.
      </p>
      <Callout title="Circle's bridge limit">
        One CCTP burn cannot exceed 10,000,000 USDC. Since Gum withdraws the whole balance,
        withdraw a network holding more than that to an address on the same network rather than
        bridging it. The dashboard and API refuse an oversized bridge leg.
      </Callout>

      <H2 id="only-usdc-bridges">Only USDC bridges</H2>
      <p>
        Gum never gives you a rate worse than 1:1, and CCTP is the one bridge that holds to it.
        So bridge legs exist for USDC alone, and Circle&apos;s per-message limit applies to them
        alone. A USDT withdrawal is one <code>transfer</code> leg on the destination network, which
        must serve USDT (Monad or Arbitrum One; <code>400 invalid_request</code> otherwise); USDT
        on another network is withdrawn separately, to an address on that network, and{" "}
        <code>409 nothing_to_withdraw</code> names those networks when the destination holds none.
      </p>
      <p>
        A leg&apos;s document is signed under its <code>token</code>. The EIP-712 domain of a USDT0
        leg is <code>name = &quot;USDT0&quot;</code> on Monad or <code>&quot;USD₮0&quot;</code> on
        Arbitrum One with <code>version = &quot;1&quot;</code>; USDC stays{" "}
        <code>&quot;USDC&quot;</code> on Monad or <code>&quot;USD Coin&quot;</code> elsewhere with{" "}
        <code>version = &quot;2&quot;</code>. Read the domain from the leg rather than hard-coding
        it, but check its <code>verifyingContract</code> against your own registry: the SDK
        signer&apos;s trusted registry is{" "}
        <code>{`{ tokens: { USDC: address, USDT?: address }, cctp }`}</code> per chain, it checks
        every leg against the entry for the withdrawal&apos;s currency, and it refuses a bridge leg
        for any currency but USDC.
      </p>

      <H2 id="dashboard">From the dashboard</H2>
      <p>
        The <Link href="/docs/dashboard#account-and-api-key">Account section</Link> shows the
        wallet&apos;s balance in each stablecoin on each network and a <strong>Withdraw</strong>{" "}
        control beneath them. Pick the currency, then the destination network among those serving
        it, enter the address, review the legs, and sign once per leg. The page checks each
        document against its leg before it asks the wallet, then tracks every leg until it lands;
        you can leave while a bridge waits for Circle.
      </p>

      <H2 id="server">From your server</H2>
      <p>
        A server needs the wallet&apos;s key. Export it once from the Account section (
        <strong>Export wallet key</strong>): Privy shows it to you on its own origin, and Gum
        never sees it. Anyone holding it controls the wallet, so keep it in a secret manager next
        to your API key and rotate the API key if either leaks.
      </p>
      <CodeBlock code={EXPORT_NOTE} lang="bash" title=".env" />
      <p>
        Then the four steps, in the official SDK or in any language with an HTTP client and an
        EIP-712 library. Every <code>uint256</code> in the document is a decimal string, the nonce
        is <code>0x</code> hex, and the signature is 65 bytes <code>r || s || v</code> with{" "}
        <code>v</code> 27 or 28, sent as <code>0x</code> hex.
      </p>
      <CodeTabs
        tabs={[
          { label: "TypeScript", content: <CodeBlock code={WITHDRAW_TS} lang="ts" /> },
          { label: "Rust", content: <CodeBlock code={WITHDRAW_RUST} lang="rust" /> },
          { label: "Go", content: <CodeBlock code={WITHDRAW_GO} lang="go" /> },
          { label: "curl", content: <CodeBlock code={WITHDRAW_CURL} lang="bash" /> },
        ]}
      />
      <p>
        Another language? Hand your agent the whole specification, examples included, and let it
        build the same four steps there.
      </p>
      <CopyPromptButton
        prompt={WITHDRAWALS_PROMPT}
        label="Copy the specification for your AI agent"
        className="border-line bg-surface hover:border-muted"
      />

      <H3 id="verify">What to check before signing</H3>
      <p>
        The document says where funds go, so a signer should trust the leg, not the document. The
        SDK&apos;s <code>signWithdrawal</code> and the dashboard both refuse a document that fails
        any of these; do the same:
      </p>
      <ul>
        {CHECKLIST.map((item) => (
          <li key={item}>{item}</li>
        ))}
      </ul>
      <p>
        The forwarder on each network:
      </p>
      <Table>
        <thead>
          <tr>
            <th>Network</th>
            <th>CCTP domain</th>
            <th>WithdrawalForwarder</th>
          </tr>
        </thead>
        <tbody>
          {bridging.map((chain) => (
            <tr key={chain.id}>
              <td>{chain.name}</td>
              <td>{chain.cctp?.domain}</td>
              <td>
                <code>{chain.cctp?.forwarder}</code>
              </td>
            </tr>
          ))}
          {bridging.length === 0 ? (
            <tr>
              <td colSpan={3}>This deployment bridges nothing; withdrawals stay on their network.</td>
            </tr>
          ) : null}
        </tbody>
      </Table>

      <H3 id="vector">Test vector</H3>
      <p>
        A bridge leg exactly as the API returns it. Its document hashes to the EIP-712 digest{" "}
        <code>{VECTOR_DIGEST}</code>, and its nonce <code>{VECTOR_NONCE}</code> is{" "}
        <code>keccak256(abi.encode(uint32 6, bytes32(0x…d00d), bytes32(0x…07)))</code> over the
        preimage. Gum&apos;s Rust, Solidity, TypeScript and browser code all pin these two
        values; an implementation that reproduces them signs what Gum expects.
      </p>
      <CodeBlock code={LEG_EXAMPLE} lang="json" title="A bridge leg" />

      <H2 id="states">States</H2>
      <Table>
        <thead>
          <tr>
            <th>Leg state</th>
            <th>Meaning</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td><code>awaiting_signature</code></td>
            <td>Prepared; <code>authorization</code> is present.</td>
          </tr>
          <tr>
            <td><code>authorized</code></td>
            <td>Signed and queued for the relayer.</td>
          </tr>
          <tr>
            <td><code>relaying</code></td>
            <td>The transfer or burn is in flight.</td>
          </tr>
          <tr>
            <td><code>burned</code>, <code>attested</code>, <code>minting</code></td>
            <td>Bridge legs only: burned and waiting for Circle; attested and queued to mint; mint in flight.</td>
          </tr>
          <tr>
            <td><code>completed</code></td>
            <td>Landed. <code>transfer_tx_hash</code> or <code>burn_tx_hash</code> + <code>mint_tx_hash</code> name the transactions.</td>
          </tr>
          <tr>
            <td><code>failed</code>, <code>expired</code>, <code>cancelled</code></td>
            <td>Terminal without landing; <code>failure_reason</code> says why. Funds stay where the last successful step left them.</td>
          </tr>
        </tbody>
      </Table>
      <p>
        The withdrawal itself is <code>awaiting_signature</code> while any leg needs a
        signature, <code>in_progress</code> while the relayer works, then{" "}
        <code>completed</code>, <code>failed</code> or <code>cancelled</code>. The API reference
        starts at <Link href="/docs/api/withdrawals/create">Create a withdrawal</Link>.
      </p>
    </DocsPage>
  );
}
