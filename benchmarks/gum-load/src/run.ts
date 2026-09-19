/**
 * Run composition and experiment orchestration: preflight, warmup, measure,
 * drain — one process owns the run directory and the payer pool. Everything
 * irreversible is journaled before it happens; everything observable flows
 * through the single event pipeline into state, the live view, and the
 * offline report.
 */

import { mkdirSync } from "node:fs";
import { join } from "node:path";
import type { HarnessConfig } from "./config.js";
import { requireApiKey } from "./config.js";
import { Journal, newRunId, replay } from "./journal.js";
import { emptyState, reduce, chainRuntime, stamp, decimalToUnits, type DisplayMode, type EventSink, type JournalEvent, type RunState, type RunPhase } from "./model.js";
import { createClients, type Clients } from "./sdk.js";
import { WalletPool, resolveSeed } from "./wallets.js";
import { ChainObserver, type ChainObservation } from "./chain-observer.js";
import { WebhookReceiver } from "./webhooks.js";
import { ApiObserver } from "./api-observer.js";
import { Guards, type Reservation } from "./guards.js";
import { Lifecycle } from "./lifecycle.js";
import { cohort, openLoop, sleep } from "./scheduler.js";
import { defaultFundingPlan, fundLocalPayers, assertFundingIsLocal } from "./funding.js";
import { ERC20_ABI } from "./chains.js";
import { createPublicClient, http, parseUnits, type Address } from "viem";
import { renderLoop, type Display } from "./ui/ansi.js";
import { buildView } from "./ui/view.js";
import { writeReport } from "./report.js";

export interface RunOptions {
  config: HarnessConfig;
  chainId: string;
  experiment: Parameters<typeof emptyState>[0]["experiment"];
  profile: "standard" | "pinned" | "attested";
  count: number;
  rate?: number;
  durationSeconds?: number;
  warmup: number;
  repeat: number;
  display: DisplayMode;
  fundLocal: boolean;
  runId?: string;
  jitter: boolean;
}

export interface ResolvedRun {
  runId: string;
  dir: string;
  state: RunState;
}

export async function run(options: RunOptions): Promise<ResolvedRun> {
  const { config } = options;
  const runId = options.runId ?? newRunId();
  const dir = join(process.cwd(), "runs", runId);
  mkdirSync(dir, { recursive: true });
  const journal = new Journal(dir);

  const chain = config.chains[options.chainId];
  if (!chain) throw new Error(`chain ${options.chainId} not configured`);
  if (options.profile === "pinned" || options.profile === "attested") {
    // both profiles pin the chain; standard selects through the payer route.
  }
  if (config.mode === "local") assertFundingIsLocal(chain, chain.tokens[config.currency] ?? Object.values(chain.tokens)[0]!);

  const state = emptyState({
    runId, mode: config.mode, ...(config.target ? { target: config.target } : {}),
    apiOrigin: config.api.url, experiment: options.experiment, profile: options.profile,
    currency: config.currency, amountUnits: decimalToUnits(config.amount), epoch: 1,
  });

  const route = createRouter(journal, state);
  const sink: EventSink = (event) => route.sink(event);

  const apiKey = requireApiKey(config.mode);
  const clients = createClients({ apiOrigin: config.api.url, apiKey, sink, checkoutOrigin: config.checkoutOrigin });

  const seed = resolveSeed({
    mode: config.mode,
    seedEnvVar: config.wallets.seedEnvVar,
    // Local seeds persist across runs (funding outlives one run); real mode
    // requires an explicit env seed and never writes one.
    runPrivateDir: join(dir, "private"),
    sharedSeedFile: join(process.cwd(), "runs", ".payer-seed.txt"),
    runId,
  });
  const pool = new WalletPool(seed, { [chain.id]: chain }, config.wallets.count, (kind, text) => {
    if (kind === "warn") sink({ type: "warning", payload: { text } });
  });
  pool.assertNoAnvilKeys(config.mode);

  const observers = new Map<string, ChainObserver>();
  const observer = new ChainObserver(
    chain, config.currency, config.observation.pollMs, config.observation.logRangeBlocks,
    () => {}, // the router pushes observations to lifecycles; the engine wires callbacks below
    (error) => sink({ type: "warning", payload: { text: error } }),
  );
  observers.set(chain.id, observer);

  // One guard controller shared by the display, the signal handler, and the
  // engine — a stop from anywhere halts admission everywhere.
  const guards = new Guards(config.limits, (event) => route.sink(event), config.mode);
  const display = await createDisplay(options.display, state, journal, () => {
    if (!state.guard.stopReason) guards.stop("operator requested stop (q)");
  });
  const onSignal = (): void => guards.stop("interrupted (SIGINT); draining");
  process.on("SIGINT", onSignal);
  const plainTimer = display.kind === "tui" ? undefined : setInterval(() => display.update(), 2000);
  const engine = new Engine({
    options, config, runId, dir, journal, state, route, clients, pool, observers, display,
    chain, observer, guards,
  });

  try {
    await engine.preflight();
    await engine.execute();
    await engine.drain();
    journal.snapshot(state);
    return { runId, dir, state };
  } finally {
    process.off("SIGINT", onSignal);
    observer.stop();
    engine.stopObservers();
    if (plainTimer) clearInterval(plainTimer);
    display.stop();
    journal.close();
  }
}

