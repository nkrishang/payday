/**
 * Chain observation, shared per chain: a filtered log scanner (payments into
 * tracked deposit addresses, settlements out of them), a finality cursor
 * implementing Gum's exact boundary, and deduplicated receipt reads.
 *
 * The scanner is the accounting source; its cadence bounds
 * `payment_first_seen` resolution. Finality matches Gum's ledger rule
 * directly: `receiptBlock ≤ head − margin` for confirmations chains, the
 * `finalized` tag for finalized chains — never viem's `confirmations: N`
 * receipt wait, which counts the inclusion block itself.
 */

import { createPublicClient, http, type Log } from "viem";
import { ERC20_ABI, type ChainConfig } from "./chains.js";
import type { BlockEvidence, EventSink } from "./model.js";

interface TransferLog extends Log {
  args: { from?: string; to?: string; value?: bigint };
  blockNumber: bigint;
  blockHash: `0x${string}`;
  transactionHash: `0x${string}`;
  logIndex: number;
}

export interface ChainObservation {
  kind: "payment_included" | "payout_included";
  address: string;
  token: string;
  valueUnits: string;
  evidence: BlockEvidence;
}

export class ChainObserver {
  private readonly client;
  private readonly chainId: string;
  private readonly token: string;
  private paymentAddresses = new Set<string>();
  private payoutAddresses = new Set<string>();
  private seenLogs = new Set<string>();
  private scannedTo = 0n;
  private finalizedTo = 0n;
  private timer?: NodeJS.Timeout;
  private running = false;
  private readonly callbacks = new Set<(observation: ChainObservation) => void>();
  private knownBlocks = new Map<number, { hash: string; time: string }>();
  /** Squared away finality checks awaiting their boundary. */
  private pendingFinality = new Map<string, { block: number; included: boolean; kind: "payment" | "payout" }>();
  rpcCalls = 0;
  observerErrors = 0;
  lastError?: string;

  constructor(
    readonly config: ChainConfig,
    private readonly currency: string,
    private readonly pollMs: number,
    private readonly rangeBlocks: number,
    private readonly sink: (observation: ChainObservation) => void,
    readonly onError: (error: string) => void,
  ) {
    this.chainId = config.id;
    const token = config.tokens[currency] ?? Object.values(config.tokens)[0]!;
    this.token = token;
    this.client = createPublicClient({ transport: http(config.rpcUrl, { batch: { wait: 16 } }) });
  }

  get observedToken(): string {
    return this.token;
  }

  watchPayment(address: string): void {
    this.paymentAddresses.add(address.toLowerCase());
  }

  watchPayout(address: string): void {
    this.payoutAddresses.add(address.toLowerCase());
  }

  on(callback: (observation: ChainObservation) => void): void {
    this.callbacks.add(callback);
  }

  async start(): Promise<void> {
    this.running = true;
    this.scannedTo = await this.head() - 1n;
    this.finalizedTo = await this.finalityBoundary();
    void this.loop();
  }

  stop(): void {
    this.running = false;
    if (this.timer) clearTimeout(this.timer);
  }

  async head(): Promise<bigint> {
    this.rpcCalls += 1;
    const head = await this.client.getBlockNumber().catch((error: unknown) => {
      this.fail(error);
      return 0n;
    });
    return head;
  }

  async finalityBoundary(): Promise<bigint> {
    this.rpcCalls += 1;
    if (this.config.finality.kind === "finalized") {
      const block = await this.client.getBlock({ blockTag: "finalized" }).catch((error: unknown) => {
        this.fail(error);
        return null;
      });
      if (!block) return this.finalizedTo;
      this.cache(block.number, block.hash, block.timestamp);
      return block.number;
    }
    const head = await this.head();
    return head > BigInt(this.config.finality.margin) ? head - BigInt(this.config.finality.margin) : 0n;
  }

  private fail(error: unknown): void {
    this.observerErrors += 1;
    this.lastError = error instanceof Error ? error.message : String(error);
    this.onError(`chain ${this.chainId} observer: ${this.lastError}`);
  }

  private cache(number: bigint, hash: `0x${string}`, seconds: bigint | number): void {
    this.knownBlocks.set(Number(number), {
      hash,
      time: new Date(Number(seconds) * 1000).toISOString(),
    });
    if (this.knownBlocks.size > 4096) {
      const oldest = Math.min(...this.knownBlocks.keys());
      this.knownBlocks.delete(oldest);
    }
  }

  private async loop(): Promise<void> {
    while (this.running) {
      try {
        await this.scan();
      } catch (error) {
        this.fail(error);
      }
      await new Promise((resolve) => (this.timer = setTimeout(resolve, this.pollMs)));
    }
  }

  /** One pass: advance the log scan, then the finality cursor. */
  async scan(): Promise<void> {
    const head = await this.head();
    if (head === 0n) return;
    if (head > this.scannedTo) {
      const from = this.scannedTo + 1n;
      const to = head;
      await this.scanRange(from, to);
      this.scannedTo = to;
    }
    const boundary = await this.finalityBoundary();
    if (boundary > this.finalizedTo) {
      this.finalizedTo = boundary;
      this.advanceFinality();
    }
  }

