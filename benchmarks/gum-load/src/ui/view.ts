/**
 * Evidence → rows. The one place that decides what the run looks like: a
 * header, a summary strip, the experiment panel (latency histogram or the
 * capacity60 countdown), a health column, the oldest-unresolved lifecycle
 * table, and a footer. Colors degrade under NO_COLOR and narrow widths.
 */

import type { RunState, OpState } from "../model.js";
import { ALL_POINTS } from "../model.js";
import { distribution, fmtSeconds, fmtUnits, histogram, sparkline, rate } from "../metrics.js";

const noColor = process.env["NO_COLOR"] !== undefined;
const c = (code: string, text: string): string => (noColor ? text : `\x1b[${code}m${text}\x1b[0m`);
const bold = (text: string): string => c("1", text);
const dim = (text: string): string => c("2", text);
const green = (text: string): string => c("32", text);
const yellow = (text: string): string => c("33", text);
const red = (text: string): string => c("31", text);
const cyan = (text: string): string => c("36", text);
const magenta = (text: string): string => c("35", text);

const PHASE_COLORS: Record<string, (text: string) => string> = {
  preflight: yellow, warmup: magenta, measure: cyan, drain: yellow, complete: green, failed: red,
};

export function buildView(state: RunState): string[] {
  const rows: string[] = [];
  const width = process.stdout.columns || 120;
  pushHeader(state, rows);
  pushSummary(state, rows);
  rows.push("");

  const leftWidth = Math.max(46, Math.floor(width * 0.62));
  const rightWidth = width - leftWidth - 1;
  const left = buildLeftPanel(state, leftWidth);
  const right = buildRightPanel(state, rightWidth);
  const height = Math.max(left.length, right.length);
  for (let i = 0; i < height; i++) {
    rows.push(padRow(left[i] ?? "", leftWidth) + dim("│") + (right[i] ?? ""));
  }

  rows.push("");
  pushTable(state, rows);
  rows.push("");
  pushFooter(state, rows);
  return rows;
}

function pushHeader(state: RunState, rows: string[]): void {
  const phase = (PHASE_COLORS[state.phase] ?? dim)(` ${state.phase.toUpperCase()} `);
  const elapsed = ((performance.now() - (state.ops.size > 0 ? Math.min(...[...state.ops.values()].map((op) => op.timings.create_start?.mono ?? performance.now())) : performance.now())) / 1000);
  const title = bold("gum-load") + dim(" ● ") + `run ${state.runId}`;
  const meta = dim(`${state.mode}${state.target ? `/${state.target}` : ""} · ${state.experiment} · ${state.profile} · ${state.apiOrigin.replace(/^https?:\/\//, "")}`);
  const clock = dim(`elapsed ${fmtClock(elapsed * 1000)}`);
  rows.push(`${title}   ${meta}   ${phase}   ${clock}`);
}

function pushSummary(state: RunState, rows: string[]): void {
  const ops = [...state.ops.values()];
  const funded = ops.filter((op) => op.txHash).length;
  const settled = ops.filter((op) => op.timings.settled_api_seen).length;
  const verified = ops.filter((op) => op.phase === "verified").length;
  const failed = ops.filter((op) => op.phase === "failed" || op.phase === "timed_out").length;
  rows.push([
    `scheduled ${bold(String(state.scheduled))}`,
    `paid ${bold(String(funded))}`,
    `api-settled ${bold(String(settled))}`,
    green(`verified ${bold(String(verified))}`),
    (failed > 0 ? red : dim)(`failed ${failed}`),
    `outstanding ${bold(fmtUnits(state.guard.outstandingPrincipalUnits))} ${state.currency}`,
    dim(`gross ${fmtUnits(state.guard.grossUnits)}`),
  ].join(dim("  ·  ")));
}

function buildLeftPanel(state: RunState, width: number): string[] {
  if (state.experiment === "capacity60") return capacityPanel(state, width);
  if (state.experiment === "soak") return soakPanel(state, width);
  return latencyPanel(state, width);
}

