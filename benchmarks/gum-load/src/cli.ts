#!/usr/bin/env node
/**
 * gum-load — the Gum load-test and benchmark harness CLI.
 *
 *   gum-load preflight --mode local [--fund-local]
 *   gum-load run --mode local --experiment latency --count 10
 *   gum-load run --mode real --target staging --experiment latency --chain 143 --count 30 ...
 *   gum-load report runs/<id>
 *
 * Resume: an interrupted run's journal is replayed by `report`; outstanding
 * funded work is never abandoned — re-run `report` after the chain settles,
 * or start a fresh run with the same payer seed to continue measuring.
 */

import { parseArgs } from "node:util";
import { writeFileSync, readFileSync, existsSync } from "node:fs";
import { join } from "node:path";
import { resolveConfig, parseDuration, type CliArgs } from "./config.js";
import { run } from "./run.js";
import { buildSummary, stateFromJournal } from "./report.js";
import { fmtSeconds } from "./metrics.js";

const USAGE = `gum-load — Gum load-test harness (an external API user; no system internals)

  setup:    npm install && npm run build      (inside benchmarks/gum-load)
  local:    just dev                           (another shell)
            scripts/local-api-key.sh bench@example.test   → export GUM_API_KEY=<key>

  preflight  --mode local [--fund-local]        validate the environment; no money moves
  run        --mode local|real ...              run one experiment
  report     runs/<id>                          regenerate summary.json / report.md / csv

run flags:
  --experiment latency|capacity60|soak   what to measure
  --profile standard|pinned|attested     payer flow            (default standard)
  --chain ID                             31337|31338 local, 143|8453|42161 real
  --currency USDC|USDT                   (default USDC)
  --amount 0.01                          per-deposit amount
  --count N                              lifecycles in the experiment
  --rate R                               open-loop arrivals per second (latency/soak)
  --duration 15m                         soak length
  --warmup N                             unmeasured warm-up lifecycles
  --repeat N                             capacity60 cohort repetitions
  --webhook https://…                    public URL of the harness webhook receiver
  --display auto|tui|plain|json          (default auto: tui on a TTY)
  --fund-local                           local only: fund payer wallets from the Anvil funder
  --no-jitter                            deterministic spacing for latency runs

secrets (environment only): GUM_API_KEY, GUM_RPC_URL_<chain>, GUM_LOAD_PAYER_SEED`;

function parse(): { args: CliArgs; positionals: string[] } {
  const { values, positionals } = parseArgs({
    options: {
      config: { type: "string" },
      mode: { type: "string" },
      target: { type: "string" },
      experiment: { type: "string" },
      profile: { type: "string" },
      chain: { type: "string" },
      currency: { type: "string" },
      amount: { type: "string" },
      count: { type: "string" },
      rate: { type: "string" },
      duration: { type: "string" },
      warmup: { type: "string" },
      repeat: { type: "string" },
      webhook: { type: "string" },
      display: { type: "string" },
      "fund-local": { type: "boolean", default: false },
      jitter: { type: "boolean", default: true },
      help: { type: "boolean", default: false },
    },
    allowPositionals: true,
  });
  if (values.help) {
    console.log(USAGE);
    process.exit(0);
  }
  return {
    args: {
      config: values.config,
      mode: values.mode as CliArgs["mode"],
      target: values.target as CliArgs["target"],
      experiment: values.experiment as CliArgs["experiment"],
      profile: values.profile as CliArgs["profile"],
      chain: values.chain,
      currency: values.currency,
      amount: values.amount,
      count: values.count !== undefined ? Number(values.count) : undefined,
      rate: values.rate !== undefined ? Number(values.rate) : undefined,
      duration: values.duration,
      warmup: values.warmup !== undefined ? Number(values.warmup) : undefined,
      repeat: values.repeat !== undefined ? Number(values.repeat) : undefined,
      webhook: values.webhook,
      display: values.display as CliArgs["display"],
      fundLocal: values["fund-local"] ?? false,
      jitter: values.jitter,
    },
    positionals,
  };
}