/**
 * The router: the single pipeline from every producer (SDK transport, chain
 * observers, webhook receiver, guard) into the journal, the reducer, and the
 * lifecycle engines.
 */
function createRouter(journal: Journal, state: RunState) {
  const depositToOp = new Map<string, string>();
  const addressToOp = new Map<string, string>();
  const lifecycleHandlers = new Map<string, (observation: ChainObservation) => void>();
  const uiWaiters = new Set<() => void>();

  const router = {
    journal,
    state,
    depositToOp,
    addressToOp,
    lifecycleHandlers,
    waiters: uiWaiters,

    sink(event: JournalEvent): void {
      const mono = performance.now();
      journal.append(event.type, event.payload, mono);
      reduce(state, event, mono);
      if (event.type === "op.created") depositToOp.set((event.payload as { depositId: string }).depositId, (event.payload as { op: string }).op);
      if (event.type === "op.bound") addressToOp.set((event.payload as { address: string }).address.toLowerCase(), (event.payload as { op: string }).op);
      router.wake();
    },

    /** Webhook capture: durable blob first, then journal (write-ahead), then ack. */
    webhookCapture: (capture: { eventId: string; type: string; depositId?: string; raw: string }) => {
      const mono = performance.now();
      const blob = journal.blob(`webhook-${capture.eventId}`, capture.raw);
      const op = capture.depositId ? depositToOp.get(capture.depositId) : undefined;
      router.sink({ type: "webhook.received", payload: { eventId: capture.eventId, type: capture.type, ...(capture.depositId ? { depositId: capture.depositId } : {}), ...(op ? { op } : {}), blob } });
      if (!op) return;
      if (capture.type === "deposit_request.deposited") {
        router.sink({ type: "op.credited", payload: { op, source: "webhook" } });
      } else if (capture.type === "deposit_request.settled") {
        router.sink({ type: "op.settled.webhook", payload: { op, eventId: capture.eventId } });
      }
    },

    webhookInvalid: (reason: string): void => {
      router.sink({ type: "webhook.invalid", payload: { reason } });
    },

    chainObservation: (observation: ChainObservation): void => {
      const op = addressToOp.get(observation.address.toLowerCase());
      if (!op) return;
      lifecycleHandlers.get(op)?.(observation);
    },

    wake: (): void => {
      for (const waiter of uiWaiters) waiter();
    },
  };
  return router;
}

async function createDisplay(display: DisplayMode, state: RunState, journal: Journal, onQuit: () => void): Promise<Display> {
  const isTTY = process.stdout.isTTY && !process.env["CI"];
  const mode = display === "auto" ? (isTTY ? "tui" : "plain") : display;
  if (mode === "tui") {
    return renderLoop(state, () => buildView(state), {
      onQuit,
      journal,
    });
  }
  if (mode === "json") {
    return {
      kind: "json",
      stop: () => {},
      update: () => {
        process.stdout.write(`${JSON.stringify({ ts: new Date().toISOString(), phase: state.phase, admitted: state.admitted })}\n`);
      },
    };
  }
  return {
    kind: "plain",
    stop: () => {},
    update: () => {
      const settled = [...state.ops.values()].filter((op) => op.phase === "verified").length;
      process.stderr.write(`[${new Date().toISOString()}] phase=${state.phase} admitted=${state.admitted} settled=${settled}\n`);
    },
  };
}

