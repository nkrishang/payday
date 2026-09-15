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
 * The checks in this module are the signer's security boundary, not a UX
 * convenience: bridge documents are signed only after their nonce commitment,
 * trusted USDC, CCTP domain, and deployed forwarder have all been verified.
 */

import type { LegAuthorizationInput, Withdrawal, WithdrawalLeg, WithdrawalTypedData } from "./index.js";

/** Anything that can sign EIP-712 typed data for the Payday wallet. */
export interface WithdrawalSigner {
  signTypedData(typedData: SignableTypedData): Promise<string>;
}

export type EncodeAbiParameters = (params: Array<{ type: string }>, values: unknown[]) => `0x${string}`;
export interface WithdrawalKeccak {
  keccak256(data: `0x${string}`): `0x${string}`;
  encodeAbiParameters: EncodeAbiParameters;
}
export interface TrustedWithdrawalChain {
  /** The trusted contract per currency code (`USDC`, `USDT`) on this chain. */
  tokens: Record<string, string>;
  /** Circle's CCTP on this chain; only USDC ever bridges through it. */
  cctp: { domain: number; forwarder: string } | null;
}
export interface SignWithdrawalOptions {
  keccak?: WithdrawalKeccak;
  chains?: (chainId: number) => TrustedWithdrawalChain | null;
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
  if (BigInt(typed.message.validBefore) <= BigInt(Math.floor(Date.now() / 1000))) {
    throw problem("has already expired");
  }
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
 * Signs every leg still awaiting its signature. All documents are checked
 * before the first signature. Bridge legs require a trusted chain registry;
 * nonce verification is mandatory and uses `options.keccak` or lazy-loaded
 * viem.
 */
export async function signWithdrawal(
  withdrawal: Withdrawal,
  signer: WithdrawalSigner,
  options: SignWithdrawalOptions = {},
): Promise<{ authorizations: LegAuthorizationInput[] }> {
  const awaiting = withdrawal.legs.filter(
    (leg): leg is WithdrawalLeg & { authorization: NonNullable<WithdrawalLeg["authorization"]> } =>
      leg.state === "awaiting_signature" && leg.authorization !== null,
  );
  const hasBridge = awaiting.some((leg) => leg.kind === "bridge");
  if (hasBridge && !options.chains) {
    throw new Error("refusing to sign a bridge withdrawal without a trusted chain registry");
  }
  const keccak = hasBridge ? options.keccak ?? (await loadViemCore()) : null;
  const checked = awaiting.map((leg) => {
    const typed = assertLegAuthorization(withdrawal, leg);
    const source = options.chains?.(Number(leg.source_chain.id));
    if (options.chains && !source) throw new Error(`leg ${leg.id}: source chain is not trusted`);
    const trustedToken = source?.tokens[withdrawal.currency];
    if (source && !trustedToken) {
      throw new Error(`leg ${leg.id}: ${withdrawal.currency} is not trusted on the source chain`);
    }
    if (trustedToken && !sameAddress(typed.domain.verifyingContract, trustedToken)) {
      throw new Error(`leg ${leg.id}: typed data is not under the trusted ${withdrawal.currency} contract`);
    }
    if (!sameAddress(typed.domain.verifyingContract, leg.token.address)) {
      throw new Error(`leg ${leg.id}: typed data is not under the leg's own token`);
    }
    if (leg.kind === "bridge") {
      if (withdrawal.currency !== "USDC") {
        throw new Error(`leg ${leg.id}: only USDC bridges; a ${withdrawal.currency} withdrawal has no bridge leg`);
      }
      if (!source?.cctp) throw new Error(`leg ${leg.id}: source chain has no trusted CCTP configuration`);
      if (!sameAddress(leg.authorization.forwarder ?? "", source.cctp.forwarder)) {
        throw new Error(`leg ${leg.id}: authorization names an untrusted forwarder`);
      }
      const destination = options.chains!(Number(withdrawal.destination.chain.id));
      if (!destination?.cctp) throw new Error(`leg ${leg.id}: destination chain has no trusted CCTP configuration`);
      if (leg.authorization.nonce_preimage?.destination_domain !== destination.cctp.domain) {
        throw new Error(`leg ${leg.id}: nonce preimage names the wrong destination domain`);
      }
      verifyBridgeNonce(keccak!, leg, typed);
    }
    return { leg, typed };
  });
  const authorizations: LegAuthorizationInput[] = [];
  for (const { leg, typed } of checked) {
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
  keccak: WithdrawalKeccak,
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

function verifyBridgeNonce(
  keccak: WithdrawalKeccak,
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
  core: WithdrawalKeccak;
  accounts: {
    privateKeyToAccount: (key: `0x${string}`) => {
      address: `0x${string}`;
      signTypedData: (typedData: SignableTypedData) => Promise<`0x${string}`>;
    };
  };
}

async function loadViemCore(): Promise<WithdrawalKeccak> {
  try {
    return (await import("viem")) as unknown as WithdrawalKeccak;
  } catch (cause) {
    throw new Error("bridge signing needs options.keccak or the optional peer dependency viem: npm install viem", { cause });
  }
}

async function loadViem(): Promise<ViemModules> {
  try {
    const [core, accounts] = await Promise.all([import("viem"), import("viem/accounts")]);
    return { core: core as unknown as ViemModules["core"], accounts: accounts as unknown as ViemModules["accounts"] };
  } catch (cause) {
    throw new Error("privateKeySigner needs the optional peer dependency viem: npm install viem", { cause });
  }
}
