import Link from "next/link";
import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { CheckoutMock } from "@/components/docs/figures";
import { Callout, DocsPage, Figure, H2, H3, Table } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Hosted checkout",
  description:
    "What the payer sees on a Payday deposit link, how the three ways to pay work, and how to build your own checkout on the public payer routes.",
};

const PAYER_READ = `import { PaydayPayerClient } from "@payday/sdk";

const payer = new PaydayPayerClient(); // no key: these routes are public

const request = await payer.depositRequests.get("dr_0198f80c-…");
request.issuer_name;          // always present, with heading
request.content_unlocked;     // false while a gated request awaits verification
request.requirements;         // { email, wallet, merchant_session, complete }
request.remaining_base_units; // the only value to do arithmetic on
request.address;              // null until the payer's wallet is bound
request.deposit_uri;          // EIP-681 request for the amount still due
request.payable;              // false once the address must stop being shown
request.server_timestamp;     // render the deadline from this, not the device clock`;

const WALLET_STEP = `import { createWalletClient, custom } from "viem";

const wallet = createWalletClient({ transport: custom(window.ethereum) });
const [account] = await wallet.getAddresses();

// 1. The payer picks one of the request's networks. The choice is final once
//    signed: the address will exist on that chain only.
const network = request.networks.find((n) => n.chain.id === chosenChainId);
await wallet.switchChain({ id: Number(network.chain.id) });

// 2. Ask Payday for the document to sign on that chain. A permissionless
//    request with no session yet gets one here; a gated request needs the
//    session that satisfied its policy.
const challenge = await payer.wallet.challenge(request.id, account, network.chain.id, {
  payerSession,
});

// 3. The wallet shows the typed data in full and signs it. No transaction.
//    Its domain names the chosen chain, so a wallet on another one refuses.
const signature = await wallet.signTypedData({ account, ...challenge.typed_data });

// 4. Hand the signature back. The response is the unlocked request with
//    \`chain\`, \`token\`, \`address\`, and \`payer_wallet\` set.
const ready = await payer.wallet.attest(request.id, account, signature, challenge.payer_session);`;

const EMAIL_STEP = `// The code goes to the mailbox the merchant asserted; the payer only types it.
const { payer_session } = await payer.verification.startEmail(request.id);
const { requirements } = await payer.verification.confirmEmail(request.id, "123456", payer_session);
// requirements.complete is now true; pass payer_session on every later read.`;