class Engine {
  private apiObserver?: ApiObserver;
  private receiver?: WebhookReceiver;
  private phaseStartedMono = performance.now();
  private guards: Guards;
  /** Run-wide operation sequence; phases never share op identities. */
  private opOffset = 0;
  /** Live money reservations, released when the op reaches a terminal phase. */
  private reservations = new Map<string, Reservation>();

  constructor(
    private readonly ctx: {
      options: RunOptions;
      config: HarnessConfig;
      runId: string;
      dir: string;
      journal: Journal;
      state: RunState;
      route: ReturnType<typeof createRouter>;
      clients: Clients;
      pool: WalletPool;
      observers: Map<string, ChainObserver>;
      display: Display;
      chain: HarnessConfig["chains"][string];
      observer: ChainObserver;
      guards: Guards;
    },
  ) {
    ctx.observer.on((observation) => ctx.route.chainObservation(observation));
    this.guards = ctx.guards;
  }

  private get sink(): EventSink {
    return (event) => this.ctx.route.sink(event);
  }

  private setPhase(phase: RunPhase): void {
    this.sink({ type: "phase.changed", payload: { phase } });
    this.phaseStartedMono = performance.now();
  }

  async preflight(): Promise<void> {
    const { config, chain, observer, clients, state } = this.ctx;
    this.sink({ type: "run.started", payload: { runId: this.ctx.runId, mode: config.mode, experiment: this.ctx.options.experiment, profile: this.ctx.options.profile, chain: chain.id, api: config.api.url } });

    // Service health: /v1/status is the external diagnostic surface.
    const status = await clients.merchant.status().catch((error: unknown) => {
      throw new Error(`preflight: /v1/status failed: ${error instanceof Error ? error.message : String(error)}`);
    });
    state.statusSample = status as unknown as Record<string, unknown>;

    // Chain heads must progress before money moves (block times vary; poll
    // rather than assume a fixed cadence).
    const headBefore = await observer.head();
    const advanceDeadline = performance.now() + 10_000;
    let headAfter = headBefore;
    while (performance.now() < advanceDeadline) {
      await sleep(500);
      headAfter = await observer.head();
      if (headAfter > headBefore) break;
    }
    if (headAfter <= headBefore && headBefore !== 0n) {
      throw new Error(`preflight: chain ${chain.id} head did not advance in 10s (${headBefore} → ${headAfter})`);
    }

    // Token allowlist must match the deployment's canonical contract.
    const token = chain.tokens[config.currency];
    if (!token) throw new Error(`preflight: chain ${chain.id} has no ${config.currency} token configured`);

    if (config.mode === "local") {
      const plan = defaultFundingPlan(chain, config.currency, this.ctx.pool.all().length, BigInt(decimalToUnits(config.amount)));
      const publicClient = createPublicClient({ transport: http(chain.rpcUrl) });
      const first = this.ctx.pool.all()[0]!;
      const tokenBalance = (await publicClient.readContract({
        address: token as Address, abi: ERC20_ABI, functionName: "balanceOf", args: [first.address],
      }).catch(() => 0n)) as bigint;
      if (tokenBalance < plan.tokenUnitsPerWallet) {
        this.sink({ type: "note", payload: { text: `funding ${this.ctx.pool.all().length} payer wallets on chain ${chain.id} from the local funder` } });
        await fundLocalPayers({ pool: this.ctx.pool, chain, plan, currency: config.currency });
        this.sink({ type: "funding.record", payload: { chainId: chain.id, wallet: "pool", tokenUnits: plan.tokenUnitsPerWallet.toString(), txHashes: [] } });
      }
    }

    // Payer balances must cover the plan before the measured phase; checked
    // after local funding so the verdict reflects what will actually run.
    await this.checkBalances(chain, token);

    // Optional webhook endpoint: register once, verify with a test delivery.
    if (config.webhook.enabled && config.webhook.publicUrl) {
      await this.setupWebhook(config.webhook.publicUrl, config.webhook.listenPort);
    } else {
      this.sink({ type: "warning", payload: { text: "webhooks disabled: settlement timing uses merchant-API polling (coarser intervals)" } });
    }
  }

