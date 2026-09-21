/**
 * Offline reporting: replay the run journal and produce summary.json,
 * report.md, deposits.csv, and unresolved-funds.json. Deterministic: the same
 * journal always yields byte-identical reports. Credentials, signed
 * transactions, and raw webhook bodies never enter these artifacts.
 */

import { writeFileSync } from "node:fs";
import { join } from "node:path";
import { Journal, readRecords } from "./journal.js";
import { emptyState, reduce, type Experiment, type JournalEvent, type OpState, type RunState } from "./model.js";
import type { ChainConfig } from "./chains.js";
import { distribution, fmtUnits, fmtSeconds } from "./metrics.js";
import { secondsBetween } from "./model.js";

export interface ReportMeta {
  experiment: Experiment;
  chain: ChainConfig;
  currency: string;
  amount: string;
  mode: "local" | "real";
}

export function writeReport(journal: Journal, state: RunState, dir: string, meta: ReportMeta): void {
  const summary = buildSummary(state, meta);
  writeFileSync(join(dir, "summary.json"), JSON.stringify(summary, null, 2));
  writeFileSync(join(dir, "report.md"), renderMarkdown(summary, state, meta));
  writeFileSync(join(dir, "deposits.csv"), renderCsv(state));
  // Anything that signed or broadcast a payment and has not reached a settled
  // or verified end state may hold funds in flight — timed_out included.
  const unresolved = [...state.ops.values()].filter((op) => op.txHash && op.phase !== "verified" && op.phase !== "settled");
  if (unresolved.length > 0) {
    writeFileSync(
      join(dir, "unresolved-funds.json"),
      JSON.stringify(
        unresolved.map((op) => ({
          op: op.op, deposit_id: op.depositId, chain: op.chainId, amount_units: op.amountUnits,
          payment_tx: op.txHash, sweep_tx: op.sweepTxHash, phase: op.phase, wallet: op.wallet,
        })),
        null, 2,
      ),
    );
  }
  void journal;
}

export interface RunSummary {
  run: { id: string; mode: string; experiment: string; profile: string; apiOrigin: string; started: string };
  workload: { chain: string; currency: string; amount: string; scheduled: number; admitted: number };
  latency?: {
    samples: number;
    paymentToPayout: ReturnType<typeof distribution>;
    observedFinalized: ReturnType<typeof distribution>;
    merchantSeen: ReturnType<typeof distribution>;
    /** Payment finalized on-chain → merchant API sees settled: the system's
     * own latency, chain inclusion and finality time excluded. */
    attributable: ReturnType<typeof distribution>;
    exploratoryP99: boolean;
  };
  capacity60?: { cohort: number; settledApi: number; settledWebhook: number; settledChain: number; pass: boolean };
  perOp: Array<Record<string, number | string | undefined>>;
  warnings: string[];
  totals: { gross: string; outstanding: string; apiCalls: number; webhooks: number };
}

