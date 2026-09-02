"use client";

import type { PayerPayment } from "@payday/sdk";
import { useCallback, useEffect, useRef, useState } from "react";
import { backoffMs, jitter, pollDelayMs } from "@/lib/poll";
import { payerClient } from "@/lib/payday";

interface PendingSend {
  hash: string;
  /** Credited total at the moment we sent, so we know when ours lands. */
  receivedAtSend: bigint;
  at: number;
}

export interface LivePayment {
  payment: PayerPayment;
  /** Local clock reading when this payment was received, for the countdown. */
  receivedAt: number;
  /** True once a read has failed and we are retrying with backoff. */
  reconnecting: boolean;
  pendingTxHash: string | null;
  /** Record a transfer this browser sent; starts the fast poll window. */
  markSent: (hash: string) => void;
  /** Re-read now, for the moment verification changes what this tab may see. */
  refresh: () => void;
}

/**
 * Keeps the server-rendered payment live.
 *
 * Reads go straight to the Payday API rather than through this app's server:
 * the payer routes are public, keyless and CORS-enabled, so a proxy would add a
 * hop and a second copy of the contract without buying anything.
 *
 * `payerSession` is this tab's opaque session, sent as a header on every read
 * so a gated invoice unlocks here and nowhere else. The server-rendered
 * payment never had it; the first read with a session replaces it.
 */
export function usePayment(initial: PayerPayment, payerSession: string | null = null): LivePayment {
  const [state, setState] = useState(() => ({
    payment: initial,
    // Captured on both server and client at first render, so the countdown
    // hydrates to the same value on each.
    receivedAt: Date.now(),
  }));
  const [reconnecting, setReconnecting] = useState(false);
  const [pendingSend, setPendingSend] = useState<PendingSend | null>(null);

  // Scheduling reads the newest values without re-creating the polling loop.
  const latest = useRef({ payment: initial, pendingSend: null as PendingSend | null });
  useEffect(() => {
    latest.current = { payment: state.payment, pendingSend };
  });

  const poke = useRef<() => void>(() => {});

  const markSent = useCallback((hash: string) => {
    const send: PendingSend = {
      hash,
      // Locked content has no credited total; nothing can be sent from a locked
      // page, so this only runs with the mechanics present.
      receivedAtSend: BigInt(latest.current.payment.received_base_units ?? "0"),
      at: Date.now(),
    };
    latest.current = { ...latest.current, pendingSend: send };
    setPendingSend(send);
    poke.current();
  }, []);

  const id = initial.id;

  useEffect(() => {
    let disposed = false;
    const session = payerSession;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let inFlight: AbortController | null = null;
    let failures = 0;

    const clear = () => {
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
    };

    const schedule = () => {
      if (disposed) return;
      clear();
      const { payment: current, pendingSend: send } = latest.current;
      const delay =
        failures > 0
          ? backoffMs(failures)
          : pollDelayMs({
              status: current.status,
              receivedBaseUnits: current.received_base_units ?? "0",
              documentHidden: typeof document !== "undefined" && document.hidden,
              msSinceSend: send ? Date.now() - send.at : null,
            });
      if (delay === null) return;
      timer = setTimeout(() => void tick(), jitter(delay, Math.random()));
    };

    const tick = async () => {
      if (disposed) return;
      clear();
      inFlight?.abort();
      const controller = new AbortController();
      inFlight = controller;

      try {
        const next = await payerClient.payments.get(id, {
          signal: controller.signal,
          ...(session === null ? {} : { payerSession: session }),
        });
        if (disposed) return;
        failures = 0;
        setReconnecting(false);
        setState({ payment: next, receivedAt: Date.now() });
        latest.current = { ...latest.current, payment: next };

        // Our transfer has been credited once the gateway's total moves past
        // what it was when we sent, so the "confirming" state can end.
        const send = latest.current.pendingSend;
        if (send && BigInt(next.received_base_units ?? "0") > send.receivedAtSend) {
          latest.current = { ...latest.current, pendingSend: null };
          setPendingSend(null);
        }
      } catch {
        if (disposed || controller.signal.aborted) return;
        // Keep the last known payment on screen; a failed read must never blank
        // out an address the payer may be mid-way through using.
        failures += 1;
        setReconnecting(true);
      } finally {
        if (inFlight === controller) inFlight = null;
      }

      schedule();
    };

    poke.current = () => {
      failures = 0;
      void tick();
    };

    const onVisibility = () => {
      if (document.hidden) {
        clear();
        return;
      }
      // Returning from a wallet app is the moment a payer most wants the truth.
      failures = 0;
      void tick();
    };

    document.addEventListener("visibilitychange", onVisibility);
    // A session that just appeared (restored from this tab, or minted by
    // verification) may unlock content the current payment withholds: read
    // at once rather than after the next poll delay.
    if (session === null) schedule();
    else void tick();

    return () => {
      disposed = true;
      clear();
      inFlight?.abort();
      poke.current = () => {};
      document.removeEventListener("visibilitychange", onVisibility);
    };
  }, [id, payerSession]);

  const refresh = useCallback(() => poke.current(), []);

  return {
    payment: state.payment,
    receivedAt: state.receivedAt,
    reconnecting,
    pendingTxHash: pendingSend?.hash ?? null,
    markSent,
    refresh,
  };
}

/** A local clock that advances once a second, for values derived from "now". */
function useNow(): number {
  const [now, setNow] = useState(() => Date.now());

  useEffect(() => {
    const interval = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(interval);
  }, []);

  return now;
}

/**
 * Seconds until the deadline, measured from the gateway's own clock.
 *
 * `server_timestamp` says what time the gateway thought it was when it answered,
 * and `receivedAt` says when that answer arrived here. Elapsed time is measured
 * between two readings of the local clock, so a device whose clock is wrong by
 * hours still counts down correctly, and every poll re-anchors it.
 */
export function useSecondsRemaining(payment: PayerPayment, receivedAt: number): number {
  const now = useNow();
  const deadline = Math.floor(new Date(payment.expires_at).getTime() / 1000);
  const serverNow = Number(payment.server_timestamp);
  const elapsed = (now - receivedAt) / 1000;

  if (!Number.isFinite(deadline) || !Number.isFinite(serverNow)) return 0;
  return Math.max(0, Math.round(deadline - (serverNow + elapsed)));
}