  private async checkBalances(chain: HarnessConfig["chains"][string], token: string): Promise<void> {
    const publicClient = createPublicClient({ transport: http(chain.rpcUrl) });
    const wallets = this.ctx.pool.all();
    let funded = 0;
    for (const wallet of wallets) {
      const balance = (await publicClient.readContract({
        address: token as Address, abi: ERC20_ABI, functionName: "balanceOf", args: [wallet.address],
      }).catch(() => 0n)) as bigint;
      if (balance >= BigInt(decimalToUnits(this.ctx.config.amount))) funded += 1;
    }
    if (funded < wallets.length) {
      const text = `only ${funded}/${wallets.length} payer wallets hold enough ${this.ctx.config.currency} on chain ${chain.id}`;
      if (this.ctx.config.mode === "local" && this.ctx.options.fundLocal) return; // funding happens below
      this.sink({ type: "warning", payload: { text: `${text}; run with --fund-local (local) or fund the pool explicitly` } });
    }
  }

  private async setupWebhook(publicUrl: string, listenPort: number): Promise<void> {
    const receiver = new WebhookReceiver(this.ctx.route.webhookCapture, this.ctx.route.webhookInvalid);
    await receiver.start(listenPort);
    this.receiver = receiver;

    // Reuse a secret from a previous run of this run-directory when present.
    const existing = await this.ctx.clients.merchant.webhooks.list().catch(() => null);
    const match = existing?.webhooks?.find((endpoint) => endpoint.url === publicUrl);
    const secretBlob = join(this.ctx.dir, "private", "webhook-secret.json");
    let secret: string | undefined;
    if (match) {
      try {
        secret = JSON.parse(readPrivateJson(this.ctx.journal, "webhook-secret.json") ?? "{}")["secret"];
      } catch {
        secret = undefined;
      }
      void secretBlob;
      if (!secret) throw new Error(`webhook endpoint ${publicUrl} already exists but its signing secret is not in this run's private/ directory`);
    } else {
      const created = await this.ctx.clients.merchant.webhooks.add(publicUrl);
      secret = created.secret;
      this.ctx.journal.blob("webhook-secret.json", JSON.stringify({ url: publicUrl, secret }));
    }
    if (!secret) throw new Error("webhook endpoint returned no signing secret");
    receiver.registerSecret(secret);

    // Prove the path end to end before measurement.
    if (match) await this.ctx.clients.merchant.webhooks.test(match.id);
    else await this.ctx.clients.merchant.webhooks.test((await this.ctx.clients.merchant.webhooks.list()).webhooks.find((endpoint) => endpoint.url === publicUrl)!.id);
    const deadline = performance.now() + 10_000;
    while (performance.now() < deadline) {
      await sleep(250);
      if (this.ctx.state.webhooksReceived > 0) {
        this.sink({ type: "webhook.verified_endpoint", payload: {} });
        this.ctx.state.webhookUrl = publicUrl;
        this.ctx.state.webhookDeliveryOk = true;
        this.ctx.state.webhooksReceived = 0;
        return;
      }
    }
    throw new Error("webhook test delivery was not received; check the tunnel and endpoint URL");
  }

  async execute(): Promise<void> {
    const { options, config } = this.ctx;
    this.startObservers();
    if (options.warmup > 0) {
      this.setPhase("warmup");
      await this.dispatch(openLoop(options.warmup, Math.max(options.rate ?? 0.2, 0.2), { jitter: true }), { warmup: true });
    }
    this.setPhase("measure");
    if (options.experiment === "latency") {
      await this.dispatch(openLoop(options.count, options.rate ?? 0.2, { jitter: options.jitter }), { warmup: false });
    } else if (options.experiment === "capacity60") {
      await this.capacity60();
    } else {
      await this.soak();
    }
    if (options.display !== "tui") this.ctx.display.update();
  }