export function buildSummary(state: RunState, meta: ReportMeta): RunSummary {
  const ops = [...state.ops.values()].filter((op) => !op.warmup);
  const e2e = ops.flatMap((op) => {
    const seconds = secondsBetween(op, "payment_inclusion", "payout_inclusion");
    return seconds === undefined ? [] : [seconds];
  });
  const observed = ops.flatMap((op) => {
    const seconds = secondsBetween(op, "payment_first_seen", "payout_finality_seen");
    return seconds === undefined ? [] : [seconds];
  });
  const merchant = ops.flatMap((op) => {
    const payment = op.timings.payment_inclusion;
    const settled = op.timings.settled_api_seen;
    if (!payment?.block || !settled) return [];
    return [(settled.mono - 0) / 1000]; // replaced below with wall-clock delta
  });
  // Merchant-visible latency uses UTC wall clocks: payment block time → the
  // observed settled API poll.
  const merchant2 = ops.flatMap((op) => {
    const payment = op.timings.payment_inclusion;
    const settled = op.timings.settled_api_seen;
    if (!payment?.block || !settled) return [];
    const paymentWall = Date.parse(payment.block.blockTime);
    const settledWall = Date.parse(settled.wall);
    return [(settledWall - paymentWall) / 1000];
  });
  void merchant;
  // The system's own latency, measured the way the operator asked for: from
  // the moment the payment is finalized on-chain — chain time excluded — to
  // the merchant API reporting the deposit settled. Chain inclusion and
  // finality are the blockchain's schedule, not ours; detection, sweeping
  // and settlement reporting are the system under test.
  const attributable = ops.flatMap((op) => {
    const seconds = wallSeconds(op, "payment_finality_seen", "settled_api_seen");
    return seconds === undefined ? [] : [seconds];
  });

    const perOp = ops.map((op) => ({
    op: op.op,
    deposit_id: op.depositId,
    chain: op.chainId,
    profile: op.warmup ? "warmup" : meta.experiment,
    payment_to_payout_s: secondsBetween(op, "payment_inclusion", "payout_inclusion"),
    detection_to_final_payout_s: secondsBetween(op, "payment_first_seen", "payout_finality_seen"),
    finality_to_settled_s: wallSeconds(op, "payment_finality_seen", "settled_api_seen"),
    finality_to_credited_s: wallSeconds(op, "payment_finality_seen", "credited_api_seen"),
    payment_to_settled_api_s: wallSeconds(op, "payment_inclusion", "settled_api_seen"),
    create_to_settled_api_s: wallSeconds(op, "create_start", "settled_api_seen"),
    outcome: op.phase,
    error: op.error,
    payment_tx: op.txHash,
    sweep_tx: op.sweepTxHash,
  }));

  const capacity = state.capacity
    ? {
        cohort: state.capacity.cohort ?? state.scheduled,
        settledApi: state.capacity.settledBy60.api,
        settledWebhook: state.capacity.settledBy60.webhook,
        settledChain: state.capacity.settledBy60.chain,
        pass: state.capacity.pass ?? state.capacity.settledBy60.api >= (state.capacity.cohort ?? state.scheduled),
      }
    : undefined;

  return {
    run: {
      id: state.runId, mode: state.mode, experiment: state.experiment, profile: state.profile,
      apiOrigin: state.apiOrigin, started: state.startedWall,
    },
    workload: { chain: meta.chain.id, currency: meta.currency, amount: meta.amount, scheduled: state.scheduled, admitted: state.admitted },
    ...(state.experiment !== "capacity60"
      ? {
          latency: {
            samples: e2e.length,
            paymentToPayout: distribution(e2e),
            observedFinalized: distribution(observed),
            merchantSeen: distribution(merchant2),
            attributable: distribution(attributable),
            exploratoryP99: e2e.length < 1000,
          },
        }
      : {}),
    ...(capacity ? { capacity60: capacity } : {}),
    perOp,
    warnings: state.warnings,
    totals: {
      gross: fmtUnits(state.guard.grossUnits),
      outstanding: fmtUnits(state.guard.outstandingPrincipalUnits),
      apiCalls: state.apiCalls,
      webhooks: state.webhooksReceived,
    },
  };
}

function wallSeconds(op: OpState, from: Parameters<typeof secondsBetween>[1], to: Parameters<typeof secondsBetween>[1]): number | undefined {
  const a = op.timings[from];
  const b = op.timings[to];
  if (!a || !b) return undefined;
  if (a.block && to === "settled_api_seen") return (Date.parse(b.wall) - Date.parse(a.block.blockTime)) / 1000;
  return (b.mono - a.mono) / 1000;
}

