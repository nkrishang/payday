import type { Withdrawal, WithdrawalLeg, WithdrawalTypedData } from "@payday/sdk";
import { encodeAbiParameters, hashTypedData, keccak256 } from "viem";
import { describe, expect, it } from "vitest";
import { checkLegAuthorization, withdrawalAuthorizationDefinition } from "./withdrawal-authorization";

/**
 * The same document `gateway-core` pins in
 * `withdrawal_authorization::tests::bridge_nonce_and_digest_vectors`, the
 * forwarder's Solidity test recomputes, and the SDK's signing test hashes:
 * a bridge leg for 1.234567 USDC on Monad towards 0x…d00d on Base.
 */
const DESTINATION = "0x000000000000000000000000000000000000d00d";
const FORWARDER = "0x2222222222222222222222222222222222222222";
const SALT = "0x0000000000000000000000000000000000000000000000000000000000000007";
const NONCE = "0x18b79105e486e10f626b71939a0226c47316949b7c24ff1c397661ea861fafaa";

const TYPED: WithdrawalTypedData = {
  domain: {
    name: "USDC",
    version: "2",
    chainId: 143,
    verifyingContract: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
  },
  primaryType: "ReceiveWithAuthorization",
  types: {
    EIP712Domain: [
      { name: "name", type: "string" },
      { name: "version", type: "string" },
      { name: "chainId", type: "uint256" },
      { name: "verifyingContract", type: "address" },
    ],
    ReceiveWithAuthorization: [
      { name: "from", type: "address" },
      { name: "to", type: "address" },
      { name: "value", type: "uint256" },
      { name: "validAfter", type: "uint256" },
      { name: "validBefore", type: "uint256" },
      { name: "nonce", type: "bytes32" },
    ],
  },
  message: {
    from: "0x1111111111111111111111111111111111111111",
    to: FORWARDER,
    value: "1234567",
    validAfter: "0",
    validBefore: "1800000000",
    nonce: NONCE,
  },
};

const LEG: WithdrawalLeg = {
  id: "wdl_1",
  kind: "bridge",
  source_chain: { id: "143", name: "Monad", native_symbol: "MON" },
  token: { symbol: "USDC", address: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603", decimals: 6 },
  amount: "1.234567",
  amount_base_units: "1234567",
  state: "awaiting_signature",
  authorization: {
    primary_type: "ReceiveWithAuthorization",
    typed_data: TYPED,
    expires_at: "2027-01-15T08:00:00Z",
    forwarder: FORWARDER,
    nonce_preimage: { destination_domain: 6, mint_recipient: DESTINATION, salt: SALT },
  },
  transfer_tx_hash: null,
  burn_tx_hash: null,
  mint_tx_hash: null,
  failure_reason: null,
};

const WITHDRAWAL: Withdrawal = {
  id: "wd_1",
  status: "awaiting_signature",
  wallet_address: "0x1111111111111111111111111111111111111111",
  currency: "USDC",
  destination: { chain: { id: "8453", name: "Base", native_symbol: "ETH" }, address: DESTINATION },
  legs: [LEG],
  created_at: "2027-01-14T08:00:00Z",
  completed_at: null,
  cancelled_at: null,
  failed_at: null,
};

describe("withdrawalAuthorizationDefinition", () => {
  it("hashes to the digest every implementation pins", () => {
    expect(hashTypedData(withdrawalAuthorizationDefinition(TYPED))).toBe(
      "0xfe0bcc7d9e69ee02881011f29a4156caa15e02b8e94c2e0c9f40e66711651993",
    );
  });

  it("agrees with the forwarder on the nonce the preimage commits to", () => {
    const recipient = `0x${DESTINATION.slice(2).padStart(64, "0")}` as `0x${string}`;
    expect(
      keccak256(
        encodeAbiParameters(
          [{ type: "uint32" }, { type: "bytes32" }, { type: "bytes32" }],
          [6, recipient, SALT],
        ),
      ),
    ).toBe(NONCE);
  });

  it("refuses a document of another type", () => {
    expect(() =>
      withdrawalAuthorizationDefinition({ ...TYPED, primaryType: "Permit" as "TransferWithAuthorization" }),
    ).toThrow(/unexpected typed data/);
  });
});

describe("checkLegAuthorization", () => {
  it("accepts the document the API describes", () => {
    expect(checkLegAuthorization(WITHDRAWAL, LEG)).toBeNull();
  });

  it("refuses a transfer leg that moves funds on another chain", () => {
    // The same leg as a same-chain transfer on Monad while the withdrawal
    // settles on Base: a legitimate token on a legitimate chain, but not
    // one the withdrawal can settle through.
    const straying: WithdrawalLeg = {
      ...LEG,
      kind: "transfer",
      authorization: {
        ...LEG.authorization!,
        primary_type: "TransferWithAuthorization",
        typed_data: {
          ...TYPED,
          primaryType: "TransferWithAuthorization",
          message: { ...TYPED.message, to: DESTINATION },
        },
        forwarder: null,
        nonce_preimage: null,
      },
    };
    expect(checkLegAuthorization(WITHDRAWAL, straying)).toMatch(/does not settle on/);
  });

  it("names what strayed", () => {
    const withMessage = (patch: Partial<WithdrawalTypedData["message"]>): WithdrawalLeg => ({
      ...LEG,
      authorization: {
        ...LEG.authorization!,
        typed_data: { ...TYPED, message: { ...TYPED.message, ...patch } },
      },
    });
    expect(checkLegAuthorization(WITHDRAWAL, withMessage({ to: DESTINATION }))).toMatch(/forwarder/);
    expect(checkLegAuthorization(WITHDRAWAL, withMessage({ value: "1" }))).toMatch(/amount/);
    expect(checkLegAuthorization(WITHDRAWAL, withMessage({ from: FORWARDER }))).toMatch(/Gum wallet/);
    expect(checkLegAuthorization(WITHDRAWAL, withMessage({ nonce: `0x${"99".repeat(32)}` }))).toMatch(/nonce/);
    expect(checkLegAuthorization(WITHDRAWAL, withMessage({ validBefore: "1" }))).toMatch(/expired/);
    expect(
      checkLegAuthorization(WITHDRAWAL, {
        ...LEG,
        authorization: {
          ...LEG.authorization!,
          forwarder: DESTINATION,
          typed_data: { ...TYPED, message: { ...TYPED.message, to: DESTINATION } },
        },
      }),
    ).toMatch(/trusted forwarder/);
    expect(
      checkLegAuthorization(WITHDRAWAL, {
        ...LEG,
        authorization: {
          ...LEG.authorization!,
          typed_data: { ...TYPED, domain: { ...TYPED.domain, verifyingContract: DESTINATION } },
        },
      }),
    ).toMatch(/trusted USDC/);
    expect(
      checkLegAuthorization(WITHDRAWAL, {
        ...LEG,
        authorization: {
          ...LEG.authorization!,
          nonce_preimage: { destination_domain: 3, mint_recipient: DESTINATION, salt: SALT },
        },
      }),
    ).toMatch(/destination network/);
    expect(
      checkLegAuthorization(WITHDRAWAL, {
        ...LEG,
        authorization: {
          ...LEG.authorization!,
          nonce_preimage: { destination_domain: 6, mint_recipient: FORWARDER, salt: SALT },
        },
      }),
    ).toMatch(/destination/);
    expect(
      checkLegAuthorization(WITHDRAWAL, { ...LEG, state: "authorized", authorization: null }),
    ).toMatch(/not awaiting/);
  });
});
