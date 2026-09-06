import Link from "next/link";
import type { Metadata } from "next";
import { CodeBlock } from "@/components/docs/code";
import { Callout, DocsPage, H2, Table } from "@/components/docs/prose";

export const metadata: Metadata = {
  title: "Environments",
  description:
    "Production and sandbox: the base URLs, the chain and USDC contract each one settles on, and how the two differ.",
};

const SANDBOX = `curl -fsS "https://api.sandbox.payday.sh/v1/deposit-requests/dr_…" \\
  -H "Authorization: Bearer payday_test_..."`;

const SANDBOX_TS = `const payday = new PaydayClient({
  apiKey: process.env.PAYDAY_TEST_API_KEY!,
  baseUrl: "https://api.sandbox.payday.sh",
});`;

export default function EnvironmentsPage() {
  return (
    <DocsPage
      eyebrow="Using Payday"
      title="Environments"
      lead="Two isolated services with the same API. Production settles real USDC on Monad; the sandbox settles Circle's test USDC on Monad testnet, with its own accounts, keys, and database."
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
            <td>Chain</td>
            <td>Monad mainnet (chain id 143)</td>
            <td>Monad testnet (chain id 10143)</td>
          </tr>
          <tr>
            <td>Token</td>
            <td>Circle-issued native USDC</td>
            <td>Circle test USDC</td>
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

      <p>
        Every deposit request reports the <code>chain</code> and <code>token</code> it accepts. Read
        them from the response rather than hard-coding them, and show them to the payer: a matching
        symbol is not enough, and USDC on another network, bridged USDC, and look-alike tokens do
        not count.
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
          <code>GET /v1/status</code>, with a key, reports the chain&apos;s finalized position, the
          indexer&apos;s cursor and lag, and the settlement queue. See{" "}
          <Link href="/docs/api/account#get-status">Account and status</Link>.
        </li>
        <li>
          Every response carries <code>X-Request-Id</code>. Quote it to support.
        </li>
      </ul>
    </DocsPage>
  );
}
