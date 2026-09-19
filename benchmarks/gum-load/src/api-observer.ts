/**
 * Budgeted merchant-API observation: paginated list polling filtered to the
 * run's `issuer_id`, recording credited/settled transitions as observed
 * facts. Polling produces intervals, not exact transition times; the bounds
 * are the previous unsuccessful poll and this poll's completion.
 */

import type { Clients } from "./sdk.js";
import type { EventSink, RunState } from "./model.js";
import { decimalToUnits } from "./model.js";

export class ApiObserver {
  private timer?: NodeJS.Timeout;
  private running = false;
  private polls = 0;

  constructor(
    private readonly clients: Clients,
    private readonly issuerId: string,
    private readonly pollMs: number,
    private readonly sink: EventSink,
    private readonly state: RunState,
    private readonly nowMono: () => number,
    private readonly onStatus: (sample: Record<string, unknown>) => void,
  ) {}

  start(): void {
    this.running = true;
    void this.loop();
  }

  stop(): void {
    this.running = false;
    if (this.timer) clearTimeout(this.timer);
  }

  private async loop(): Promise<void> {
    while (this.running) {
      try {
        await this.poll();
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        this.state.apiLastError = message;
      }
      await new Promise((resolve) => (this.timer = setTimeout(resolve, this.pollMs)));
    }
  }

  async poll(): Promise<void> {
    // A small page of the run's own requests amortizes the allowance across
    // every lifecycle instead of one GET per op. Cursors are followed so an
    // op outside the first page is never permanently invisible (a bounded
    // number of pages caps observation traffic on huge soaks).
    const pageLimit = 100;
    // Generous page budget: a big capacity cohort must stay fully visible to
    // observation, or the measured limit would be the harness's, not Gum's.
    const maxPages = 50;
    let cursor: string | null = null;
    let pages = 0;
    do {
      const page = await this.clients.merchant.depositRequests.list({ issuer_id: this.issuerId, limit: pageLimit, ...(cursor ? { starting_after: cursor } : {}) });
      this.polls += 1;
      pages += 1;
      cursor = page.next_cursor ?? null;
      for (const summary of page.deposit_requests ?? []) this.observe(summary);
    } while (cursor && pages < maxPages);
  }

  private observe(summary: { id: string; received?: string; status?: string }): void {
    const op = this.state.byDeposit.get(summary.id);
    if (!op) return;
    const opState = this.state.ops.get(op);
    if (!opState) return;
    const amount = opState.amountUnits ?? "0";
    // The list payload carries `received` as a decimal string. Transitions are
    // emitted once — re-polling settled ops must not flood the journal.
    const receivedUnits = decimalToUnits(summary.received ?? "0");
    if (BigInt(receivedUnits) >= BigInt(amount) && BigInt(amount) > 0 && !opState.timings.credited_api_seen && !opState.timings.deposited_webhook_received) {
      this.sink({ type: "op.credited", payload: { op, source: "api" } });
    }
    if (summary.status === "settled" && !opState.timings.settled_api_seen && !opState.timings.settled_webhook_received) {
      this.sink({ type: "op.settled.api", payload: { op } });
    } else if ((summary.status === "expired" || summary.status === "returned" || summary.status === "needs_attention") && opState.outcome === undefined) {
      this.sink({ type: "op.outcome", payload: { op, outcome: summary.status } });
    }
  }

  /** One full-detail read for post-settlement verification. */
  async detail(depositId: string): Promise<Record<string, unknown> | null> {
    const full = await this.clients.merchant.depositRequests.get(depositId);
    return full as unknown as Record<string, unknown>;
  }
}
