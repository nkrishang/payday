import { backoffMs } from "./poll";

export interface RelayBroadcast {
  intentId: string;
  originChainId: string;
  hash: string;
  at: number;
}

const prefix = "gum:relay-send:";

export function recordRelayBroadcast(requestId: string, broadcast: RelayBroadcast): void {
  try {
    sessionStorage.setItem(`${prefix}${requestId}`, JSON.stringify(broadcast));
  } catch {
    // The report is still attempted immediately when storage is unavailable.
  }
}

export function pendingRelayReport(requestId: string): RelayBroadcast | null {
  try {
    const value = sessionStorage.getItem(`${prefix}${requestId}`);
    if (!value) return null;
    const parsed = JSON.parse(value) as Partial<RelayBroadcast>;
    return typeof parsed.intentId === "string" && typeof parsed.originChainId === "string" &&
      typeof parsed.hash === "string" && typeof parsed.at === "number" ? parsed as RelayBroadcast : null;
  } catch {
    return null;
  }
}

export function clearRelayReport(requestId: string): void {
  try {
    sessionStorage.removeItem(`${prefix}${requestId}`);
  } catch {
    // Storage may be unavailable.
  }
}

/** Stateful retry policy kept separate from React so reload recovery is easy to test. */
export class RelaySendRetrier {
  private failures = 0;
  private inFlight = false;

  constructor(private readonly send: (entry: RelayBroadcast) => Promise<unknown>) {}

  get delay(): number { return backoffMs(this.failures); }

  async attempt(requestId: string): Promise<boolean> {
    const entry = pendingRelayReport(requestId);
    if (!entry || this.inFlight) return false;
    this.inFlight = true;
    try {
      await this.send(entry);
      clearRelayReport(requestId);
      this.failures = 0;
      return true;
    } catch {
      this.failures += 1;
      return false;
    } finally {
      this.inFlight = false;
    }
  }
}