export default function CheckoutPage() {
  return (
    <DocsPage
      eyebrow="Using Payday"
      title="Hosted checkout"
      lead="Every deposit_url points at a page Payday hosts. It shows the payer exactly what they need, withholds what they should not see, and offers three ways to pay the same one-time address."
    >
      <Figure caption="The same request before and after its payer verifies and signs. A locked page renders nothing it was not sent: the withheld fields are absent from the HTML, not hidden by it.">
        <CheckoutMock />
      </Figure>

      <H2 id="what-the-payer-sees">What the payer sees</H2>
      <ul>
        <li>
          <strong>The request</strong>: who is asking, the heading and reference, the payer&apos;s
          name, notes, and the attached PDF through a short-lived link.
        </li>
        <li>
          <strong>The network step</strong>: the networks the request may be paid on (Monad, Base,
          Arbitrum One), each with its gas token and rough confirmation time. Choosing one is
          mandatory before the wallet step, and the choice is final once signed.
        </li>
        <li>
          <strong>The instructions</strong>: the amount still due, the chosen network and the exact
          token contract there, the one-time address, and a QR code encoding the same request.
        </li>
        <li>
          <strong>The clock</strong>: a countdown to the deadline, driven by Payday&apos;s clock,
          never the device&apos;s. When it reaches zero the page waits for the chain&apos;s verdict
          rather than declaring the request expired itself.
        </li>
        <li>
          <strong>Live status</strong>: the page polls and moves on its own as finalized transfers
          arrive, then shows the settlement transaction once the request settles.
        </li>
      </ul>
      <p>
        For a gated request only the issuer name and heading show until the payer&apos;s session
        satisfies the policy; see <Link href="/docs/payer-verification">Verifying the payer</Link>.
        For every request, the address, the QR, and the pay button appear only after the network
        and wallet steps.
      </p>

      <H2 id="three-ways-to-pay">Three ways to pay</H2>
      <Table>
        <thead>
          <tr>
            <th>Way</th>
            <th>How it works</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>Connected wallet</td>
            <td>
              The page discovers installed browser wallets and, with WalletConnect, phone wallets.
              It switches the wallet to the chosen chain, checks the token contract, then sends a
              plain USDC transfer of the
              amount still due, re-read at the moment of signing. No approval, no contract call. It
              refuses to send from any wallet but the attested one.
            </td>
          </tr>
          <tr>
            <td>Scanned QR</td>
            <td>
              An EIP-681 request for the amount still due, which a mobile wallet turns into a
              pre-filled transfer.
            </td>
          </tr>
          <tr>
            <td>Copied address</td>
            <td>
              The address in full, for a wallet or exchange withdrawal. The payer must send from the
              wallet they signed with.
            </td>
          </tr>
        </tbody>
      </Table>
      <p>
        All three stop being offered the moment the request is no longer payable, because funds sent
        afterwards route back to the payer rather than to you.
      </p>

      <Callout tone="warning" title="What the payer must get right">
        Exactly the displayed amount, of the exact USDC contract, on the network they chose, from
        the wallet they signed with, and not at the deadline boundary. Bridged USDC, look-alike
        tokens, another network, and native gas do not count. The address refuses to settle on any
        other chain, so a wrong-network deposit is not lost, but returning it is a manual
        support case. The checkout says all
        of this; if you build your own, repeat it.
      </Callout>

      <H2 id="the-link">The link</H2>
      <p>
        A deposit link is open by design: anyone holding it may read the request and fulfil it,
        which is what makes it shareable. It carries no merchant data. Your payout address, the
        recovery address, metadata, the customer, and the policy assertions never reach the page.
        Search engines are told not to index it.
      </p>
      <p>
        When the request names a <code>payer.email</code>, Payday emails that address as the request
        is issued: a message in Payday&apos;s design naming the issuer, the amount, the heading and
        reference, and the deadline, with a button to the same link. Merchant-session requests are
        never emailed, since their link opens only from your application.
      </p>

      <H2 id="build-your-own">Building your own checkout</H2>
      <p>
        The hosted page is built on Payday&apos;s public payer routes, and so can yours. They take
        no API key, return no merchant data, and answer reads from any browser origin. The
        verification and wallet writes are answered for the hosted checkout&apos;s origin only, so a
        custom checkout drives those from a server or asks Payday to admit its origin.
      </p>
      <CodeBlock code={PAYER_READ} lang="ts" title="Reading a request" />

      <H3 id="email-verification">Email verification</H3>
      <CodeBlock code={EMAIL_STEP} lang="ts" />

      <H3 id="wallet-step">The wallet step</H3>
      <p>
        Every request, gated or not, takes this step before it has an address. The payer chooses a
        network first; the typed data comes from Payday for that chain and goes to the wallet
        verbatim.
      </p>
      <CodeBlock code={WALLET_STEP} lang="ts" />
      <p>
        Then render <code>address</code>, <code>deposit_uri</code>, and the QR from{" "}
        <code>GET /v1/payer/deposit-requests/&#123;id&#125;/qr</code>, and poll the request until it
        settles. The routes are listed under <Link href="/docs/api/payer">Payer routes</Link>.
      </p>
      <Callout title="Rules the hosted page follows, which yours should too">
        <p>
          Never let the device clock decide that a request expired; chain time does. Hide the
          address the moment <code>payable</code> is false. Do arithmetic on base units only. Show
          only what the API sent, so a gated request&apos;s withheld content cannot leak by
          accident. And send the session token in the <code>Payday-Payer-Session</code> header,
          never in a URL.
        </p>
      </Callout>
    </DocsPage>
  );
}
