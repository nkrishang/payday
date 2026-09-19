/**
 * Typed evidence model: every observable fact of a benchmark run, the journal
 * events that record them, and the deterministic reducer that turns events
 * into state for the live view and the offline report.
 *
 * Timing discipline (see docs/load-testing-plan.md §3): every event carries a
 * process-local monotonic reading and UTC wall time; chain evidence carries
 * block number/hash/timestamp. Monotonic readings are never compared across
 * process epochs; each event's epoch id makes that checkable.
 */

export type RunPhase = "preflight" | "warmup" | "measure" | "drain" | "complete" | "failed";

export type Experiment = "latency" | "capacity60" | "soak";
export type Profile = "standard" | "pinned" | "attested";
export type DisplayMode = "auto" | "tui" | "plain" | "json";
export type LifecyclePhase =
  | "created" | "binding" | "paying" | "included" | "finalized"
  | "settled" | "verified" | "failed" | "timed_out";

/** The measurement points of docs/load-testing-plan.md §3.2. */
export type TimingPoint =
  | "arrival_scheduled"
  | "create_start"
  | "create_complete"
  | "address_ready"
  | "payment_submit_start"
  | "payment_submit_ack"
  | "payment_inclusion"
  | "payment_first_seen"
  | "payment_finality_seen"
  | "deposited_webhook_received"
  | "credited_api_seen"
  | "payout_inclusion"
  | "payout_finality_seen"
  | "settled_webhook_received"
  | "settled_api_seen"
  | "settled_detail_verified";

export interface Stamp {
  /** Process-local monotonic milliseconds (performance.now-based). */
  mono: number;
  /** UTC wall time, ISO 8601. */
  wall: string;
}

export interface BlockEvidence {
  blockNumber: number;
  blockHash: string;
  /** Chain-reported block time in UTC, ISO 8601. */
  blockTime: string;
  txHash: string;
  logIndex: number;
}

export type LifecycleOutcome = "settled" | "failed" | "timed_out" | "expired" | "returned" | "needs_attention";

export interface OpState {
  op: string;
  index: number;
  warmup: boolean;
  phase: "created" | "binding" | "paying" | "included" | "finalized" | "settled" | "verified" | "failed" | "timed_out";
  depositId?: string;
  chainId?: string;
  currency?: string;
  amountUnits?: string;
  address?: string;
  payout?: string;
  wallet?: string;
  txHash?: string;
  sweepTxHash?: string;
  timings: Partial<Record<TimingPoint, Stamp & { block?: BlockEvidence }>>;
  outcome?: LifecycleOutcome;
  error?: string;
}

export interface ChainRuntime {
  chainId: string;
  name: string;
  currency: string;
  headBlock: number | null;
  headAtMono: number | null;
  finalizedBlock: number | null;
  finalizedAtMono: number | null;
  lastLogScanMono: number | null;
  logBatchesScanned: number;
  rpcCalls: number;
  observerErrors: number;
  lastObserverError?: string;
  /** Completed end-to-end block-clock latencies, seconds, in arrival order. */
  e2e: number[];
  detection: number[];
  sweep: number[];
  recentCompletions: number[];
  watched: number;
}

export interface GuardRuntime {
  reserved: number;
  funded: number;
  outstandingPrincipalUnits: string;
  grossUnits: string;
  paused: boolean;
  stopReason?: string;
}

export interface RunState {
  runId: string;
  mode: "local" | "real";
  target?: string;
  apiOrigin: string;
  experiment: Experiment;
  profile: Profile;
  currency: string;
  amountUnits: string;
  phase: RunPhase;
  startedWall: string;
  epoch: number;
  scheduled: number;
  admitted: number;
  ops: Map<string, OpState>;
  byDeposit: Map<string, string>;
  chains: Map<string, ChainRuntime>;
  guard: GuardRuntime;
  apiCalls: number;
  api429s: number;
  apiLastError?: string;
  webhooksReceived: number;
  webhooksInvalid: number;
  webhookUrl?: string;
  webhookDeliveryOk: boolean;
  statusSample?: Record<string, unknown>;
  warnings: string[];
  capacity?: {
    t0Mono: number;
    deadlineMono: number;
    frozen: boolean;
    cohort?: number;
    pass?: boolean;
    settledBy60: { api: number; webhook: number; chain: number };
    previousTrials: Array<{ cohort: number; settledBy60: number; pass: boolean }>;
  };
  drainStartedMono?: number;
}

