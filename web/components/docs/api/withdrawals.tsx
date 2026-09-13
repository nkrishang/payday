import { LEG_EXAMPLE, WITHDRAW_CURL } from "../withdrawals-prompt";
import type { EndpointGroup } from "./types";

const WITHDRAWAL = `{
  "id": "wd_0198f80c-7777-7dc1-a369-90556a64f700",
  "status": "awaiting_signature",
  "wallet_address": "0x1111111111111111111111111111111111111111",
  "destination": {
    "chain": { "id": "8453", "name": "Base", "native_symbol": "ETH" },
    "address": "0x000000000000000000000000000000000000d00d"
  },
  "legs": [
    ${LEG_EXAMPLE.replace(/\n/g, "\n    ")},
    {
      "id": "wdl_0198f80c-8888-7dc1-a369-90556a64f700",
      "kind": "transfer",
      "source_chain": { "id": "8453", "name": "Base", "native_symbol": "ETH" },
      "amount": "12.000000",
      "amount_base_units": "12000000",
      "state": "awaiting_signature",
      "authorization": { "primary_type": "TransferWithAuthorization", "typed_data": { "…": "…" }, "expires_at": "2027-01-15T08:00:00Z", "forwarder": null, "nonce_preimage": null },
      "transfer_tx_hash": null,
      "burn_tx_hash": null,
      "mint_tx_hash": null,
      "failure_reason": null
    }
  ],
  "created_at": "2027-01-14T08:00:00Z",
  "completed_at": null,
  "cancelled_at": null,
  "failed_at": null
}`;

const LEG_FIELDS = [
  { name: "id", type: "wdl_ id", description: "" },
  {
    name: "kind",
    type: "transfer | bridge",
    description: "Funds already on the destination chain, or crossing through CCTP.",
  },
  { name: "source_chain", type: "Chain", description: "" },
  { name: "amount, amount_base_units", type: "string", description: "The whole balance on that chain." },
  {
    name: "state",
    type: "string",
    description:
      "awaiting_signature, authorized, relaying, burned, attested, minting, completed, failed, expired, cancelled.",
  },
  {
    name: "authorization",
    type: "object | null",
    description:
      "While awaiting_signature: primary_type, typed_data (EIP-712, uint256 as decimal strings), expires_at, forwarder and nonce_preimage for bridge legs.",
  },
  {
    name: "transfer_tx_hash, burn_tx_hash, mint_tx_hash",
    type: "string | null",
    description: "Set as each transaction is finalized.",
  },
  { name: "failure_reason", type: "string | null", description: "Set when state is failed." },
];