  private async dispatch(entries: Array<{ index: number; atMs: number; warmup: boolean }>, context: { warmup: boolean }): Promise<void> {
    const { config, state } = this.ctx;
    const t0 = performance.now();
    const running: Promise<void>[] = [];
    let dispatched = 0;
    for (const entry of entries) {
      const target = t0 + entry.atMs;
      const wait = target - performance.now();
      if (wait > 0) await sleep(wait);
      const reason = this.guards.canAdmit();
      if (reason) {
        this.sink({ type: "warning", payload: { text: `admission stopped: ${reason}` } });
        break;
      }
      // Run-wide identity: warm-up and measured phases never share an op name,
      // idempotency key, reference, or signed-tx blob.
      const opIndex = this.opOffset + entry.index;
      const plan = {
        op: `op-${opIndex}`,
        index: opIndex,
        warmup: context.warmup,
        currency: config.currency,
        amount: config.amount,
        chainId: this.ctx.chain.id,
        profile: this.ctx.options.profile,
        issuerId: `bench-${this.ctx.runId}`,
        reference: `bench:${this.ctx.runId}:${opIndex}`,
        payout: config.payout.mode === "fixed" ? config.payout.address : this.ctx.pool.all()[entry.index % this.ctx.pool.all().length]!.address,
        scheduledMono: target,
      };
      // Money exposure is reserved before any external effect; a breach stops
      // admission (nothing already funded is rolled back).
      let reservation;
      try {
        reservation = this.guards.reserve(plan.op, plan.amount);
      } catch (error) {
        this.sink({ type: "warning", payload: { text: `admission stopped: ${error instanceof Error ? error.message : String(error)}` } });
        break;
      }
      this.reservations.set(plan.op, reservation);
      const lifecycle = new Lifecycle(plan, this.ctx.clients, this.ctx.pool, this.ctx.chain, this.ctx.observers, this.sink, state, () => performance.now(), (name, contents) => this.ctx.journal.blob(name, contents));
      this.ctx.route.lifecycleHandlers.set(plan.op, async (observation) => {
        // Fill block-clock evidence before the reducer records completion.
        if (!observation.evidence.blockTime) {
          const time = await this.ctx.observer.ensureBlockTime(observation.evidence.blockNumber);
          if (time) observation.evidence.blockTime = time;
        }
        lifecycle.onObservation(observation);
      });
      running.push(lifecycle.run().catch(() => {}));
      dispatched += 1;
      if (dispatched % 5 === 0) this.ctx.display.update();
    }
    await Promise.all(running);
    this.opOffset += entries.length;
  }

  private async capacity60(): Promise<void> {
    const { options, state } = this.ctx;
    const trials: Array<{ cohort: number; settledBy60: number; pass: boolean }> = [];
    for (let trial = 0; trial < options.repeat; trial++) {
      const t0 = performance.now();
      state.capacity = {
        t0Mono: t0, deadlineMono: t0 + 60_000, frozen: false,
        settledBy60: { api: 0, webhook: 0, chain: 0 },
        previousTrials: trials,
      };
      // Membership is fixed before dispatch so a trial's count can never
      // include earlier trials' or warm-up operations. cohort() restarts at
      // index 0; the run-wide opOffset makes identities unique.
      const cohortOps = new Set(Array.from({ length: options.count }, (_, i) => `op-${this.opOffset + i}`));
      const entries = cohort(options.count, 0);
      await this.dispatch(entries, { warmup: false });
      // Observe through the freeze; keep draining later.
      while (performance.now() < state.capacity.deadlineMono) {
        await sleep(500);
        this.ctx.display.update();
        this.pollFinality();
      }
      const counts = this.countSettledBy(state, cohortOps, state.capacity.deadlineMono);
      state.capacity.frozen = true;
      const pass = counts.api === options.count && counts.chain === options.count;
      // The freeze is an event so a replayed journal reproduces the result.
      this.sink({ type: "capacity.frozen", payload: { t0Mono: state.capacity.t0Mono, deadlineMono: state.capacity.deadlineMono, cohort: options.count, pass, settledBy60: counts } });
      trials.push({ cohort: options.count, settledBy60: counts.api, pass });
      this.sink({ type: "note", payload: { text: `capacity60 cohort ${options.count}: ${counts.api}/${options.count} settled via API, ${counts.chain} seen on-chain by 60s (${pass ? "pass" : "fail"})` } });
      // Drain this cohort fully before the next trial so queues start empty.
      await this.drainOnce();
    }
  }

  private countSettledBy(state: RunState, members: Set<string>, deadlineMono: number): { api: number; webhook: number; chain: number } {
    let api = 0;
    let webhook = 0;
    let chain = 0;
    for (const name of members) {
      const op = state.ops.get(name);
      if (!op) continue;
      if (op.timings.settled_api_seen && op.timings.settled_api_seen.mono <= deadlineMono) api += 1;
      if (op.timings.settled_webhook_received && op.timings.settled_webhook_received.mono <= deadlineMono) webhook += 1;
      if (op.timings.payout_inclusion && op.timings.payout_inclusion.mono <= deadlineMono) chain += 1;
    }
    return { api, webhook, chain };
  }