export type JournalEvent =
  | { type: "run.started"; payload: Record<string, unknown> }
  | { type: "phase.changed"; payload: { phase: RunPhase } }
  | { type: "op.intended"; payload: { op: string; index: number; warmup: boolean; body: unknown; idempotencyKey: string; scheduledMono?: number } }
  | { type: "op.created"; payload: { op: string; depositId: string; currency: string; amount: string; networks: unknown } }
  | { type: "op.bound"; payload: { op: string; chainId: string; address: string; payout: string; wallet: string } }
  | { type: "op.payment.intent"; payload: { op: string; wallet: string; chainId: string; to: string; valueUnits: string; nonce: number } }
  | { type: "op.payment.signed"; payload: { op: string; txHash: string; blob: string } }
  | { type: "op.payment.broadcast"; payload: { op: string; txHash: string } }
  | { type: "op.payment.included"; payload: { op: string; evidence: BlockEvidence; observedMono: number } }
  | { type: "op.payment.finalized"; payload: { op: string; boundaryBlock: number } }
  | { type: "op.payout.included"; payload: { op: string; sweepTxHash: string; evidence: BlockEvidence; payoutAddress: string; valueUnits: string } }
  | { type: "op.payout.finalized"; payload: { op: string; boundaryBlock: number } }
  | { type: "op.credited"; payload: { op: string; source: "api" | "webhook" } }
  | { type: "op.settled.api"; payload: { op: string } }
  | { type: "op.settled.webhook"; payload: { op: string; eventId: string } }
  | { type: "op.verified"; payload: { op: string; checks: string[] } }
  | { type: "op.failed"; payload: { op: string; phase: string; error: string } }
  | { type: "op.timed_out"; payload: { op: string } }
  | { type: "op.outcome"; payload: { op: string; outcome: LifecycleOutcome } }
  | { type: "api.call"; payload: { op?: string; route: string; status: number; ms: number; attempt: number } }
  | { type: "api.rate_limited"; payload: { route: string; retryAfter?: number } }
  | { type: "webhook.received"; payload: { eventId: string; type: string; depositId?: string; op?: string; blob: string } }
  | { type: "webhook.invalid"; payload: { reason: string } }
  | { type: "webhook.verified_endpoint"; payload: Record<string, never> }
  | { type: "guard.event"; payload: { kind: string; detail?: string } }
  | { type: "funding.record"; payload: { chainId: string; wallet: string; nativeWei?: string; tokenUnits?: string; txHashes: string[] } }
  | { type: "status.sample"; payload: { sample: Record<string, unknown> } }
  | { type: "warning"; payload: { text: string } }
  | { type: "run.stopped"; payload: { reason: string } }
  | { type: "capacity.frozen"; payload: { t0Mono: number; deadlineMono: number; cohort: number; pass: boolean; settledBy60: { api: number; webhook: number; chain: number } } }
  | { type: "note"; payload: { text: string } };

/** The reducer applies journal events to state; also the live engine's sink. */
export type EventSink = (event: JournalEvent) => void;

export const ALL_POINTS: TimingPoint[] = [
  "arrival_scheduled", "create_start", "create_complete", "address_ready",
  "payment_submit_start", "payment_submit_ack", "payment_inclusion", "payment_first_seen",
  "payment_finality_seen", "deposited_webhook_received", "credited_api_seen",
  "payout_inclusion", "payout_finality_seen", "settled_webhook_received",
  "settled_api_seen", "settled_detail_verified",
];

export function emptyState(init: Pick<RunState, "runId" | "mode" | "apiOrigin" | "experiment" | "profile" | "currency" | "amountUnits" | "epoch"> & { target?: string }): RunState {
  return {
    ...init,
    phase: "preflight",
    startedWall: new Date().toISOString(),
    scheduled: 0,
    admitted: 0,
    ops: new Map(),
    byDeposit: new Map(),
    chains: new Map(),
    guard: { reserved: 0, funded: 0, outstandingPrincipalUnits: "0", grossUnits: "0", paused: false },
    apiCalls: 0,
    api429s: 0,
    webhooksReceived: 0,
    webhooksInvalid: 0,
    webhookDeliveryOk: false,
    warnings: [],
  };
}

export function chainRuntime(state: RunState, chainId: string, name: string, currency: string): ChainRuntime {
  let runtime = state.chains.get(chainId);
  if (!runtime) {
    runtime = {
      chainId, name, currency, headBlock: null, headAtMono: null, finalizedBlock: null, finalizedAtMono: null,
      lastLogScanMono: null, logBatchesScanned: 0, rpcCalls: 0, observerErrors: 0,
      e2e: [], detection: [], sweep: [], recentCompletions: [], watched: 0,
    };
    state.chains.set(chainId, runtime);
  }
  return runtime;
}

