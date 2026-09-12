/**
 * Signing a withdrawal from a server: `@payday/sdk/signing`.
 *
 * `POST /v1/withdrawals` returns one leg per network the Payday wallet holds
 * USDC on, each with EIP-712 typed data (an EIP-3009 authorization under
 * that chain's USDC). This module turns a withdrawal into the signed
 * `authorizations` array `client.withdrawals.authorize` takes, using any
 * signer that can sign typed data: viem's `privateKeyToAccount`, a KMS-backed
 * account, ethers' `Wallet` through a one-line adapter. `privateKeySigner`
 * builds one from the wallet key exported from the dashboard; it loads
 * `viem` on demand, which is why viem is an optional peer dependency and the
 * core client stays dependency-free.
 *
 * Before signing, `assertLegAuthorization` checks the document says what the
 * API says it does — the leg's amount, the destination or the documented
 * forwarder, and, for a bridge leg, the nonce's commitment to the destination
 * — so a signer never trusts the API's typed data blindly.
 */

import type { LegAuthorizationInput, Withdrawal, WithdrawalLeg, WithdrawalTypedData } from "./index.js";

/** Anything that can sign EIP-712 typed data for the Payday wallet. */
export interface WithdrawalSigner {
  signTypedData(typedData: SignableTypedData): Promise<string>;
}

/** `WithdrawalTypedData` with every `uint256` as a bigint, as viem and ethers take it. */
export interface SignableTypedData {
  domain: { name: string; version: string; chainId: number; verifyingContract: `0x${string}` };
  primaryType: WithdrawalTypedData["primaryType"];
  types: Record<string, Array<{ name: string; type: string }>>;
  message: {
    from: `0x${string}`;
    to: `0x${string}`;
    value: bigint;
    validAfter: bigint;
    validBefore: bigint;
    nonce: `0x${string}`;
  };
}

const HEX_ADDRESS = /^0x[0-9a-fA-F]{40}$/;
const HEX_32 = /^0x[0-9a-fA-F]{64}$/;