  private async soak(): Promise<void> {
    const { options } = this.ctx;
    if (!options.rate || !options.durationSeconds) throw new Error("soak requires --rate and --duration");
    const rate = options.rate;
    const durationSeconds = options.durationSeconds;
    // The workload is rate × duration; --count only ever caps it below that
    // (it never truncates a soak to a handful of arrivals).
    const total = Math.ceil(rate * durationSeconds) + 1;
    const cap = options.count > 0 ? Math.min(options.count, total) : total;
    const entries = openLoop(cap, rate, { jitter: options.jitter });
    const stopAt = performance.now() + durationSeconds * 1000;
    const capped = entries.filter((entry) => entry.atMs <= durationSeconds * 1000);
    await this.dispatchWithDeadline(capped, stopAt);
  }

  private async dispatchWithDeadline(entries: Array<{ index: number; atMs: number; warmup: boolean }>, stopAtMono: number): Promise<void> {
    // Identical to dispatch but abandons future arrivals at the wall clock so
    // a soak admits for exactly its duration.
    const truncated = entries.filter((entry) => performance.now() + Math.max(0, entry.atMs) <= stopAtMono);
    await this.dispatch(truncated, { warmup: false });
  }

  /** Advances finality stamps; cheap, called from wait loops. */
  private pollFinality(): void {
    const observer = this.ctx.observer;
    const state = this.ctx.state;
    const mono = performance.now();
    for (const op of state.ops.values()) {
      if (op.chainId !== this.ctx.chain.id) continue;
      const payment = op.timings.payment_inclusion;
      if (payment?.block && !op.timings.payment_finality_seen && observer.isFinalized(payment.block.blockNumber)) {
        this.sink({ type: "op.payment.finalized", payload: { op: op.op, boundaryBlock: observer.boundary } });
      }
      const payout = op.timings.payout_inclusion;
      if (payout?.block && !op.timings.payout_finality_seen && observer.isFinalized(payout.block.blockNumber)) {
        this.sink({ type: "op.payout.finalized", payload: { op: op.op, boundaryBlock: observer.boundary } });
      }
      void this.maybeVerify(op, performance.now()).catch(() => {});
    }
  }

  private verifying = new Set<string>();

  private async maybeVerify(op: NonNullable<ReturnType<RunState["ops"]["get"]>>, _mono: number): Promise<void> {
    if (op.phase !== "settled") return;
    const payout = op.timings.payout_inclusion;
    const settledApi = op.timings.settled_api_seen;
    if (!payout?.block || !settledApi || !op.depositId) return;
    const observer = this.ctx.observer;
    // Verified requires Gum's finality boundary to have crossed the payout
    // evidence — height checks alone do not establish settlement.
    if (!observer.isFinalized(payout.block.blockNumber)) return;
    if (this.verifying.has(op.op)) return;
    this.verifying.add(op.op);
    try {
      const payoutTime = await observer.ensureBlockTime(payout.block.blockNumber);
      if (payoutTime && !payout.block.blockTime) payout.block.blockTime = payoutTime;
      const payment = op.timings.payment_inclusion;
      if (payment?.block) {
        const paymentTime = await observer.ensureBlockTime(payment.block.blockNumber);
        if (paymentTime && !payment.block.blockTime) payment.block.blockTime = paymentTime;
      }
      const detail = await this.apiObserver?.detail(op.depositId).catch(() => null);
      const checks: string[] = [];
      // A failed or missing detail read is not a pass; it retries on the next
      // poll so a transient API error never fakes verification.
      if (!detail) return;
      const status = (detail as { status?: string }).status;
      if (status === "settled") checks.push("api.status=settled");
      const payoutAddress = (detail as { payout_address?: string }).payout_address;
      if (payoutAddress && op.payout && payoutAddress.toLowerCase() === op.payout.toLowerCase()) checks.push("api.payout_address matches");
      if (checks.length === 0) return;
      if (payout.block.blockTime && payment?.block?.blockTime) checks.push("chain evidence paired");
      this.sink({ type: "op.verified", payload: { op: op.op, checks } });
    } finally {
      this.verifying.delete(op.op);
    }
  }

  async drain(): Promise<void> {
    this.setPhase("drain");
    await this.drainOnce();
    this.stopObservers();
    this.writeArtifacts();
  }

