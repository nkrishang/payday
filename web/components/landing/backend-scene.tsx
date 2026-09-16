"use client";

import { useRef, type CSSProperties } from "react";
import { AT, CreateCall, WebhookHandler } from "./hero-scenes";
import { Stage, useInView, useLoopClock, useReducedMotion } from "./stage";

/**
 * The integration, both files of it, side by side: the backend creates a
 * deposit request and gets an id back; later, the webhook lands and the
 * handler credits the user. One clock runs both. The pieces keep the
 * hero stage's timeline, so the create call reads the clock from its
 * start and the handler reads it from the moment its event is sent.
 */

const STAGE = { width: 560, height: 360 } as const;

const TOTAL_MS = 13_000;
/** When the event leaves the first file for the second. */
const SEND_MS = 7_000;
const ARRIVE_MS = 8_000;

export function BackendScene() {
  const reduced = useReducedMotion();
  const frameRef = useRef<HTMLDivElement>(null);
  const inView = useInView(frameRef);
  const t = useLoopClock(TOTAL_MS, reduced ? 11_500 : null, inView);

  const createT = Math.min(t, ARRIVE_MS - 1);
  const handlerT = t < ARRIVE_MS ? 0 : t - ARRIVE_MS + AT.inbound;
  const packet = t >= SEND_MS && t < ARRIVE_MS ? (t - SEND_MS) / (ARRIVE_MS - SEND_MS) : null;

  return (
    <div
      ref={frameRef}
      className="grid items-center gap-6 lg:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)] lg:gap-0"
    >
      <div data-reveal="scale" style={{ "--i": 2 } as CSSProperties}>
        <Stage
          width={STAGE.width}
          height={STAGE.height}
          label="server.ts: the backend creates a deposit request and gets its id back."
          className="rounded-[18px] bg-gum-black text-gum-white"
        >
          <div className="h-full p-4">
            <CreateCall t={createT} />
          </div>
        </Stage>
      </div>

      <span
        aria-hidden="true"
        className="relative mx-3 hidden h-[2px] w-14 bg-gum-grey/30 lg:block"
      >
        {packet !== null ? (
          <span
            className="absolute top-1/2 left-0 size-2.5 -translate-y-1/2 rounded-full bg-gum-pink"
            style={{ transform: `translate(${packet * 46}px, -50%)` }}
          />
        ) : null}
      </span>

      <div data-reveal="scale" style={{ "--i": 3 } as CSSProperties}>
        <Stage
          width={STAGE.width}
          height={STAGE.height}
          label="webhook.ts: the settled event lands and the handler credits the user."
          className="rounded-[18px] bg-gum-black text-gum-white"
        >
          <div className="h-full p-4">
            <WebhookHandler t={handlerT} />
          </div>
        </Stage>
      </div>
    </div>
  );
}