/** The document as a signer library takes it: numbers widened to bigint, the domain type dropped. */
export function toSignableTypedData(typed: WithdrawalTypedData): SignableTypedData {
  const { EIP712Domain: _domain, ...types } = typed.types;
  return {
    domain: {
      name: typed.domain.name,
      version: typed.domain.version,
      chainId: typed.domain.chainId,
      verifyingContract: typed.domain.verifyingContract as `0x${string}`,
    },
    primaryType: typed.primaryType,
    types,
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
 * Refuse to sign a document that does not match what the API says about the
 * leg: the payee must be the withdrawal's destination (transfer leg) or the
 * forwarder the leg names (bridge leg), the value the leg's amount, the
 * signer the withdrawal's wallet, `validAfter` zero, and the type the one the
 * leg claims. The nonce's commitment to the destination is checked by
 * `verifyBridgeNonce`, which needs keccak256 and therefore viem.
 */
export function assertLegAuthorization(withdrawal: Withdrawal, leg: WithdrawalLeg): WithdrawalTypedData {
  const authorization = leg.authorization;
  if (!authorization) throw new Error(`leg ${leg.id} is not awaiting a signature`);
  const typed = authorization.typed_data;
  const problem = (what: string) => new Error(`leg ${leg.id}: typed data ${what}`);
  if (typed.primaryType !== authorization.primary_type) throw problem("names another type than the leg");
  const expectedType = leg.kind === "bridge" ? "ReceiveWithAuthorization" : "TransferWithAuthorization";
  if (typed.primaryType !== expectedType) throw problem(`is a ${typed.primaryType} for a ${leg.kind} leg`);
  if (!sameAddress(typed.message.from, withdrawal.wallet_address)) throw problem("is not signed from the Payday wallet");
  if (typed.domain.chainId !== Number(leg.source_chain.id)) throw problem("is under another chain's domain");
  if (typed.message.value !== leg.amount_base_units) throw problem("does not authorize the leg's amount");
  if (typed.message.validAfter !== "0") throw problem("has a non-zero validAfter");
  if (!HEX_32.test(typed.message.nonce)) throw problem("has a malformed nonce");
  if (!HEX_ADDRESS.test(typed.domain.verifyingContract)) throw problem("has a malformed token address");
  const payee = leg.kind === "bridge" ? authorization.forwarder : withdrawal.destination.address;
  if (!payee || !sameAddress(typed.message.to, payee)) {
    throw problem(leg.kind === "bridge" ? "does not pay the documented forwarder" : "does not pay the destination");
  }
  if (leg.kind === "bridge") {
    const preimage = authorization.nonce_preimage;
    if (!preimage || !sameAddress(preimage.mint_recipient, withdrawal.destination.address)) {
      throw problem("does not commit to the destination");
    }
  }
  return typed;
}

/**
 * Signs every leg still awaiting its signature. Checks each document with
 * `assertLegAuthorization` first; pass `verifyNonce` (from `privateKeySigner`
 * or your own keccak) to also check a bridge leg's nonce commitment.
 */
export async function signWithdrawal(
  withdrawal: Withdrawal,
  signer: WithdrawalSigner,
  options: { verifyNonce?: (leg: WithdrawalLeg, typed: WithdrawalTypedData) => void } = {},
): Promise<{ authorizations: LegAuthorizationInput[] }> {
  const authorizations: LegAuthorizationInput[] = [];
  for (const leg of withdrawal.legs) {
    if (leg.state !== "awaiting_signature" || !leg.authorization) continue;
    const typed = assertLegAuthorization(withdrawal, leg);
    options.verifyNonce?.(leg, typed);
    const signature = await signer.signTypedData(toSignableTypedData(typed));
    authorizations.push({ leg_id: leg.id, signature });
  }
  return { authorizations };
}

/**
 * A signer from the Payday wallet's private key (export it once from the
 * dashboard and keep it in a secret manager). Loads viem on demand; install
 * `viem` alongside `@payday/sdk` to use it. The returned signer also
 * carries `verifyNonce`, which recomputes a bridge leg's nonce from its
 * documented preimage and refuses a document whose nonce differs.
 */
export async function privateKeySigner(
  privateKey: string,
): Promise<WithdrawalSigner & { address: string; verifyNonce: (leg: WithdrawalLeg, typed: WithdrawalTypedData) => void }> {
  const viem = await loadViem();
  const account = viem.accounts.privateKeyToAccount(privateKey as `0x${string}`);
  return {
    address: account.address,
    signTypedData: (typedData) => account.signTypedData(typedData),
    verifyNonce: (leg, typed) => verifyBridgeNonce(viem.core, leg, typed),
  };
}

/** `keccak256(abi.encode(uint32 destinationDomain, bytes32 mintRecipient, bytes32 salt))`, as the forwarder recomputes it. */
export function bridgeNonce(
  keccak: { keccak256: (data: `0x${string}`) => `0x${string}`; encodeAbiParameters: EncodeAbiParameters },
  destinationDomain: number,
  mintRecipient: string,
  salt: string,
): `0x${string}` {
  const recipient = `0x${mintRecipient.slice(2).toLowerCase().padStart(64, "0")}` as `0x${string}`;
  return keccak.keccak256(
    keccak.encodeAbiParameters(
      [{ type: "uint32" }, { type: "bytes32" }, { type: "bytes32" }],
      [destinationDomain, recipient, salt as `0x${string}`],
    ),
  );
}

type EncodeAbiParameters = (params: Array<{ type: string }>, values: unknown[]) => `0x${string}`;

function verifyBridgeNonce(
  keccak: { keccak256: (data: `0x${string}`) => `0x${string}`; encodeAbiParameters: EncodeAbiParameters },
  leg: WithdrawalLeg,
  typed: WithdrawalTypedData,
): void {
  if (leg.kind !== "bridge") return;
  const preimage = leg.authorization?.nonce_preimage;
  if (!preimage) throw new Error(`leg ${leg.id}: bridge leg without a nonce preimage`);
  const expected = bridgeNonce(keccak, preimage.destination_domain, preimage.mint_recipient, preimage.salt);
  if (expected.toLowerCase() !== typed.message.nonce.toLowerCase()) {
    throw new Error(`leg ${leg.id}: the nonce does not commit to the documented destination`);
  }
}

function sameAddress(a: string, b: string): boolean {
  return a.toLowerCase() === b.toLowerCase();
}

interface ViemModules {
  core: { keccak256: (data: `0x${string}`) => `0x${string}`; encodeAbiParameters: EncodeAbiParameters };
  accounts: {
    privateKeyToAccount: (key: `0x${string}`) => {
      address: `0x${string}`;
      signTypedData: (typedData: SignableTypedData) => Promise<`0x${string}`>;
    };
  };
}

async function loadViem(): Promise<ViemModules> {
  try {
    const [core, accounts] = await Promise.all([import("viem"), import("viem/accounts")]);
    return { core: core as unknown as ViemModules["core"], accounts: accounts as unknown as ViemModules["accounts"] };
  } catch (cause) {
    throw new Error("privateKeySigner needs the optional peer dependency viem: npm install viem", { cause });
  }
}
