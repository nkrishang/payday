/**
 * The lifecycle driver: one deposit request from create intent through
 * binding, payment broadcast, and (asynchronously) observation to verified
 * settlement. Talks to Gum only through the public SDK clients and to the
 * chain only as a payer wallet would.
 */

import type { Clients } from "./sdk.js";
import type { ChainConfig } from "./chains.js";
import { ERC20_ABI } from "./chains.js";
import type { ChainObserver, ChainObservation } from "./chain-observer.js";
import { createPublicClient, encodeFunctionData, http, verifyTypedData, type Address } from "viem";
import { viemChain, type PayerWallet, type WalletPool } from "./wallets.js";
import type { BlockEvidence, EventSink, JournalEvent, Profile, RunState } from "./model.js";
import { decimalToUnits, reduce } from "./model.js";

export interface LifecyclePlan {
  op: string;
  index: number;
  warmup: boolean;
  currency: string;
  amount: string;
  chainId: string;
  profile: Profile;
  issuerId: string;
  reference: string;
  payout: string;
  /** The scheduler's intended arrival; dispatch time is recorded separately. */
  scheduledMono?: number;
}

export class LifecycleError extends Error {}

export class Lifecycle {
  private readonly amountUnits: string;

  constructor(
    private readonly plan: LifecyclePlan,
    private readonly clients: Clients,
    private readonly pool: WalletPool,
    private readonly chain: ChainConfig,
    private readonly observers: Map<string, ChainObserver>,
    private readonly sink: EventSink,
    private readonly state: RunState,
    private readonly nowMono: () => number,
    private readonly blobWriter: (name: string, contents: string) => string,
  ) {
    this.amountUnits = decimalToUnits(plan.amount);
  }

  private emit(event: JournalEvent, options: { durable?: boolean } = {}): void {
    this.sink(event);
    void options;
  }

  get depositId(): string | undefined {
    return this.state.ops.get(this.plan.op)?.depositId;
  }

  /** The active half: create → bind → pay. Observation continues elsewhere. */
  async run(): Promise<void> {
    const { plan } = this;
    const chain = this.chain;
    const mono = this.nowMono();
    // arrival_scheduled carries the scheduler's intended time so the gap to
    // actual dispatch (generator delay) stays measurable.
    this.emit({ type: "op.intended", payload: { op: plan.op, index: plan.index, warmup: plan.warmup, body: { currency: plan.currency, amount: plan.amount, chainId: plan.chainId, profile: plan.profile }, idempotencyKey: `bench:${plan.issuerId}:${plan.op}`, scheduledMono: plan.scheduledMono ?? mono } });

    const body = {
      amount: plan.amount,
      currency: plan.currency,
      payout_address: plan.payout,
      issuer: { name: "Gum Load Benchmark" },
      payer: { name: `Gum Benchmark Payer ${plan.index}` },
      issuer_id: plan.issuerId,
      reference: plan.reference,
      metadata: { benchmark: true, run: plan.issuerId, op: plan.op, index: plan.index },
      ...(plan.profile === "pinned" || plan.profile === "attested" ? { chain_id: plan.chainId } : {}),
      ...(plan.profile === "attested" ? { verification: { wallet_attestation: true } } : {}),
      expires_in: 3600,
    } as const;

    const wallet = this.pool.all()[plan.index % this.pool.all().length]!;

    let created;
    try {
      created = await this.clients.merchant.depositRequests.create({ ...body, metadata: body.metadata }, `bench:${plan.issuerId}:${plan.op}`);
    } catch (error) {
      this.fail(error);
      return;
    }
    this.emit({ type: "op.created", payload: { op: plan.op, depositId: created.id, currency: plan.currency, amount: plan.amount, networks: created.networks } });

    // Token-contract allowlist check: the API's snapshot must name the exact
    // contract the operator approved.
    const network = created.networks?.find((entry) => entry.chain.id === plan.chainId);
    if (network) {
      const allowed = chain.tokens[plan.currency]?.toLowerCase();
      if (allowed && network.token.address.toLowerCase() !== allowed) {
        this.fail(new LifecycleError(`deposit request names token ${network.token.address} but the allowlist has ${allowed}`));
        return;
      }
    }

    let address: string | undefined;
    if (plan.profile === "attested") {
      try {
        address = await this.attestAndBind(created.id, wallet, created);
      } catch (error) {
        this.fail(error);
        return;
      }
    } else if (plan.profile === "standard") {
      try {
        const bound = await this.clients.payer.depositRequests.selectNetwork(created.id, plan.chainId);
        address = bound.address ?? undefined;
      } catch (error) {
        this.fail(error);
        return;
      }
    } else {
      // Pinned requests answer the address in the create response once
      // derived; otherwise read it back once.
      address = created.address ?? undefined;
      if (!address) {
        const fetched = await this.clients.merchant.depositRequests.get(created.id).catch(() => null);
        address = fetched?.address ?? undefined;
      }
    }
    if (!address) {
      this.fail(new LifecycleError("no payment address after binding"));
      return;
    }
    this.emit({ type: "op.bound", payload: { op: plan.op, chainId: plan.chainId, address, payout: plan.payout, wallet: wallet.address } });

    await this.pay(address, wallet, chain, plan);
  }