  private async drainOnce(): Promise<void> {
    const { config, state } = this.ctx;
    this.startObservers();
    const timeoutSeconds = config.limits.perOpTimeoutSeconds;
    // An operator stop shortens the drain: keep collecting briefly, then
    // report whatever remains outstanding.
    const drainBudget = state.guard.stopReason ? Math.min(timeoutSeconds, 30) : timeoutSeconds;
    const deadline = performance.now() + drainBudget * 1000;
    while (performance.now() < deadline) {
      this.pollFinality();
      this.releaseFinishedReservations();
      const outstanding = [...state.ops.values()].filter((op) => op.phase !== "verified" && op.phase !== "failed" && op.phase !== "timed_out" && op.outcome === undefined);
      if (outstanding.length === 0) break;
      const oldest = outstanding.map((op) => op.timings.payment_submit_ack?.mono).reduce<number | undefined>((acc, value) => (acc === undefined ? value : Math.min(acc, value ?? Number.MAX_SAFE_INTEGER)), undefined);
      const ageSeconds = oldest !== undefined ? (performance.now() - oldest) / 1000 : undefined;
      const guard = this.guards;
      guard.checkOldestUnsettled(ageSeconds);
      this.ctx.display.update();
      await sleep(config.observation.apiPollMs);
      // Time out individual ops that will never complete.
      for (const op of outstanding) {
        const start = op.timings.create_start?.mono ?? 0;
        if (start > 0 && performance.now() - start > timeoutSeconds * 1000 && op.phase !== "verified") {
          this.sink({ type: "op.timed_out", payload: { op: op.op } });
        }
      }
    }
    // Observers stay running: capacity60 drains between trials, and the next
    // trial needs the same observation streams. run()'s finally stops them.
  }

  /** Releases money reservations for ops that reached a terminal phase. */
  private releaseFinishedReservations(): void {
    for (const [name, reservation] of this.reservations) {
      const op = this.ctx.state.ops.get(name);
      if (!op) {
        reservation.release();
        this.reservations.delete(name);
        continue;
      }
      if (op.phase === "verified" || op.phase === "failed" || op.phase === "timed_out" || op.outcome !== undefined) {
        reservation.release();
        this.reservations.delete(name);
      }
    }
  }

  private startObservers(): void {
    const { config, observer, state } = this.ctx;
    if (this.apiObserver) return;
    this.apiObserver = new ApiObserver(
      this.ctx.clients, `bench-${this.ctx.runId}`, config.observation.apiPollMs, this.sink, state,
      () => performance.now(),
      (sample) => { state.statusSample = sample; },
    );
    this.apiObserver.start();
    void observer.start().catch((error: unknown) => {
      this.sink({ type: "warning", payload: { text: `observer start failed: ${String(error)}` } });
    });
    // Finality polling needs a heartbeat independent of the drain loop.
    const runtime = chainRuntime(state, this.ctx.chain.id, this.ctx.chain.name, config.currency);
    const heartbeat = async (): Promise<void> => {
      if (!this.apiObserver) return;
      runtime.finalizedBlock = this.ctx.observer.boundary;
      runtime.rpcCalls = this.ctx.observer.rpcCalls;
      runtime.observerErrors = this.ctx.observer.observerErrors;
      runtime.watched = this.ctx.observer.watchedPaymentCount;
      const head = await this.ctx.observer.head().catch(() => null);
      if (head !== null) {
        runtime.headBlock = Number(head);
        runtime.headAtMono = performance.now();
      }
      this.pollFinality();
      setTimeout(() => void heartbeat(), config.observation.pollMs);
    };
    void heartbeat();
  }

  stopObservers(): void {
    this.apiObserver?.stop();
    this.apiObserver = undefined;
    this.ctx.observer.stop();
  }

  private writeArtifacts(): void {
    this.setPhase("complete");
    writeReport(this.ctx.journal, this.ctx.state, this.ctx.dir, {
      experiment: this.ctx.options.experiment,
      chain: this.ctx.chain,
      currency: this.ctx.config.currency,
      amount: this.ctx.config.amount,
      mode: this.ctx.config.mode,
    });
  }
}

function readPrivateJson(journal: Journal, name: string): string | null {
  try {
    return journal.blobContents(`private/${name}`);
  } catch {
    return null;
  }
}

export { stamp, replay };
