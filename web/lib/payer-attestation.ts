import type { PayerAttestationTypedData } from "@payday/sdk";
import type { TypedDataDefinition } from "viem";

/**
 * The EIP-712 document a payer's wallet signs to bind itself to a deposit
 * request, in the shape viem's `signTypedData` / `hashTypedData` take.
 *
 * The API hands the document over verbatim (`WalletChallenge.typed_data`)
 * and rebuilds it from its own record when the signature comes back, so the
 * page never edits it: only the wallet address it names came from here, and
 * that went to the API before the challenge was minted. `expiresAt` is a
 * `uint256` and arrives as a JSON number; viem wants it as a bigint.
 */
export const PAYER_ATTESTATION_TYPES = {
  PayerAttestation: [
    { name: "statement", type: "string" },
    { name: "attributionHash", type: "bytes32" },
    { name: "wallet", type: "address" },
    { name: "nonce", type: "bytes32" },
    { name: "expiresAt", type: "uint256" },
  ],
} as const;

export type PayerAttestationDefinition = TypedDataDefinition<
  typeof PAYER_ATTESTATION_TYPES,
  "PayerAttestation"
>;

export function payerAttestationDefinition(
  typed: PayerAttestationTypedData,
): PayerAttestationDefinition {
  if (typed.primaryType !== "PayerAttestation") {
    throw new Error(`unexpected typed data: ${typed.primaryType}`);
  }
  return {
    domain: {
      name: typed.domain.name,
      version: typed.domain.version,
      chainId: typed.domain.chainId,
      verifyingContract: typed.domain.verifyingContract as `0x${string}`,
    },
    types: PAYER_ATTESTATION_TYPES,
    primaryType: "PayerAttestation",
    message: {
      statement: typed.message.statement,
      attributionHash: typed.message.attributionHash as `0x${string}`,
      wallet: typed.message.wallet as `0x${string}`,
      nonce: typed.message.nonce as `0x${string}`,
      expiresAt: BigInt(typed.message.expiresAt),
    },
  };
}