function latencyPanel(state: RunState, width: number): string[] {
  const rows: string[] = [];
  rows.push(bold("Settlement latency (payment landed → payout landed)"));
  for (const [chainId, runtime] of state.chains) {
    if (runtime.e2e.length === 0 && runtime.detection.length === 0) {
      rows.push(`  ${chainId}: ${dim("no completed samples yet")}`);
      continue;
    }
    const e2e = distribution(runtime.e2e);
    const det = distribution(runtime.detection);
    rows.push(`  ${bold(chainId)} ${dim(runtime.currency)}  ${dim(`n=${e2e.count}`)}`);
    rows.push(`    p50 ${cyan(fmtSeconds(e2e.p50))}   p95 ${cyan(fmtSeconds(e2e.p95))}   p99 ${e2e.count >= 1000 ? cyan(fmtSeconds(e2e.p99)) : yellow(`${fmtSeconds(e2e.p99)}*`)}   max ${fmtSeconds(e2e.max)}`);
    if (e2e.count > 0 && e2e.count < 1000) rows.push(`    ${dim("* exploratory p99: fewer than 1,000 samples")}`);
    rows.push(`    detection→payout ${dim("p50")} ${fmtSeconds(det.p50)}  ${dim("p95")} ${fmtSeconds(det.p95)}   ${dim("last 40:")} ${sparkline(runtime.recentCompletions, Math.max(10, width - 46))}`);
    rows.push("");
    for (const { label, bar, count } of histogram(runtime.e2e, 8, Math.max(10, Math.floor(width / 3)))) {
      rows.push(`    ${dim(label.padEnd(14))} ${bar} ${dim(String(count))}`);
    }
  }
  return rows;
}

function capacityPanel(state: RunState, width: number): string[] {
  const rows: string[] = [];
  const capacity = state.capacity;
  rows.push(bold("Capacity60 — one cohort at T0, settled within 60 seconds"));
  if (!capacity) {
    rows.push(dim("  waiting for the cohort…"));
    return rows;
  }
  if (!capacity.frozen) {
    const remaining = Math.max(0, capacity.deadlineMono - performance.now());
    rows.push(`  ${bold(magenta(fmtClock(remaining)))} ${dim("remaining in the window")}`);
  } else {
    rows.push(`  ${green("window frozen")} ${dim("— draining late completions")}`);
  }
  rows.push(`  cohort ${bold(String(state.scheduled))}   settled via API ${bold(String(capacity.settledBy60.api))}   via webhook ${bold(String(capacity.settledBy60.webhook))}   on-chain ${bold(String(capacity.settledBy60.chain))}`);
  const pct = state.scheduled > 0 ? capacity.settledBy60.api / state.scheduled : 0;
  const barWidth = Math.max(10, width - 30);
  rows.push(`  [${"█".repeat(Math.round(pct * barWidth)).padEnd(barWidth, " ")}] ${(pct * 100).toFixed(0)}%`);
  if (capacity.previousTrials.length > 0) {
    rows.push(`  ${dim("previous cohorts:")} ${capacity.previousTrials.map((trial) => `${trial.cohort}→${trial.settledBy60} ${trial.pass ? green("pass") : red("fail")}`).join("  ")}`);
  }
  return rows;
}

function soakPanel(state: RunState, width: number): string[] {
  const rows: string[] = [];
  rows.push(bold("Soak — open-loop arrivals, backlog and latency"));
  const ops = [...state.ops.values()];
  const settled = ops.filter((op) => op.phase === "verified" || op.timings.settled_api_seen).length;
  const outstanding = ops.filter((op) => op.txHash && !op.timings.settled_api_seen).length;
  const elapsed = ((performance.now() - Math.min(...ops.map((op) => op.timings.create_start?.mono ?? performance.now()))) / 1000) || 1;
  rows.push(`  admitted ${ops.length}   settled ${bold(String(settled))}   outstanding ${outstanding > 0 ? yellow(String(outstanding)) : "0"}   ${dim(`throughput ${rate(settled, elapsed)}`)}`);
  rows.push(`  ${dim("recent completions:")} ${sparkline(ops.map((op) => (op.timings.payout_inclusion?.block && op.timings.payment_inclusion?.block ? 1 : 0)), Math.max(10, width - 30))}`);
  for (const [chainId, runtime] of state.chains) {
    const e2e = distribution(runtime.e2e);
    rows.push(`  ${bold(chainId)} p50 ${fmtSeconds(e2e.p50)}  p95 ${fmtSeconds(e2e.p95)}  ${dim(`n=${e2e.count}`)}`);
  }
  return rows;
}

