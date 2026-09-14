import type { Withdrawal, WithdrawalLeg, WithdrawalTypedData } from "@payday/sdk";
import { encodeAbiParameters, keccak256, type TypedDataDefinition } from "viem";
import { chainById } from "./config";

/**
 * The EIP-712 document the Payday wallet signs for one withdrawal leg, in
 * the shape viem's `signTypedData` / `hashTypedData` and Privy's
 * `useSignTypedData` take.
 *
 * The API hands the document over (`WithdrawalLeg.authorization.typed_data`)
 * and rebuilds it from its own record when the signature comes back, so the
 * page never edits it: every `uint256` arrives as a decimal string and is
 * widened to a bigint, nothing else changes. What the page does check,
 * before asking for a signature, is that the document is bound to the
 * browser's trusted deployment configuration and intended destination. These
 * checks are the merchant's security boundary, not a UX nicety: wallet UIs
 * are deliberately hidden while the dashboard signs.
 */

export const AUTHORIZATION_FIELDS = [
  { name: "from", type: "address" },
  { name: "to", type: "address" },
  { name: "value", type: "uint256" },
  { name: "validAfter", type: "uint256" },
  { name: "validBefore", type: "uint256" },
  { name: "nonce", type: "bytes32" },
] as const;

export const AUTHORIZATION_TYPES = {
  TransferWithAuthorization: AUTHORIZATION_FIELDS,
  ReceiveWithAuthorization: AUTHORIZATION_FIELDS,
} as const;

export type WithdrawalAuthorizationDefinition = TypedDataDefinition<
  typeof AUTHORIZATION_TYPES,
  "TransferWithAuthorization" | "ReceiveWithAuthorization"
>;

export function withdrawalAuthorizationDefinition(
  typed: WithdrawalTypedData,
): WithdrawalAuthorizationDefinition {
  if (
    typed.primaryType !== "TransferWithAuthorization" &&
    typed.primaryType !== "ReceiveWithAuthorization"
  ) {
    throw new Error(`unexpected typed data: ${typed.primaryType}`);
  }
  return {
    domain: {
      name: typed.domain.name,
      version: typed.domain.version,
      chainId: typed.domain.chainId,
      verifyingContract: typed.domain.verifyingContract as `0x${string}`,
    },
    types: AUTHORIZATION_TYPES,
    primaryType: typed.primaryType,
    message: {
      from: typed.message.from as `0x${string}`,
      to: typed.message.to as `0x${string}`,
      value: BigInt(typed.message.value),
      validAfter: BigInt(typed.message.validAfter),
      validBefore: BigInt(typed.message.validBefore),
      nonce: typed.message.nonce as `0x${string}`,
    },
  };
}

/**
 * The same document as `eth_signTypedData_v4` takes it, which is what Privy's
 * `signTypedData` forwards to the embedded wallet: plain JSON, every
 * `uint256` a decimal string, the primary type's fields listed once. Nothing
 * is converted, so what the wallet shows is what the API sent.
 */
export function privyTypedData(typed: WithdrawalTypedData): {
  domain: { name: string; version: string; chainId: number; verifyingContract: string };
  types: Record<string, Array<{ name: string; type: string }>>;
  primaryType: string;
  message: Record<string, string>;
} {
  if (
    typed.primaryType !== "TransferWithAuthorization" &&
    typed.primaryType !== "ReceiveWithAuthorization"
  ) {
    throw new Error(`unexpected typed data: ${typed.primaryType}`);
  }
  return {
    domain: { ...typed.domain },
    types: { [typed.primaryType]: AUTHORIZATION_FIELDS.map((field) => ({ ...field })) },
    primaryType: typed.primaryType,
    message: { ...typed.message },
  };
}

function same(a: string, b: string): boolean {
  return a.toLowerCase() === b.toLowerCase();
}

/**
 * The document must match the leg it is for: signed from the withdrawal's
 * wallet, under the source chain's domain, for the leg's amount, paying the
 * destination (transfer) or the forwarder the leg names (bridge), and, for a
 * bridge leg, committing to the destination through its nonce preimage.
 * Returns a reason when it does not, so the page can refuse to sign.
 */
export function checkLegAuthorization(withdrawal: Withdrawal, leg: WithdrawalLeg): string | null {
  const authorization = leg.authorization;
  if (!authorization) return "This leg is not awaiting a signature.";
  const typed = authorization.typed_data;
  const expectedType = leg.kind === "bridge" ? "ReceiveWithAuthorization" : "TransferWithAuthorization";
  if (typed.primaryType !== expectedType || authorization.primary_type !== expectedType) {
    return "The document is not the authorization type this leg needs.";
  }
  if (!same(typed.message.from, withdrawal.wallet_address)) {
    return "The document is not signed from your Payday wallet.";
  }
  if (typed.domain.chainId !== Number(leg.source_chain.id)) {
    return "The document is under another network's USDC.";
  }
  const source = chainById(leg.source_chain.id);
  if (!source) return "The source network is not trusted by this dashboard.";
  if (!same(typed.domain.verifyingContract, source.usdcAddress)) {
    return "The document is not under this network's trusted USDC contract.";
  }
  let validBefore: bigint;
  try {
    validBefore = BigInt(typed.message.validBefore);
  } catch {
    return "The document has an invalid expiry time.";
  }
  if (validBefore <= BigInt(Math.floor(Date.now() / 1000))) {
    return "The authorization has already expired.";
  }
  if (typed.message.value !== leg.amount_base_units || typed.message.validAfter !== "0") {
    return "The document does not authorize exactly this leg's amount.";
  }
  const payee = leg.kind === "bridge" ? authorization.forwarder : withdrawal.destination.address;
  if (!payee || !same(typed.message.to, payee)) {
    return leg.kind === "bridge"
      ? "The document does not pay the documented forwarder."
      : "The document does not pay your destination address.";
  }
  if (leg.kind === "bridge") {
    const preimage = authorization.nonce_preimage;
    if (!preimage || !same(preimage.mint_recipient, withdrawal.destination.address)) {
      return "The document does not commit the bridge to your destination address.";
    }
    if (!source.cctp) return "The source network has no trusted bridge configuration.";
    if (!authorization.forwarder || !same(authorization.forwarder, source.cctp.forwarder)) {
      return "The authorization does not use this network's trusted forwarder.";
    }
    const destination = chainById(withdrawal.destination.chain.id);
    if (!destination) return "The destination network is not trusted by this dashboard.";
    if (!destination.cctp) return "The destination network has no trusted bridge configuration.";
    if (preimage.destination_domain !== destination.cctp.domain) {
      return "The bridge authorization names the wrong destination network.";
    }
    const recipient = `0x${preimage.mint_recipient.slice(2).toLowerCase().padStart(64, "0")}` as `0x${string}`;
    const nonce = keccak256(
      encodeAbiParameters(
        [{ type: "uint32" }, { type: "bytes32" }, { type: "bytes32" }],
        [preimage.destination_domain, recipient, preimage.salt as `0x${string}`],
      ),
    );
    if (!same(nonce, typed.message.nonce)) {
      return "The signed nonce does not commit to the documented bridge destination.";
    }
  }
  return null;
}