  /**
   * Wallet-attested binding: mint the challenge, validate every commitment
   * against the allowlists, sign with the payer key, attest. Never calls
   * `selectNetwork` — wallet-attested requests choose the chain here.
   */
  private async attestAndBind(depositId: string, wallet: PayerWallet, created: Awaited<ReturnType<Clients["merchant"]["depositRequests"]["create"]>>): Promise<string> {
    const challenge = await this.clients.payer.wallet.challenge(depositId, wallet.address, this.plan.chainId);
    const typed = challenge.typed_data;
    const expectedFactory = this.chain.factories ?? [];
    const problems: string[] = [];
    if (typed.domain.name !== "Gum") problems.push(`domain.name ${typed.domain.name}`);
    if (typed.domain.version !== "2") problems.push(`domain.version ${typed.domain.version}`);
    if (typed.primaryType !== "PayerAttestation") problems.push(`primaryType ${typed.primaryType}`);
    if (String(typed.domain.chainId) !== this.plan.chainId) problems.push(`domain.chainId ${typed.domain.chainId}`);
    if (typed.message.wallet.toLowerCase() !== wallet.address.toLowerCase()) problems.push("message.wallet is not the leased payer");
    const attributionHash = (created as { attribution?: { hash?: string } }).attribution?.hash;
    if (attributionHash && typed.message.attributionHash.toLowerCase() !== attributionHash.toLowerCase()) problems.push("attributionHash mismatch");
    if (expectedFactory.length > 0 && !expectedFactory.includes(typed.domain.verifyingContract.toLowerCase())) {
      problems.push(`verifyingContract ${typed.domain.verifyingContract} is not allowlisted`);
    }
    // The message schema is fixed by the server (gum-core payer_attestation);
    // the harness signs exactly these fields and nothing else.
    const expectedSchema: Array<[string, string]> = [
      ["statement", "string"], ["attributionHash", "bytes32"], ["wallet", "address"],
      ["nonce", "bytes32"], ["expiresAt", "uint256"],
    ];
    const schema = typed.types["PayerAttestation"] ?? [];
    for (const [name, type] of expectedSchema) {
      const field = schema.find((entry) => entry.name === name);
      if (!field || field.type !== type) problems.push(`schema field ${name} is missing or mis-typed`);
    }
    if (problems.length > 0) throw new LifecycleError(`challenge rejected: ${problems.join("; ")}`);

    const domain = {
      name: typed.domain.name,
      version: typed.domain.version,
      chainId: typed.domain.chainId,
      verifyingContract: typed.domain.verifyingContract as `0x${string}`,
    };
    const message = {
      statement: typed.message.statement,
      attributionHash: typed.message.attributionHash as `0x${string}`,
      wallet: typed.message.wallet as `0x${string}`,
      nonce: typed.message.nonce as `0x${string}`,
      expiresAt: BigInt(Math.round(Number(typed.message.expiresAt))),
    };
    const types = {
      PayerAttestation: [
        { name: "statement", type: "string" },
        { name: "attributionHash", type: "bytes32" },
        { name: "wallet", type: "address" },
        { name: "nonce", type: "bytes32" },
        { name: "expiresAt", type: "uint256" },
      ],
    } as const;

    const signature = await wallet.account.signTypedData({ domain, types, primaryType: "PayerAttestation" as const, message });
    // Independent verification that the signature recovers to the wallet.
    await verifyTypedData({ address: wallet.address, domain, types, primaryType: "PayerAttestation" as const, message, signature })
      .catch((error: unknown) => { throw new LifecycleError(`self-verification failed: ${String(error)}`); });

    const bound = await this.clients.payer.wallet.attest(depositId, wallet.address, signature, challenge.payer_session);
    return bound.address ?? "";
  }