export const WITHDRAWALS: EndpointGroup = {
  slug: "withdrawals",
  title: "Withdrawals",
  endpoints: [
    {
      slug: "create",
      title: "Create a withdrawal",
      method: "POST",
      path: "/v1/withdrawals",
      auth: "key",
      summary:
        "Snapshots the Payday wallet's USDC on every network into legs towards one destination. Nothing moves until each leg is signed.",
      body: (
        <p>
          One withdrawal may be open per account. Each leg&apos;s <code>authorization.typed_data</code>{" "}
          is the EIP-712 document to sign under that chain&apos;s USDC; see{" "}
          <a href="/docs/withdrawals#server">Withdrawals</a> for the checklist a signer
          applies before signing. A reused <code>Idempotency-Key</code> with the same destination
          replays the withdrawal with <code>Idempotency-Replayed: true</code>; with another
          destination it is <code>409 idempotency_conflict</code>.
        </p>
      ),
      headers: [{ name: "Idempotency-Key", type: "string", required: true, description: "1–255 bytes." }],
      bodyFields: [
        {
          name: "destination.chain_id",
          type: "string",
          required: true,
          description: "Decimal chain id of one of the deployment's networks.",
        },
        {
          name: "destination.address",
          type: "string",
          required: true,
          description: "Nonzero EVM address you control on that network. Stored EIP-55.",
        },
      ],
      response: {
        description: "Withdrawal, status awaiting_signature.",
        fields: [
          { name: "id", type: "wd_ id", description: "" },
          {
            name: "status",
            type: "string",
            description: "awaiting_signature, in_progress, completed, failed, cancelled.",
          },
          { name: "wallet_address", type: "string", description: "The Payday wallet every leg is signed from." },
          { name: "destination", type: "{ chain: Chain, address }", description: "" },
          { name: "legs", type: "Leg[]", description: "One per network holding USDC, in registry order." },
          ...LEG_FIELDS.map((field) => ({ ...field, name: `legs[].${field.name}` })),
          { name: "created_at, completed_at, cancelled_at, failed_at", type: "timestamp | null", description: "" },
        ],
      },
      answers: [
        { status: 409, code: "wallet_not_ready", when: "The account has not signed in to the dashboard yet, so its wallet is unknown." },
        { status: 409, code: "withdrawal_in_progress", when: "Another withdrawal is open; finish or cancel it." },
        { status: 409, code: "nothing_to_withdraw", when: "No USDC on any network." },
        { status: 503, code: "withdrawals_unavailable", when: "A balance could not be read, or a chain holding funds cannot bridge on this deployment." },
      ],
      examples: {
        curl: WITHDRAW_CURL,
        ts: `const withdrawal = await payday.withdrawals.create(
  { destination: { chain_id: "8453", address: "0x0000…d00d" } },
  crypto.randomUUID(),
);`,
        response: WITHDRAWAL,
        responseTitle: "201 Created",
      },
    },
    {
      slug: "authorize",
      title: "Submit signatures",
      method: "POST",
      path: "/v1/withdrawals/{id}/authorizations",
      auth: "key",
      summary:
        "Records the merchant's signatures over the legs' typed data. Every signature is verified against the wallet before any is stored.",
      pathParams: [{ name: "id", type: "wd_ id", required: true, description: "" }],
      bodyFields: [
        { name: "authorizations[].leg_id", type: "wdl_ id", required: true, description: "" },
        {
          name: "authorizations[].signature",
          type: "string",
          required: true,
          description: "0x hex, 65 bytes r || s || v, over the leg's typed_data.",
        },
      ],
      response: { description: "Withdrawal; in_progress once every leg is signed." },
      answers: [
        { status: 400, code: "signature_invalid", when: "The message names the leg. Nothing was recorded." },
        { status: 404, code: "withdrawal_leg_not_found", when: "" },
        { status: 409, code: "leg_not_awaiting_signature", when: "The leg already carries a different signature, or has moved on. Resending the same signature is a no-op." },
        { status: 409, code: "authorization_expired", when: "24 hours passed; create a new withdrawal." },
        { status: 409, code: "withdrawal_finished", when: "" },
      ],
      examples: {
        curl: `curl -fsS "$API/v1/withdrawals/wd_0198f80c-…/authorizations" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  -H "Content-Type: application/json" \\
  -d '{ "authorizations": [ { "leg_id": "wdl_0198f80c-…", "signature": "0x…" } ] }'`,
        ts: `import { privateKeySigner, signWithdrawal } from "@payday/sdk/signing";

const signer = await privateKeySigner(process.env.PAYDAY_WALLET_KEY!);
const { authorizations } = await signWithdrawal(withdrawal, signer, { chains });
const signed = await payday.withdrawals.authorize(withdrawal.id, authorizations);`,
        response: WITHDRAWAL.replace('"status": "awaiting_signature"', '"status": "in_progress"'),
      },
    },
    {
      slug: "get",
      title: "Get a withdrawal",
      method: "GET",
      path: "/v1/withdrawals/{id}",
      auth: "key",
      summary: "Retrieves a withdrawal with every leg's state and transaction hashes. Poll it every ten seconds while in_progress.",
      pathParams: [{ name: "id", type: "wd_ id", required: true, description: "" }],
      response: { description: "Withdrawal." },
      answers: [{ status: 404, code: "withdrawal_not_found", when: "" }],
      examples: {
        curl: `curl -fsS "$API/v1/withdrawals/wd_0198f80c-…" -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const withdrawal = await payday.withdrawals.get(id);`,
        response: WITHDRAWAL,
      },
    },
    {
      slug: "list",
      title: "List withdrawals",
      method: "GET",
      path: "/v1/withdrawals",
      auth: "key",
      summary: "Lists withdrawals, newest first.",
      query: [
        { name: "limit", type: "integer", description: "1–100, default 20." },
        { name: "starting_after", type: "wd_ id", description: "The last id of the previous page." },
      ],
      response: { description: "{ withdrawals: Withdrawal[], next_cursor: wd_ id | null }." },
      examples: {
        curl: `curl -fsS "$API/v1/withdrawals?limit=20" -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `const { withdrawals, next_cursor } = await payday.withdrawals.list({ limit: 20 });`,
        response: `{
  "withdrawals": [ ${WITHDRAWAL.replace(/\n/g, "\n    ")} ],
  "next_cursor": null
}`,
      },
    },
    {
      slug: "cancel",
      title: "Cancel a withdrawal",
      method: "POST",
      path: "/v1/withdrawals/{id}/cancel",
      auth: "key",
      summary: "Cancels while nothing has been relayed. Signatures already given are never used.",
      pathParams: [{ name: "id", type: "wd_ id", required: true, description: "" }],
      response: { description: "Withdrawal, status cancelled." },
      answers: [
        { status: 409, code: "withdrawal_not_cancellable", when: "A leg has been relayed; the withdrawal runs to completion." },
        { status: 409, code: "withdrawal_finished", when: "" },
      ],
      examples: {
        curl: `curl -fsS -X POST "$API/v1/withdrawals/wd_0198f80c-…/cancel" -H "Authorization: Bearer $PAYDAY_API_KEY"`,
        ts: `await payday.withdrawals.cancel(id);`,
        response: WITHDRAWAL.replace('"status": "awaiting_signature"', '"status": "cancelled"').replace('"cancelled_at": null', '"cancelled_at": "2027-01-14T08:05:00Z"'),
      },
    },
  ],
};