/** Apply one event to state. Pure except for the maps it mutates. */
export function reduce(state: RunState, event: JournalEvent, mono: number, wall?: string): void {
  switch (event.type) {
    case "phase.changed":
      state.phase = event.payload.phase;
      return;
    case "op.intended": {
      const { op, index, warmup, scheduledMono } = event.payload;
      state.ops.set(op, {
        op, index, warmup, phase: "created",
        timings: {
          arrival_scheduled: { mono: scheduledMono ?? mono, wall: new Date(Date.now()).toISOString() },
          // The op's lifecycle clock starts here; per-op timeouts and the UI
          // elapsed clock both read it.
          create_start: { mono, wall: new Date(Date.now()).toISOString() },
        },
      });
      state.scheduled += 1;
      // Admission is an event, not a side mutation, so journal replay
      // reproduces the admitted count exactly.
      state.admitted += 1;
      return;
    }
    case "op.created": {
      const op = state.ops.get(event.payload.op);
      if (!op) return;
      op.depositId = event.payload.depositId;
      op.currency = event.payload.currency;
      op.amountUnits = decimalToUnits(event.payload.amount);
      state.byDeposit.set(event.payload.depositId, op.op);
      return;
    }
    case "op.bound": {
      const op = state.ops.get(event.payload.op);
      if (!op) return;
      op.chainId = event.payload.chainId;
      op.address = event.payload.address;
      op.payout = event.payload.payout;
      op.wallet = event.payload.wallet;
      chainRuntime(state, event.payload.chainId, event.payload.chainId, op.currency ?? "").watched += 1;
      return;
    }
    case "op.payment.signed": {
      // Identity is retained at signing: if the broadcast ack is lost, the
      // transaction still exists on-chain and must stay in liability reports.
      const op = state.ops.get(event.payload.op);
      if (!op) return;
      op.txHash = event.payload.txHash;
      return;
    }
    case "op.payment.broadcast": {
      const op = state.ops.get(event.payload.op);
      if (!op) return;
      op.phase = "paying";
      op.txHash = event.payload.txHash;
      stamp(op, "payment_submit_ack", mono, undefined, wall);
      state.guard.funded += 1;
      if (op.amountUnits) {
        state.guard.outstandingPrincipalUnits = (BigInt(state.guard.outstandingPrincipalUnits || "0") + BigInt(op.amountUnits)).toString();
      }
      return;
    }
    case "op.payment.included": {
      const op = state.ops.get(event.payload.op);
      if (!op) return;
      op.phase = "included";
      stamp(op, "payment_inclusion", mono, event.payload.evidence, wall);
      stamp(op, "payment_first_seen", mono, undefined, wall);
      return;
    }
    case "op.payment.finalized": {
      const op = state.ops.get(event.payload.op);
      if (!op) return;
      op.phase = "finalized";
      stamp(op, "payment_finality_seen", mono, undefined, wall);
      return;
    }
    case "op.credited": {
      const op = state.ops.get(event.payload.op);
      if (!op) return;
      stamp(op, event.payload.source === "webhook" ? "deposited_webhook_received" : "credited_api_seen", mono, undefined, wall);
      const other: TimingPoint = event.payload.source === "webhook" ? "credited_api_seen" : "deposited_webhook_received";
      if (!op.timings[other]) stamp(op, other, mono, undefined, wall);
      return;
    }
    case "op.payout.included": {
      const op = state.ops.get(event.payload.op);
      if (!op) return;
      op.sweepTxHash = event.payload.sweepTxHash;
      op.phase = "settled";
      stamp(op, "payout_inclusion", mono, event.payload.evidence, wall);
      return;
    }
    case "op.payout.finalized": {
      const op = state.ops.get(event.payload.op);
      if (!op) return;
      stamp(op, "payout_finality_seen", mono, undefined, wall);
      return;
    }
    case "op.settled.api": {
      const op = state.ops.get(event.payload.op);
      if (!op) return;
      stamp(op, "settled_api_seen", mono, undefined, wall);
      if (op.phase !== "verified") op.phase = "settled";
      return;
    }
    case "op.settled.webhook": {
      const op = state.ops.get(event.payload.op);
      if (!op) return;
      stamp(op, "settled_webhook_received", mono, undefined, wall);
      return;
    }
    case "op.verified": {
      const op = state.ops.get(event.payload.op);
      if (!op) return;
      stamp(op, "settled_detail_verified", mono, undefined, wall);
      op.phase = "verified";
      if (op.amountUnits) {
        state.guard.outstandingPrincipalUnits = (BigInt(state.guard.outstandingPrincipalUnits || "0") - BigInt(op.amountUnits)).toString();
        if (BigInt(state.guard.outstandingPrincipalUnits) < 0n) state.guard.outstandingPrincipalUnits = "0";
      }
      recordCompletion(state, op, mono);
      return;
    }
    case "op.failed": {
      const op = state.ops.get(event.payload.op);
      if (!op) return;
      op.phase = "failed";
      op.error = event.payload.error;
      return;
    }
    case "op.timed_out": {
      const op = state.ops.get(event.payload.op);
      if (!op) return;
      op.phase = "timed_out";
      op.outcome = "timed_out";
      return;
    }
    case "op.outcome": {
      const op = state.ops.get(event.payload.op);
      if (!op) return;
      op.outcome = event.payload.outcome;
      return;
    }
    case "api.call":
      state.apiCalls += 1;
      return;
    case "api.rate_limited":
      state.api429s += 1;
      return;
    case "webhook.received":
      state.webhooksReceived += 1;
      return;
    case "webhook.invalid":
      state.webhooksInvalid += 1;
      return;
    case "warning":
      if (!state.warnings.includes(event.payload.text)) {
        state.warnings.push(event.payload.text);
        if (state.warnings.length > 50) state.warnings.shift();
      }
      return;
    case "run.stopped":
      if (!state.guard.stopReason) state.guard.stopReason = event.payload.reason;
      return;
    case "capacity.frozen": {
      // Capacity results are events, so a replayed journal reproduces them.
      state.capacity = {
        t0Mono: event.payload.t0Mono,
        deadlineMono: event.payload.deadlineMono,
        frozen: true,
        cohort: event.payload.cohort,
        pass: event.payload.pass,
        settledBy60: event.payload.settledBy60,
        previousTrials: state.capacity?.previousTrials ?? [],
      };
      return;
    }
    default:
      return;
  }
}

