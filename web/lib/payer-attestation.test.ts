import type { PayerAttestationTypedData } from "@payday/sdk";
import { hashTypedData } from "viem";
import { describe, expect, it } from "vitest";
import { payerAttestationDefinition } from "./payer-attestation";

/**
 * The same document `gateway-core` pins in
 * `payer_attestation::tests::digest_is_pinned_and_typed_data_is_the_wallet_shape`.
 * The digest the wallet signs must be the digest the API recovers against,
 * and the API derives the payment address from it, so the two
 * implementations are held to one vector.
 */
const TYPED: PayerAttestationTypedData = {
  domain: {
    name: "Payday",
    version: "1",
    chainId: 143,
    verifyingContract: "0x5FbDB2315678afecb367f032d93F642f64180aa3",
  },
  primaryType: "PayerAttestation",
  types: {
    EIP712Domain: [
      { name: "name", type: "string" },
      { name: "version", type: "string" },
      { name: "chainId", type: "uint256" },
      { name: "verifyingContract", type: "address" },
    ],
    PayerAttestation: [
      { name: "statement", type: "string" },
      { name: "attributionHash", type: "bytes32" },
      { name: "wallet", type: "address" },
      { name: "nonce", type: "bytes32" },
      { name: "expiresAt", type: "uint256" },
    ],
  },
  message: {
    statement:
      "I control this wallet and will pay this Payday deposit request from it. Only transfers from this wallet count toward the request, and any funds Payday returns go back to it.",
    attributionHash: "0x8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a8a",
    // The wallet of the secp256k1 key 0x0707…07, as gateway-core's test signs.
    wallet: "0x4a62316623ad457F02cDC5D997deD67a383EC569",
    nonce: "0x1111111111111111111111111111111111111111111111111111111111111111",
    expiresAt: 1_900_000_000,
  },
};

describe("payerAttestationDefinition", () => {
  it("hashes to the digest gateway-core pins for the same document", () => {
    expect(hashTypedData(payerAttestationDefinition(TYPED))).toBe(
      "0x23f81e489d7192b8735c0d0c7866fbd8cd502c345b7dca546fc0bd29c0585392",
    );
  });

  it("refuses a document of another type", () => {
    expect(() =>
      payerAttestationDefinition({ ...TYPED, primaryType: "Other" as "PayerAttestation" }),
    ).toThrow(/unexpected typed data/);
  });
});