function renderMarkdown(summary: RunSummary, state: RunState, meta: ReportMeta): string {
  const lines: string[] = [];
  lines.push(`# gum-load report — ${summary.run.id}`, "");
  lines.push(`- Mode: **${summary.run.mode}**${summary.run.mode === "real" ? ` (${summary.run.apiOrigin})` : ""}, experiment **${summary.run.experiment}**, profile **${summary.run.profile}**`);
  lines.push(`- Chain: **${meta.chain.name} (${meta.chain.id})**, ${meta.currency} @ ${meta.amount}, finality: ${meta.chain.finality.kind}${meta.chain.finality.kind === "confirmations" ? ` − ${meta.chain.finality.margin} blocks (Gum confirmation policy, not L1 finality)` : " tag"}`);
  lines.push(`- Started: ${summary.run.started}`, "");

  if (summary.latency) {
    const l = summary.latency;
    lines.push("## Settlement latency", "");
    lines.push("| metric | n | p50 | p95 | p99 |", "|---|---|---|---|---|");
    lines.push(`| payment landed → payout landed | ${l.paymentToPayout.count} | ${fmtSeconds(l.paymentToPayout.p50)} | ${fmtSeconds(l.paymentToPayout.p95)} | ${fmtSeconds(l.paymentToPayout.p99)}${l.exploratoryP99 ? " (exploratory, n<1000)" : ""} |`);
    lines.push(`| observed payment → observed finalized payout | ${l.observedFinalized.count} | ${fmtSeconds(l.observedFinalized.p50)} | ${fmtSeconds(l.observedFinalized.p95)} | ${fmtSeconds(l.observedFinalized.p99)} |`);
    lines.push(`| payment landed → merchant API sees settled | ${l.merchantSeen.count} | ${fmtSeconds(l.merchantSeen.p50)} | ${fmtSeconds(l.merchantSeen.p95)} | ${fmtSeconds(l.merchantSeen.p99)} |`);
    lines.push(`| payment finalized → merchant API sees settled (system latency, chain time excluded) | ${l.attributable.count} | ${fmtSeconds(l.attributable.p50)} | ${fmtSeconds(l.attributable.p95)} | ${fmtSeconds(l.attributable.p99)} |`, "");
  }
  if (summary.capacity60) {
    lines.push("## Capacity60", "");
    lines.push(`Cohort **${summary.capacity60.cohort}** at a common T0: **${summary.capacity60.settledApi}** settled via API, ${summary.capacity60.settledWebhook} via webhook, ${summary.capacity60.settledChain} on-chain within 60 seconds — ${summary.capacity60.pass ? "**pass**" : "**fail**"}.`, "");
  }
  lines.push("## Totals", "");
  lines.push(`Gross transferred ${summary.totals.gross} ${meta.currency}, outstanding ${summary.totals.outstanding}, ${summary.totals.apiCalls} API calls, ${summary.totals.webhooks} webhooks received.`, "");
  if (state.warnings.length > 0) {
    lines.push("## Warnings", "");
    for (const warning of state.warnings) lines.push(`- ${warning}`);
    lines.push("");
  }
  const unresolved = summary.perOp.filter((op) => op.payment_tx && op.outcome !== "verified" && op.outcome !== "failed");
  if (unresolved.length > 0) lines.push(`**${unresolved.length} lifecycles remain unresolved** — see unresolved-funds.json.`, "");
  return lines.join("\n");
}

function renderCsv(state: RunState): string {
  const header = "op,deposit_id,chain,outcome,payment_tx,sweep_tx,payment_to_payout_s";
  const rows = [...state.ops.values()].map((op) => [
    op.op, op.depositId ?? "", op.chainId ?? "", op.phase, op.txHash ?? "", op.sweepTxHash ?? "",
    secondsBetween(op, "payment_inclusion", "payout_inclusion")?.toFixed(3) ?? "",
  ].join(","));
  return [header, ...rows].join("\n");
}

/** Resume support: rebuild state from a journal and drain outstanding work. */
export function stateFromJournal(path: string): RunState {
  const records = readRecords(path, { lenientTail: true });
  let state: RunState | undefined;
  for (const record of records) {
    if (record.type === "run.started") {
      const payload = record.payload as { runId: string; mode: "local" | "real"; experiment: Experiment; profile: RunState["profile"]; api: string };
      state = emptyState({
        runId: payload.runId, mode: payload.mode, experiment: payload.experiment,
        profile: payload.profile, apiOrigin: payload.api, currency: "", amountUnits: "0", epoch: record.epoch,
      });
    }
    if (!state) continue;
    reduce(state, record as unknown as JournalEvent, record.mono, record.wall);
  }
  if (!state) throw new Error(`journal ${path} has no run.started record`);
  return state;
}