async function main(): Promise<number> {
  const { args, positionals } = parse();
  const command = positionals[0];

  if (!command || command === "help") {
    console.log(USAGE);
    return 0;
  }

  if (command === "preflight") {
    const config = resolveConfig(args);
    const result = await run({
      config,
      chainId: args.chain ?? (config.mode === "local" ? "31337" : Object.keys(config.chains)[0]!),
      experiment: "latency", profile: args.profile ?? "standard", count: 0, warmup: 0, repeat: 1,
      display: args.display ?? "plain", fundLocal: args.fundLocal === true, runId: `preflight-${Date.now()}`, jitter: true,
    });
    console.log(`preflight passed — run artifacts in ${result.dir}`);
    return 0;
  }

  if (command === "run") {
    const config = resolveConfig(args);
    if (!args.experiment) throw new Error("--experiment latency|capacity60|soak is required");
    if (args.experiment === "capacity60") {
      if (!args.count) throw new Error("capacity60 requires --count N (the cohort size); it takes no --rate/--duration");
      if (args.rate !== undefined || args.duration !== undefined) throw new Error("capacity60 takes no --rate/--duration");
    }
    if (args.experiment === "soak" && (!args.rate || !args.duration)) {
      throw new Error("soak requires --rate R and --duration 15m");
    }
    if (args.experiment === "latency" && config.mode === "real" && !args.count) {
      throw new Error("real-mode latency requires an explicit --count (the plan's approved limits apply)");
    }
    const display = args.display ?? "auto";
    const result = await run({
      config,
      chainId: args.chain ?? (config.mode === "local" ? "31337" : Object.keys(config.chains)[0]!),
      experiment: args.experiment,
      profile: args.profile ?? "standard",
      // Soak derives its workload from --rate × --duration; only latency uses
      // a count default.
      count: args.experiment === "soak" ? args.count ?? 0 : args.count ?? 10,
      rate: args.rate,
      durationSeconds: args.duration ? parseDuration(args.duration) : undefined,
      warmup: args.warmup ?? 0,
      repeat: args.repeat ?? 1,
      display,
      fundLocal: args.fundLocal === true,
      jitter: args.jitter !== false,
    });
    if (display !== "tui") printResult(result.dir);
    return 0;
  }

  if (command === "report") {
    const dir = positionals[1];
    if (!dir || !existsSync(join(dir, "events.ndjson"))) {
      throw new Error("report requires a run directory containing events.ndjson");
    }
    const state = stateFromJournal(join(dir, "events.ndjson"));
    const summary = buildSummary(state, {
      experiment: state.experiment,
      chain: {
        id: "", name: state.runId, rpcUrl: "", finality: { kind: "finalized", margin: 0 },
        tokens: {}, nativeSymbol: "",
      },
      currency: state.currency || "USDC",
      amount: "",
      mode: state.mode,
    });
    writeFileSync(join(dir, "summary.json"), JSON.stringify(summary, null, 2));
    printResult(dir);
    return 0;
  }

  throw new Error(`unknown command ${command}; try --help`);
}

function printResult(dir: string): void {
  const summaryPath = join(dir, "summary.json");
  console.log(`\nRun artifacts in ${dir}/  (summary.json, report.md, deposits.csv)`);
  if (!existsSync(summaryPath)) return;
  const summary = JSON.parse(readFileSync(summaryPath, "utf8")) as {
    latency?: { samples: number; paymentToPayout: { p50?: number; p95?: number } };
    capacity60?: { cohort: number; settledApi: number; pass: boolean };
  };
  if (summary.latency && summary.latency.samples > 0) {
    console.log(`  settlement latency: n=${summary.latency.samples}  p50 ${fmtSeconds(summary.latency.paymentToPayout.p50)}  p95 ${fmtSeconds(summary.latency.paymentToPayout.p95)}  — report.md has the full picture`);
  }
  if (summary.capacity60) {
    console.log(`  capacity60: ${summary.capacity60.settledApi}/${summary.capacity60.cohort} settled by 60s — ${summary.capacity60.pass ? "pass" : "fail"}`);
  }
}

main()
  .then((code) => process.exit(code))
  .catch((error: unknown) => {
    console.error(`gum-load: ${error instanceof Error ? error.message : String(error)}`);
    process.exit(1);
  });
