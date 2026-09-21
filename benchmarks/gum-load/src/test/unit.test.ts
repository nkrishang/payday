import { test } from "node:test";
import assert from "node:assert/strict";
import { verifySignature } from "../webhooks.js";
import { createHmac } from "node:crypto";
import { distribution, percentile, sparkline, histogram, fmtUnits } from "../metrics.js";
import { decimalToUnits, emptyState, reduce, type RunState } from "../model.js";

test("webhook signature verifies Gum's v1 HMAC scheme and rejects stale or tampered input", () => {
  const secret = "whsec_test";
  const body = JSON.stringify({ id: "evt_1", type: "deposit_request.settled", data: {} });
  const t = Math.floor(Date.now() / 1000);
  const sig = `v1,t=${t},sha256=${createHmac("sha256", secret).update(`v1.${t}.${body}`).digest("hex")}`;
  assert.deepEqual(verifySignature(body, sig, secret, t), { ok: true });
  assert.equal(verifySignature(body + " ", sig, secret, t).ok, false);
  assert.equal(verifySignature(body, sig, "other", t).ok, false);
  const stale = `v1,t=${t - 4000},sha256=${createHmac("sha256", secret).update(`v1.${t - 4000}.${body}`).digest("hex")}`;
  assert.equal(verifySignature(body, stale, secret, t).ok, false);
  assert.equal(verifySignature(body, undefined, secret, t).ok, false);
});

test("percentiles and distributions behave on small and empty samples", () => {
  assert.equal(percentile([], 99), undefined);
  const samples = Array.from({ length: 100 }, (_, i) => i);
  assert.equal(percentile(samples, 50), 49);
  assert.equal(percentile(samples, 95), 94);
  assert.equal(percentile(samples, 100), 99);
  const d = distribution([1, 2, 3, 4, 100]);
  assert.equal(d.count, 5);
  assert.equal(d.max, 100);
  assert.ok(d.mean !== undefined && d.mean > 0);
  assert.deepEqual(distribution([]), { count: 0 });
});

test("sparkline and histogram render something for any input", () => {
  assert.equal(sparkline([], 10).trim(), "");
  const spark = sparkline([1, 2, 3, 4, 5], 5);
  assert.equal(spark.length, 5);
  const bars = histogram([1, 2, 2, 3, 10, 10, 11, 20], 4, 8);
  assert.equal(bars.length, 4);
  assert.ok(bars.every((entry) => typeof entry.count === "number"));
});

test("decimal to base units matches the six-decimal token convention", () => {
  assert.equal(decimalToUnits("0.01"), "10000");
  assert.equal(decimalToUnits("1.5"), "1500000");
  assert.equal(decimalToUnits("10.500000"), "10500000");
  assert.throws(() => decimalToUnits("-1"));
  assert.throws(() => decimalToUnits("abc"));
  assert.equal(fmtUnits("10500000"), "10.5");
});

function stateWithOp(): RunState {
  const state = emptyState({
    runId: "r", mode: "local", apiOrigin: "http://x", experiment: "latency",
    profile: "standard", currency: "USDC", amountUnits: "10000", epoch: 1,
  });
  return state;
}

test("reducer records write-ahead intent, lifecycle stamps, and completion in order", () => {
  const state = stateWithOp();
  reduce(state, { type: "phase.changed", payload: { phase: "measure" } }, 0);
  reduce(state, { type: "op.intended", payload: { op: "op-0", index: 0, warmup: false, body: {}, idempotencyKey: "k" } }, 1);
  reduce(state, { type: "op.created", payload: { op: "op-0", depositId: "dr_1", currency: "USDC", amount: "0.01", networks: [] } }, 2);
  reduce(state, { type: "op.bound", payload: { op: "op-0", chainId: "31337", address: "0xabc", payout: "0xpayout", wallet: "0xwallet" } }, 3);
  assert.equal(state.byDeposit.get("dr_1"), "op-0");
  assert.equal(state.chains.get("31337")?.watched, 1);

  const evidence = { blockNumber: 5, blockHash: "0xb", blockTime: "2026-09-19T00:00:00Z", txHash: "0xt", logIndex: 0 };
  reduce(state, { type: "op.payment.broadcast", payload: { op: "op-0", txHash: "0xt" } }, 4);
  reduce(state, { type: "op.payment.included", payload: { op: "op-0", evidence, observedMono: 5 } }, 5);
  reduce(state, { type: "op.payout.included", payload: { op: "op-0", sweepTxHash: "0xs", evidence: { ...evidence, blockNumber: 9, blockTime: "2026-09-19T00:00:04Z" }, payoutAddress: "0xpayout", valueUnits: "10000" } }, 6);
  reduce(state, { type: "op.settled.api", payload: { op: "op-0" } }, 7);
  reduce(state, { type: "op.verified", payload: { op: "op-0", checks: ["api.status=settled"] } }, 8);

  const op = state.ops.get("op-0")!;
  assert.equal(op.phase, "verified");
  assert.ok(op.timings.payment_inclusion);
  assert.ok(op.timings.settled_api_seen);
  // Completion latency came from block clocks: 4s between block timestamps.
  assert.equal(state.chains.get("31337")?.e2e[0], 4);
  // Principal released on verification.
  assert.equal(state.guard.outstandingPrincipalUnits, "0");
});

test("reducer tolerates unknown ops and duplicate stamps without corrupting state", () => {
  const state = stateWithOp();
  reduce(state, { type: "op.settled.api", payload: { op: "missing" } }, 1);
  reduce(state, { type: "op.intended", payload: { op: "op-0", index: 0, warmup: false, body: {}, idempotencyKey: "k" } }, 2);
  reduce(state, { type: "op.created", payload: { op: "op-0", depositId: "dr_1", currency: "USDC", amount: "0.01", networks: [] } }, 3);
  reduce(state, { type: "op.credited", payload: { op: "op-0", source: "webhook" } }, 4);
  reduce(state, { type: "op.credited", payload: { op: "op-0", source: "api" } }, 5);
  const op = state.ops.get("op-0")!;
  assert.ok(op.timings.deposited_webhook_received);
  assert.ok(op.timings.credited_api_seen);
});