function recordCompletion(state: RunState, op: OpState, mono: number): void {
  if (!op.chainId) return;
  const runtime = state.chains.get(op.chainId);
  if (!runtime) return;
  const payment = op.timings.payment_inclusion;
  const payout = op.timings.payout_inclusion;
  if (payment?.block && payout?.block) {
    const seconds = (Date.parse(payout.block.blockTime) - Date.parse(payment.block.blockTime)) / 1000;
    runtime.e2e.push(seconds);
    runtime.recentCompletions.push(seconds);
    if (runtime.recentCompletions.length > 60) runtime.recentCompletions.shift();
  }
  const first = op.timings.payment_first_seen;
  if (first && payout) runtime.detection.push((payout.mono - first.mono) / 1000);
  const finality = op.timings.payout_finality_seen;
  if (first && finality) runtime.sweep.push((finality.mono - first.mono) / 1000);
}

export function stamp(op: OpState, point: TimingPoint, mono: number, block?: BlockEvidence, wall?: string): void {
  if (op.timings[point]) return;
  // The wall time rides with the event when replaying a journal; only a live
  // reduction falls back to the current clock, so replayed reports stay
  // byte-identical to the originals.
  op.timings[point] = { mono, wall: wall ?? new Date().toISOString(), ...(block ? { block } : {}) };
}

/** Decimal-string units conversion: all six-decimal stablecoins. */
export function decimalToUnits(decimal: string): string {
  const trimmed = decimal.trim();
  if (!/^\d+(\.\d+)?$/.test(trimmed)) throw new Error(`amount must be a plain decimal, got ${decimal}`);
  const [whole, frac = ""] = trimmed.split(".");
  return BigInt(`${whole}${frac.padEnd(6, "0").slice(0, 6)}`).toString();
}

/** Seconds between two timing points using chain block clocks when available. */
export function secondsBetween(op: OpState, from: TimingPoint, to: TimingPoint): number | undefined {
  const a = op.timings[from];
  const b = op.timings[to];
  if (!a || !b) return undefined;
  if (a.block && b.block) return (Date.parse(b.block.blockTime) - Date.parse(a.block.blockTime)) / 1000;
  return (b.mono - a.mono) / 1000;
}