function buildRightPanel(state: RunState, width: number): string[] {
  const rows: string[] = [];
  rows.push(bold("Health"));
  for (const [chainId, runtime] of state.chains) {
    const age = runtime.headAtMono ? fmtSeconds((performance.now() - runtime.headAtMono) / 1000, 0) : "—";
    rows.push(`  ${chainId}: head ${runtime.headBlock ?? "—"}  finality boundary ${runtime.finalizedBlock ?? "—"}  ${dim(`age ${age}`)}`);
    rows.push(`  ${dim(`rpc ${runtime.rpcCalls} · scans ${runtime.logBatchesScanned} · errors ${runtime.observerErrors} · watched ${runtime.watched}`)}`);
  }
  rows.push(`  api: ${state.apiCalls} calls${state.api429s > 0 ? yellow(` · ${state.api429s} rate-limited`) : ""}${state.apiLastError ? red(` · ${truncate(state.apiLastError, Math.max(10, width - 12))}`) : ""}`);
  rows.push(`  webhooks: ${state.webhookDeliveryOk ? green("endpoint verified") : dim("polling only")}${state.webhooksInvalid > 0 ? red(` · ${state.webhooksInvalid} invalid`) : ""}`);
  if (state.guard.stopReason) rows.push(`  ${red(`stopped: ${truncate(state.guard.stopReason, Math.max(10, width - 14))}`)}`);
  else if (state.guard.paused) rows.push(`  ${yellow("admission paused (p to resume)")}`);
  for (const warning of state.warnings.slice(-3)) {
    rows.push(`  ${yellow(`⚠ ${truncate(warning, Math.max(10, width - 6))}`)}`);
  }
  return rows;
}

function pushTable(state: RunState, rows: string[]): void {
  const ops = [...state.ops.values()];
  const active = ops.filter((op) => op.phase !== "verified" && op.phase !== "failed");
  const done = ops.filter((op) => op.phase === "verified").slice(-3);
  const shown = [...active.slice(0, 12), ...done];
  rows.push(bold(`Lifecycles ${dim(`(${active.length} outstanding${ops.length > shown.length ? `, ${ops.length - shown.length} more` : ""})`)}`));
  if (shown.length === 0) {
    rows.push(dim("  none yet"));
    return;
  }
  rows.push(`  ${dim("#".padEnd(6))} ${dim("chain".padEnd(7))} ${dim("phase".padEnd(11))} ${dim("age".padEnd(8))} ${dim("payment".padEnd(12))} ${dim("payout".padEnd(12))} ${dim("settled")}`);
  for (const op of shown) {
    const age = op.timings.create_start ? (performance.now() - op.timings.create_start.mono) / 1000 : 0;
    const phase = PHASE_FOR[op.phase] ?? op.phase;
    rows.push(
      `  ${dim(op.index.toString().padEnd(6))} ${(op.chainId ?? "—").padEnd(7)} ${phase.padEnd(11)} ${fmtSeconds(age, 0).padEnd(8)} ${shortHash(op.txHash).padEnd(12)} ${shortHash(op.sweepTxHash).padEnd(12)} ${op.timings.settled_api_seen ? green("✓") : dim("·")}`,
    );
  }
}

const PHASE_FOR: Record<string, string> = {
  created: "created", binding: "binding", paying: "paying", included: "detected",
  finalized: "finalized", settled: "sweeping", verified: "verified",
  failed: "failed", timed_out: "timed out",
};

function pushFooter(state: RunState, rows: string[]): void {
  const hint = dim("[q] stop & drain   [p] pause admission");
  const last = state.warnings.length > 0 ? yellow(truncate(state.warnings[state.warnings.length - 1]!, process.stdout.columns || 120)) : green("no warnings");
  rows.push(`${hint}   ${last}`);
}

function padRow(row: string, width: number): string {
  const visible = row.replace(/\x1b\[[0-9;]*m/g, "");
  return row + " ".repeat(Math.max(0, width - visible.length));
}

function shortHash(hash: string | undefined): string {
  return hash ? dim(`${hash.slice(0, 10)}…`) : "—";
}

function truncate(text: string, width: number): string {
  return text.length > width ? `${text.slice(0, width - 1)}…` : text;
}

function fmtClock(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return `${minutes.toString().padStart(2, "0")}:${seconds.toString().padStart(2, "0")}`;
}

/** Exposed for tests: every timing point the table could show. */
export const ALL_TIMING_POINTS = ALL_POINTS;
export type { OpState };
