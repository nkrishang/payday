import Link from "next/link";
import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { Callout, DocsPage, H2, Table } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Environments",
  description:
    "Production and sandbox: the base URLs, the currencies and networks each one settles on with their contracts, and how the two differ.",
};

const SANDBOX = `curl -fsS "https://api.sandbox.payday.sh/v1/deposit-requests/dr_…" \\
  -H "Authorization: Bearer payday_test_..."`;

const SANDBOX_TS = `const payday = new GumClient({
  apiKey: process.env.PAYDAY_TEST_API_KEY!,
  baseUrl: "https://api.sandbox.payday.sh",
});`;

export default function EnvironmentsPage() {
  return (
    <DocsPage
      eyebrow="Using Payday"
      title="Environments"
      lead="Two isolated services with the same API. Production settles USDC on Monad, Base, and Arbitrum One and USDT on Monad and Arbitrum One; the sandbox settles Circle's test USDC on their testnets, with its own accounts, keys, and database."
    >
      <Table>
        <thead>
          <tr>
            <th></th>
            <th>Production</th>
            <th>Sandbox</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>API</td>
            <td>
              <code>https://api.payday.sh</code>
            </td>
            <td>
              <code>https://api.sandbox.payday.sh</code>
            </td>
          </tr>
          <tr>
            <td>Keys</td>
            <td>
              <code>payday_live_…</code>
            </td>
            <td>
              <code>payday_test_…</code>
            </td>
          </tr>
          <tr>
            <td>Networks</td>
            <td>Monad (143), Base (8453), Arbitrum One (42161)</td>
            <td>Monad testnet (10143), Base Sepolia (84532), Arbitrum Sepolia (421614)</td>
          </tr>
          <tr>
            <td>Currencies</td>
            <td>USDC on every network; USDT (as USDT0) on Monad and Arbitrum One</td>
            <td>Circle test USDC on each network; USDT is not offered</td>
          </tr>
          <tr>
            <td>Funds</td>
            <td>Real</td>
            <td>Test tokens from a faucet</td>
          </tr>
          <tr>
            <td>Indexing and finality</td>
            <td>Real</td>
            <td>Real: the same indexer, the same finality gate</td>
          </tr>
          <tr>
            <td>OpenAPI</td>
            <td>
              <code>/openapi.json</code>
            </td>
            <td>
              <code>/openapi.json</code>
            </td>
          </tr>
        </tbody>
      </Table>

      <H2 id="currencies-and-networks">Currencies and networks</H2>
      <p>
        A deposit request is denominated in one <code>currency</code>, <code>USDC</code> by default
        or <code>USDT</code>. Production serves these contracts; all have six decimals.
      </p>
      <Table>
        <thead>
          <tr>
            <th>Network</th>
            <th>USDC</th>
            <th>USDT</th>
          </tr>
        </thead>
        <tbody>
          <tr>
            <td>Monad (143)</td>
            <td>
              <code>0x754704Bc059F8C67012fEd69BC8A327a5aafb603</code>
            </td>
            <td>
              <code>0xe7cd86e13AC4309349F30B3435a9d337750fC82D</code> (USDT0)
            </td>
          </tr>
          <tr>
            <td>Base (8453)</td>
            <td>
              <code>0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913</code>
            </td>
            <td>Not served</td>
          </tr>
          <tr>
            <td>Arbitrum One (42161)</td>
            <td>
              <code>0xaf88d065e77c8cC2239327C5EDb3A432268e5831</code>
            </td>
            <td>
              <code>0xFd086bC7CD5C481DCC9C85ebE478A1C0b69FCbb9</code> (USDT0)
            </td>
          </tr>
        </tbody>
      </Table>
      <p>
        USDC is Circle&apos;s native issuance on each network. USDT on Monad and Arbitrum One is
        Tether&apos;s USDT0, the omnichain USDT operated under Tether&apos;s license and backed 1:1
        by USDT locked on Ethereum; a wallet shows it as <code>USDT0</code>, which is what{" "}
        <code>token.symbol</code> carries there. Base&apos;s USDT is a bridge wrapper and is not
        served as a deposit currency. The sandbox serves Circle&apos;s test USDC only.
      </p>
      <p>
        The rule behind the matrix: Payday never gives you a rate worse than 1:1. USDC bridges
        through Circle&apos;s CCTP at exactly 1:1, so a USDC request may be paid on any network
        and withdrawn to whichever you choose. USDT has no such path, so a USDT request must pin{" "}
        <code>chain_id</code> to a network serving it (<code>400 invalid_request</code> naming{" "}
        <code>chain_id</code> otherwise; <code>422 unsupported_chain</code> for a network that does
        not serve it), and a USDT withdrawal moves the destination network&apos;s balance only.
      </p>
      <p>
        Every deposit request lists the <code>networks</code> it may be paid on, each with the
        currency&apos;s exact contract there. The payer picks one on the hosted checkout before
        signing, unless the request was created with <code>chain_id</code>, which pins the list to
        that network; from then on <code>chain</code> and <code>token</code> name it. Read them
        from the response rather than hard-coding them, and show them to the payer: a matching
        symbol is not enough, and the same currency on another network, a bridged or wrapped
        version, another stablecoin, and look-alike tokens do not count.
      </p>

      <H2 id="sandbox">Using the sandbox</H2>
      <p>
        Point the client at the sandbox origin with a test key. Everything else is the same: the
        routes, the fields, the statuses, the webhooks, the proof.
      </p>
      <CodeBlock code={SANDBOX_TS} lang="ts" />
      <CodeBlock code={SANDBOX} lang="bash" />
      <p>
        There is deliberately no &quot;mark as paid&quot; button. To settle a sandbox request, open
        its link as the payer, sign the wallet step, and send test USDC from that wallet. What you
        then watch is the real pipeline: finalized transfer, credit, settlement, proof.
      </p>

      <Callout title="Accounts do not cross environments">
        A sandbox account, its keys, its identities, and its requests exist only in the sandbox.
        Sign in to each separately and keep the two keys in separate secrets.
      </Callout>

      <H2 id="status-and-health">Status and health</H2>
      <ul>
        <li>
          <code>GET /health</code> is unauthenticated readiness: <code>ok</code> when the API can
          reach its database.
        </li>
        <li>
          <code>GET /v1/status</code>, with a key, reports each network&apos;s finalized position,
          the indexer&apos;s cursor and lag there, and its settlement queue. See{" "}
          <Link href="/docs/api/account#get-status">Account and status</Link>.
        </li>
        <li>
          Every response carries <code>X-Request-Id</code>. Quote it to support.
        </li>
      </ul>
    </DocsPage>
  );
}