  /** Reserve the nonce, sign, journal the signed bytes, then broadcast. */
  private async pay(address: string, wallet: PayerWallet, chain: ChainConfig, plan: LifecyclePlan): Promise<void> {
    const observer = this.observers.get(chain.id);
    if (!observer) throw new LifecycleError(`no observer for chain ${chain.id}`);
    const token = chain.tokens[plan.currency] ?? Object.values(chain.tokens)[0]!;
    observer.watchPayment(address);
    observer.watchPayout(plan.payout);

    const data = encodeFunctionData({
      abi: ERC20_ABI,
      functionName: "transfer",
      args: [address as Address, BigInt(this.amountUnits)],
    });

    await this.pool.withSubmissionSlot(chain, wallet, async (lease) => {
      const client = this.pool.walletClient(chain, wallet);
      const publicClient = createPublicClient({ chain: viemChain(chain), transport: http(chain.rpcUrl) });
      const [fee, gas] = await Promise.all([
        publicClient.estimateFeesPerGas().catch(() => undefined),
        publicClient.estimateGas({ account: wallet.address, to: token as Address, data }).catch(() => 80_000n),
      ]);
      const nonce = lease.nonce;
      this.emit({ type: "op.payment.intent", payload: { op: plan.op, wallet: wallet.address, chainId: chain.id, to: address, valueUnits: this.amountUnits, nonce } });

      const signed = await client.signTransaction({
        account: wallet.account,
        chain: viemChain(chain),
        to: token as Address,
        data,
        nonce,
        gas,
        ...(fee?.maxFeePerGas ? { maxFeePerGas: fee.maxFeePerGas } : {}),
        ...(fee?.maxPriorityFeePerGas ? { maxPriorityFeePerGas: fee.maxPriorityFeePerGas } : {}),
        ...(fee?.gasPrice ? { gasPrice: fee.gasPrice } : {}),
      });
      // viem signs without computing the hash for us; the keccak of the signed
      // bytes is the tx hash.
      const { keccak256 } = await import("viem");
      const txHash = keccak256(signed);
      this.emit({ type: "op.payment.signed", payload: { op: plan.op, txHash, blob: this.blobWriter(`signed-${plan.op}`, signed) } });

      // Broadcast only after the signed bytes are durable.
      await client.sendRawTransaction({ serializedTransaction: signed });
      this.emit({ type: "op.payment.broadcast", payload: { op: plan.op, txHash } });
      return txHash;
    }).catch((error: unknown) => {
      this.fail(error);
      return null;
    });
  }

  private fail(error: unknown): void {
    const message = error instanceof Error ? error.message : String(error);
    this.emit({ type: "op.failed", payload: { op: this.plan.op, phase: this.state.ops.get(this.plan.op)?.phase ?? "created", error: message } });
  }

  /** Chain-observation entry point; called by the engine for each lifecycle. */
  onObservation(observation: ChainObservation): void {
    const op = this.state.byDeposit.get(this.plan.op);
    void op;
    const state = this.state.ops.get(this.plan.op);
    if (!state) return;
    const mono = this.nowMono();
    if (observation.kind === "payment_included" && observation.address.toLowerCase() === state.address?.toLowerCase()) {
      this.emit({ type: "op.payment.included", payload: { op: this.plan.op, evidence: observation.evidence, observedMono: mono } });
    } else if (observation.kind === "payout_included" && state.address && observation.address.toLowerCase() === state.address.toLowerCase()) {
      const expected = BigInt(this.amountUnits);
      if (BigInt(observation.valueUnits) >= expected) {
        this.emit({ type: "op.payout.included", payload: { op: this.plan.op, sweepTxHash: observation.evidence.txHash, evidence: observation.evidence, payoutAddress: observation.address, valueUnits: observation.valueUnits } });
      }
    }
  }
}

/** Convenience: convert decimal amount to units for planning math. */
export function planUnits(plan: LifecyclePlan): bigint {
  return BigInt(decimalToUnits(plan.amount));
}