  private async scanRange(from: bigint, to: bigint): Promise<void> {
    // Chunked by the configured range ceiling so a provider's cap only costs
    // extra calls, never correctness. A sweep is a Transfer *from* a payment
    // address to its payout, so outgoing evidence filters on the same
    // payment-address set.
    for (let start = from; start <= to; start += BigInt(this.rangeBlocks)) {
      const end = start + BigInt(this.rangeBlocks) - 1n > to ? to : start + BigInt(this.rangeBlocks) - 1n;
      const logs = await this.fetchLogs(start, end, [...this.paymentAddresses], false);
      for (const log of logs) this.emit("payment_included", log);
      const outgoing = await this.fetchLogs(start, end, [...this.paymentAddresses], true);
      for (const log of outgoing) this.emit("payout_included", log);
    }
  }

  private async fetchLogs(from: bigint, to: bigint, addresses: string[], outgoing = false): Promise<TransferLog[]> {
    if (addresses.length === 0) return [];
    this.rpcCalls += 1;
    try {
      const logs = await this.client.getLogs({
        address: this.token as `0x${string}`,
        event: ERC20_ABI[0],
        args: outgoing ? { from: addresses as `0x${string}`[] } : { to: addresses as `0x${string}`[] },
        fromBlock: from,
        toBlock: to,
      });
      return logs as unknown as TransferLog[];
    } catch (error) {
      // Rethrow after accounting: the caller must not advance the scan cursor
      // past a range that failed to read — that would drop evidence forever.
      this.fail(error);
      throw error;
    }
  }

  private emit(kind: ChainObservation["kind"], log: TransferLog): void {
    const args = log.args;
    // Both directions key on the payment address: a payment lands `to` it, a
    // sweep leaves `from` it (toward the payout, which is evidence, not the
    // filter key — the log query already constrained `from` to this set).
    const address = kind === "payment_included" ? args.to : args.from;
    if (!address || args.value === undefined) return;
    const key = address.toLowerCase();
    if (!this.paymentAddresses.has(key)) return;
    if (kind === "payout_included" && this.payoutAddresses.size > 0 && args.to && !this.payoutAddresses.has(args.to.toLowerCase())) return;
    // Ranges can be rescanned after a partial failure; each on-chain event is
    // delivered once.
    const dedup = `${kind}:${log.transactionHash}:${log.logIndex}`;
    if (this.seenLogs.has(dedup)) return;
    this.seenLogs.add(dedup);
    if (this.seenLogs.size > 65_536) {
      // Bound memory on long soaks; old observations have long been consumed.
      this.seenLogs.clear();
    }
    const observation: ChainObservation = {
      kind,
      address: key,
      token: this.token.toLowerCase(),
      valueUnits: args.value.toString(),
      evidence: {
        blockNumber: Number(log.blockNumber),
        blockHash: log.blockHash,
        blockTime: this.knownBlocks.get(Number(log.blockNumber))?.time ?? "",
        txHash: log.transactionHash,
        logIndex: Number(log.logIndex),
      },
    };
    for (const callback of this.callbacks) callback(observation);
    this.sink(observation);
  }

  private blockTime(blockNumber: number): string {
    return this.knownBlocks.get(blockNumber)?.time ?? new Date(0).toISOString();
  }

  /** Fetches a block's real timestamp when first needed for evidence. */
  async ensureBlockTime(blockNumber: number): Promise<string | undefined> {
    const known = this.knownBlocks.get(blockNumber);
    if (known?.time) return known.time;
    this.rpcCalls += 1;
    const block = await this.client.getBlock({ blockNumber: BigInt(blockNumber) }).catch(() => null);
    if (!block) return undefined;
    this.cache(block.number, block.hash, block.timestamp);
    return this.knownBlocks.get(blockNumber)?.time;
  }

  private advanceFinality(): void {
    void this.chainId;
    // Tracked by the engine: finality is a per-address cursor question, so
    // the observer exposes the boundary and the engine marks its evidence.
  }

  /** Current Gum-accepted finality boundary for this chain. */
  get boundary(): number {
    return Number(this.finalizedTo);
  }

  /** Whether the payment/payout evidence at `block` has crossed the boundary. */
  isFinalized(blockNumber: number): boolean {
    return this.finalizedTo > 0n && BigInt(blockNumber) <= this.finalizedTo;
  }

  get watchedPaymentCount(): number {
    return this.paymentAddresses.size;
  }

  async receipt(txHash: string): Promise<TransferLog[] | null> {
    this.rpcCalls += 1;
    const receipt = await this.client.getTransactionReceipt({ hash: txHash as `0x${string}` }).catch(() => null);
    if (!receipt) return null;
    const logs = (receipt.logs as unknown as TransferLog[]).filter((log) => log.address.toLowerCase() === this.token.toLowerCase());
    for (const log of logs) {
      this.cache(log.blockNumber, log.blockHash, 0);
    }
    return logs;
  }
}

export function hexToUnits(hexOrBigint: bigint | string): string {
  return BigInt(hexOrBigint).toString();
}
