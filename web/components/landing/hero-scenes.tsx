"use client";

import { ArrowUpRight, Check, Copy, Loader2, Plus, Wallet, X } from "lucide-react";
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * The hero's picture of the product: Payday inside a merchant's own app.
 *
 * One composition, on a loop. The left half is the merchant's backend — an
 * editor whose tabs follow the story: the call that creates a deposit
 * request, the events Payday sends back while the user pays, the webhook
 * handler that credits them. The right half is the merchant's app as their
 * user sees it: a wallet page, an add-funds sheet that Payday powers, a
 * balance that goes up. Nothing here is Payday's own UI; the point is that
 * the merchant's app never has to leave itself.
 *
 * Everything is a pure function of one clock: every element reads `t`,
 * milliseconds into the loop, and derives what is typed, which tab is open,
 * where the cursor is. Nothing is scheduled, so a tab that sleeps and wakes
 * picks up mid-story rather than firing a backlog of timers.
 *
 * The stage has a fixed size and scales to the card, so the composition
 * never reflows on a phone.
 */

const STAGE = { width: 640, height: 400 } as const;

const CHAPTERS = [
  { title: "Create the request", from: 0 },
  { title: "Your user pays in-app", from: 9_500 },
  { title: "Credit off the webhook", from: 19_000 },
] as const;

const TOTAL_MS = 27_500;

/** The last beat of the loop, in which the whole stage fades before it starts over. */
const LOOP_FADE_MS = 400;

const PAYER_WALLET = "0x7099…79C8";
const DEPOSIT_ADDRESS = "0x9a3F…A0c2";
const PAYOUT_WALLET = "0x3C44…93BC";
const TX_HASH = "0x8f3a…5b8f";

const BALANCE_BEFORE = 1_240;
const DEPOSIT = 250;

/* ------------------------------------------------------------------------ */
/* Timing                                                                   */
/* ------------------------------------------------------------------------ */

/** Every beat of the story, in milliseconds into the loop. */
const AT = {
  // Chapter 1: the app's user asks to add funds; the backend creates a request.
  toAddFunds: 900,
  addFunds: 1_500,
  codeTyped: 1_700,
  response: 5_000,
  sheetUp: 5_700,
  // Chapter 2: they pay from their wallet without leaving the app.
  eventsTab: 9_500,
  ready: 9_900,
  toPay: 10_600,
  pay: 11_400,
  walletIn: 11_700,
  toConfirm: 12_400,
  confirm: 13_300,
  walletOut: 13_900,
  deposited: 14_200,
  settled: 16_200,
  received: 16_300,
  sheetDown: 17_700,
  // Chapter 3: the webhook lands and the app credits the balance.
  webhookTab: 19_000,
  inbound: 19_600,
  runFrom: 20_000,
  credit: 22_000,
  countTo: 23_000,
  responded: 23_400,
} as const;

/** The first characters of `text` for a typist starting at `start`. */
function typed(t: number, text: string, start: number, perChar: number): string {
  if (t < start) return "";
  return text.slice(0, Math.min(text.length, Math.floor((t - start) / perChar)));
}

/** True for the `hold` milliseconds after `at`. */
function within(t: number, at: number, hold: number): boolean {
  return t >= at && t < at + hold;
}

/** Progress from `from` to `to`, clamped to 0-1. */
function progress(t: number, from: number, to: number): number {
  if (t <= from) return 0;
  if (t >= to) return 1;
  return (t - from) / (to - from);
}

function easeOut(fraction: number): number {
  return 1 - Math.pow(1 - fraction, 3);
}

/* ------------------------------------------------------------------------ */
/* The clock                                                                */
/* ------------------------------------------------------------------------ */

/**
 * Milliseconds into the loop, advancing with the frame rate and wrapping at
 * the end. A frame after a long pause counts as a short one, so a background
 * tab does not skip a chapter when it comes back.
 */
function useLoopClock(total: number, frozenAt: number | null): number {
  const [t, setT] = useState(0);

  useEffect(() => {
    if (frozenAt !== null) return;
    let frame = 0;
    let last = performance.now();
    let elapsed = 0;
    let shown = 0;
    const tick = (now: number) => {
      const dt = Math.min(now - last, 100);
      last = now;
      elapsed = (elapsed + dt) % total;
      // Thirty frames a second is plenty for typing and a moving cursor.
      if (Math.abs(elapsed - shown) >= 33 || elapsed < shown) {
        shown = elapsed;
        setT(elapsed);
      }
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [total, frozenAt]);

  return frozenAt ?? t;
}

/**
 * `/?scene=<ms>` holds the loop at that moment, for looking at one frame of
 * it: a design review, a screenshot. Read once, on the client; the server
 * and the first paint see a running loop either way.
 */
function usePinnedMoment(): number | null {
  const [pinned, setPinned] = useState<number | null>(null);
  useEffect(() => {
    const raw = new URLSearchParams(window.location.search).get("scene");
    const at = raw === null ? Number.NaN : Number(raw);
    void Promise.resolve().then(() => setPinned(Number.isFinite(at) ? at % TOTAL_MS : null));
  }, []);
  return pinned;
}

function useReducedMotion(): boolean {
  const [reduced, setReduced] = useState(false);
  useEffect(() => {
    const query = window.matchMedia("(prefers-reduced-motion: reduce)");
    const update = () => setReduced(query.matches);
    update();
    query.addEventListener("change", update);
    return () => query.removeEventListener("change", update);
  }, []);
  return reduced;
}

/* ------------------------------------------------------------------------ */
/* The stage                                                                */
/* ------------------------------------------------------------------------ */

export function HeroScenes() {
  const reduced = useReducedMotion();
  const pinned = usePinnedMoment();
  // Without motion, hold the moment the request exists on both sides: the
  // code that made it and the sheet it opened.
  const frozenAt = pinned ?? (reduced ? 8_000 : null);
  const t = useLoopClock(TOTAL_MS, frozenAt);

  const chapter = CHAPTERS.reduce(
    (current, entry, index) => (t >= entry.from ? index : current),
    0,
  );
  const fading = frozenAt === null && t >= TOTAL_MS - LOOP_FADE_MS;

  const [scale, setScale] = useState(1);
  const frameRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    const frame = frameRef.current;
    if (!frame) return;
    // Measured before the first paint, so the stage never shows unscaled;
    // the observer then follows the card as the viewport changes.
    const fit = (width: number) => setScale(width / STAGE.width);
    fit(frame.clientWidth);
    const observer = new ResizeObserver(([entry]) => {
      if (entry) fit(entry.contentRect.width);
    });
    observer.observe(frame);
    return () => observer.disconnect();
  }, []);

  return (
    <div
      ref={frameRef}
      role="img"
      aria-label="Payday inside a merchant's app: the backend creates a deposit request, the user pays from a wallet without leaving the app, and a webhook credits their balance."
      className="relative w-full overflow-hidden rounded-[18px] bg-[#0f0f0e] text-[#f6f2ea]"
      style={{ aspectRatio: `${STAGE.width} / ${STAGE.height}` }}
    >
      <div
        className={cn(
          "absolute top-0 left-0 origin-top-left",
          fading ? "landing-stage-fade-out" : "landing-stage-fade-in",
        )}
        style={{ width: STAGE.width, height: STAGE.height, transform: `scale(${scale})` }}
      >
        {/* A quiet green glow behind the app: the product's "go" colour, faded almost to nothing. */}
        <div
          aria-hidden="true"
          className="pointer-events-none absolute top-[-140px] right-[-120px] size-[420px] rounded-full bg-brand-green/[0.09] blur-[90px]"
        />

        <Backend t={t} chapter={chapter} />
        <MerchantApp t={t} />
        <Cursor target={cursorTarget(t)} pressed={cursorPressed(t)} />
        <Chapters t={t} chapter={chapter} />
      </div>
    </div>
  );
}

/** Where the pointer is heading, if anywhere. */
function cursorTarget(t: number): string | null {
  if (within(t, AT.toAddFunds, AT.addFunds + 500 - AT.toAddFunds)) return "add-funds";
  if (within(t, AT.toPay, AT.walletIn + 400 - AT.toPay)) return "pay";
  if (within(t, AT.toConfirm, AT.walletOut - AT.toConfirm)) return "confirm";
  return null;
}

function cursorPressed(t: number): boolean {
  return (
    within(t, AT.addFunds - 60, 160) ||
    within(t, AT.pay - 60, 160) ||
    within(t, AT.confirm - 60, 160)
  );
}

/** The three chapters along the foot of the stage, with the current one filling. */
function Chapters({ t, chapter }: { t: number; chapter: number }) {
  return (
    <ol className="absolute inset-x-0 bottom-0 grid grid-cols-3 gap-5 px-6 pb-4">
      {CHAPTERS.map((entry, index) => {
        const next = CHAPTERS[index + 1]?.from ?? TOTAL_MS;
        const fraction =
          index < chapter ? 1 : index === chapter ? progress(t, entry.from, next) : 0;
        return (
          <li key={entry.title} className="min-w-0">
            <span className="block h-[2px] w-full overflow-hidden rounded-full bg-white/10">
              <span
                className="block h-full origin-left rounded-full bg-brand-green"
                style={{ transform: `scaleX(${fraction})` }}
              />
            </span>
            <span
              className={cn(
                "mt-2 block truncate text-[10.5px] font-medium transition-colors duration-300",
                index === chapter ? "text-[#f6f2ea]" : "text-[#8b8780]",
              )}
            >
              <span className="tabular mr-1.5 text-[#8b8780]">0{index + 1}</span>
              {entry.title}
            </span>
          </li>
        );
      })}
    </ol>
  );
}

/* ------------------------------------------------------------------------ */
/* The cursor                                                               */
/* ------------------------------------------------------------------------ */

/**
 * A pointer that glides to whatever element carries `data-cursor={target}`
 * and presses when told. The anchor is always in the tree, so the stage is
 * its parent from the first layout effect on; it measures the target in the
 * stage's own space, so the layout is free to be whatever it is.
 */
function Cursor({ target, pressed }: { target: string | null; pressed: boolean }) {
  const anchorRef = useRef<HTMLSpanElement>(null);
  const [at, setAt] = useState<{ x: number; y: number } | null>(null);

  useLayoutEffect(() => {
    const stage = anchorRef.current?.parentElement;
    if (!stage || !target) return;
    const element = stage.querySelector<HTMLElement>(`[data-cursor="${target}"]`);
    if (!element) return;
    // Both rectangles are scaled the same way, so their ratio is unscaled.
    const box = stage.getBoundingClientRect();
    const rect = element.getBoundingClientRect();
    const scale = box.width / STAGE.width;
    setAt({
      x: (rect.left - box.left + rect.width * 0.6) / scale,
      y: (rect.top - box.top + rect.height * 0.6) / scale,
    });
  }, [target]);

  return (
    <>
      <span ref={anchorRef} hidden />
      {at ? (
        <svg
          aria-hidden="true"
          viewBox="0 0 24 24"
          className={cn(
            "landing-cursor pointer-events-none absolute top-0 left-0 z-30 size-5 drop-shadow-[0_2px_4px_rgb(0_0_0/0.5)]",
            target ? "opacity-100" : "opacity-0",
          )}
          style={{ transform: `translate(${at.x}px, ${at.y}px) scale(${pressed ? 0.8 : 1})` }}
        >
          <path
            d="M5.5 3.2 19 12.1l-6.1 1.3 3.3 6.3-2.6 1.3-3.3-6.3L5.5 19.3Z"
            fill="#f6f2ea"
            stroke="#0f0f0e"
            strokeWidth="1.4"
            strokeLinejoin="round"
          />
        </svg>
      ) : null}
    </>
  );
}

/* ------------------------------------------------------------------------ */
/* The backend                                                              */
/* ------------------------------------------------------------------------ */

const TABS = ["server.ts", "events", "webhook.ts"] as const;

const CREATE_COMMENT = "// POST /wallet/add-funds";

const CREATE_CODE = `const deposit = await payday.depositRequests.create({
  amount: "250.00",
  payer: { name: user.displayName },
  payer_policy: {
    mode: "merchant_session",
    payer_reference: user.id,
  },
  expires_in: 3600,
});`;

const WEBHOOK_CODE = `app.post("/payday/webhook", async (req, res) => {
  const event = verify(req, WEBHOOK_SECRET);

  if (event.type === "deposit_request.settled") {
    const { payer_reference, amount } =
      event.data.deposit_request;
    await ledger.credit(payer_reference, amount);
  }

  res.status(200).end();
});`;

/** The line that credits the user, lit once it has run. */
const CREDIT_LINE = 6;

/** Which line of the handler is running, as the event moves through it. */
function runningLine(t: number): number | null {
  if (t < AT.runFrom) return null;
  if (t < AT.runFrom + 500) return 1;
  if (t < AT.runFrom + 1_000) return 3;
  if (t < AT.runFrom + 1_500) return 4;
  if (t < AT.credit + 1_000) return CREDIT_LINE;
  if (t < AT.responded) return 9;
  return null;
}

const EVENTS = [
  {
    at: AT.ready,
    time: "12:04:07",
    type: "deposit_request.ready",
    note: `wallet ${PAYER_WALLET} bound · Monad`,
  },
  {
    at: AT.deposited,
    time: "12:04:31",
    type: "deposit_request.deposited",
    note: `250.00 USDC seen · ${TX_HASH}`,
  },
  {
    at: AT.settled,
    time: "12:04:33",
    type: "deposit_request.settled",
    note: `250.00 USDC → ${PAYOUT_WALLET}`,
  },
] as const;

function Backend({ t, chapter }: { t: number; chapter: number }) {
  const tab = chapter;

  return (
    <div className="absolute top-6 bottom-[58px] left-6 flex w-[350px] flex-col overflow-hidden rounded-[12px] border border-white/[0.08] bg-[#161614]">
      <div className="flex items-center gap-1 border-b border-white/[0.08] px-2 pt-2">
        {TABS.map((entry, index) => (
          <span
            key={entry}
            className={cn(
              "rounded-t-[6px] px-2.5 pt-1.5 pb-2 font-mono text-[10.5px] whitespace-nowrap transition-colors duration-300",
              index === tab
                ? "-mb-px border border-b-0 border-white/[0.08] bg-[#0f0f0e] text-[#f6f2ea]"
                : "text-[#8b8780]",
            )}
          >
            {entry}
            {index === 1 && chapter === 1 && t >= AT.ready ? (
              <span className="ml-1.5 inline-block size-1.5 rounded-full bg-brand-green align-middle" />
            ) : null}
          </span>
        ))}
        <span className="ml-auto pr-1.5 pb-1 font-mono text-[10px] whitespace-nowrap text-[#8b8780]">
          your backend
        </span>
      </div>

      <div key={tab} className="landing-scene-step relative min-h-0 flex-1 bg-[#0f0f0e]">
        {tab === 0 ? (
          <CreateCall t={t} />
        ) : tab === 1 ? (
          <EventStream t={t} />
        ) : (
          <WebhookHandler t={t} />
        )}
      </div>
    </div>
  );
}

function CreateCall({ t }: { t: number }) {
  const code = typed(t, CREATE_CODE, AT.codeTyped, 13);
  const typing = within(t, AT.codeTyped, CREATE_CODE.length * 13 + 400);
  const responded = t >= AT.response;

  return (
    <div className="flex h-full flex-col">
      <pre className="min-h-0 flex-1 overflow-hidden px-3 py-3 font-mono text-[10px] leading-[1.65] tracking-[-0.02em] text-[#d8d2c6]">
        <span className="block text-[#7a766f]">{CREATE_COMMENT}</span>
        {code ? <Highlighted code={code} /> : null}
        {typing || !code ? <Caret /> : null}
      </pre>
      {responded ? (
        <div className="landing-scene-line border-t border-white/[0.08] px-3 py-2.5 font-mono text-[10.5px] leading-[1.7]">
          <p className="flex items-center gap-2 text-brand-green">
            <span className="size-1.5 rounded-full bg-brand-green" />
            201 Created
          </p>
          <dl className="mt-1 grid grid-cols-[92px_1fr] text-[#d8d2c6]">
            <dt className="text-[#8b8780]">id</dt>
            <dd>dr_0198f80c…f700</dd>
            <dt className="text-[#8b8780]">client_secret</dt>
            <dd>cs_v6Kq…9dQ</dd>
            <dt className="text-[#8b8780]">status</dt>
            <dd>awaiting_deposit</dd>
          </dl>
        </div>
      ) : null}
    </div>
  );
}

function EventStream({ t }: { t: number }) {
  const shown = EVENTS.filter((event) => t >= event.at);
  return (
    <div className="flex h-full flex-col px-3 py-3 font-mono text-[10.5px] leading-[1.6]">
      <p className="text-[#8b8780]">
        <span className="text-[#d8d2c6]">payday</span> events --follow
        <span className="ml-1.5 text-brand-green">dr_0198f80c…f700</span>
      </p>
      <ol className="mt-2 grid gap-2">
        {shown.map((event, index) => (
          <li key={event.type} className="landing-scene-line grid grid-cols-[62px_1fr] gap-x-3">
            <span className="tabular text-[#8b8780]">{event.time}</span>
            <span className="min-w-0">
              <span
                className={cn(
                  "block truncate",
                  index === shown.length - 1 && event.type.endsWith("settled")
                    ? "text-brand-green"
                    : "text-[#f6f2ea]",
                )}
              >
                {event.type}
              </span>
              <span className="block truncate text-[#8b8780]">{event.note}</span>
            </span>
          </li>
        ))}
      </ol>
      {shown.length < EVENTS.length ? (
        <p className="mt-3 flex items-center gap-2 text-[#8b8780]">
          <span className="size-1.5 animate-pulse rounded-full bg-brand-yellow" />
          {shown.length === 0
            ? "waiting for the user"
            : shown.length === 1
              ? "waiting for the transfer"
              : "awaiting finality"}
        </p>
      ) : null}
    </div>
  );
}

function WebhookHandler({ t }: { t: number }) {
  const running = runningLine(t);
  const inbound = t >= AT.inbound;
  const responded = t >= AT.responded;
  const lines = WEBHOOK_CODE.split("\n");

  return (
    <div className="flex h-full flex-col">
      <pre className="relative min-h-0 flex-1 overflow-hidden px-3 py-3 font-mono text-[10px] leading-[1.65] tracking-[-0.02em] text-[#d8d2c6]">
        {lines.map((line, index) => (
          <span
            key={index}
            className={cn(
              "-mx-3 block px-3 transition-colors duration-200",
              running === index && "bg-brand-green/[0.12]",
              index === CREDIT_LINE && t >= AT.credit && "text-brand-green",
            )}
          >
            <Highlighted code={line} />
            {line === "" ? " " : null}
          </span>
        ))}
      </pre>
      <div className="flex items-center gap-2 border-t border-white/[0.08] px-3 py-2.5 font-mono text-[10.5px]">
        {inbound ? (
          <span className="landing-scene-line flex min-w-0 items-center gap-2 text-[#d8d2c6]">
            <span className="rounded-[4px] bg-brand-yellow px-1.5 py-px text-[10px] font-semibold text-brand-black">
              POST
            </span>
            <span className="truncate">
              /payday/webhook <span className="text-[#8b8780]">·</span>{" "}
              <span className="text-brand-yellow">deposit_request.settled</span>
            </span>
          </span>
        ) : (
          <span className="text-[#8b8780]">listening on :3000</span>
        )}
        {responded ? (
          <span className="landing-scene-line ml-auto flex shrink-0 items-center gap-1.5 text-brand-green">
            <span className="size-1.5 rounded-full bg-brand-green" />
            200
          </span>
        ) : null}
      </div>
    </div>
  );
}

/** Enough of a highlighter for a dozen lines: strings, keywords, calls, punctuation. */
function Highlighted({ code }: { code: string }) {
  const pattern =
    /("(?:[^"\\]|\\.)*"?)|\b(const|await|async|if|return)\b|([A-Za-z_$][\w$]*)(?=\()|([{}()[\],;.]|=>|===|=)|([A-Za-z_$][\w$]*)|(\d+(?:\.\d+)?)|(\s+)|(.)/g;
  const parts: ReactNode[] = [];
  for (const match of code.matchAll(pattern)) {
    const [text, string, keyword, call, punctuation, word] = match;
    const key = parts.length;
    if (string) {
      parts.push(
        <span key={key} className="text-brand-yellow">
          {text}
        </span>,
      );
    } else if (keyword) {
      parts.push(
        <span key={key} className="text-brand-green">
          {text}
        </span>,
      );
    } else if (call) {
      parts.push(
        <span key={key} className="text-[#f6f2ea]">
          {text}
        </span>,
      );
    } else if (punctuation) {
      parts.push(
        <span key={key} className="text-[#7a766f]">
          {text}
        </span>,
      );
    } else if (word) {
      parts.push(<span key={key}>{text}</span>);
    } else {
      parts.push(text);
    }
  }
  return <>{parts}</>;
}

function Caret() {
  return (
    <span
      aria-hidden="true"
      className="landing-caret ml-px inline-block h-[1.15em] w-[6px] bg-[#f6f2ea]/80 align-text-bottom"
    />
  );
}

/* ------------------------------------------------------------------------ */
/* The merchant's app                                                       */
/* ------------------------------------------------------------------------ */

function MerchantApp({ t }: { t: number }) {
  const sheetOpen = t >= AT.sheetUp && t < AT.sheetDown + 350;
  const sheetLeaving = t >= AT.sheetDown;
  const credited = t >= AT.credit;
  const balance = BALANCE_BEFORE + DEPOSIT * easeOut(progress(t, AT.credit, AT.countTo));
  const toast = within(t, AT.credit + 300, 3_600);

  return (
    <div className="absolute top-6 right-6 bottom-[58px] w-[230px] overflow-hidden rounded-[14px] bg-[#f6f2ea] text-brand-black shadow-[0_24px_60px_-24px_rgb(0_0_0/0.8)]">
      <div className="flex items-center gap-2 border-b border-brand-black/[0.08] px-4 py-2.5">
        <span className="flex size-5 items-center justify-center rounded-[6px] bg-brand-black text-[10px] font-bold text-[#f6f2ea]">
          A
        </span>
        <span className="text-[12px] font-semibold tracking-tight">Acme</span>
        <span className="ml-auto flex size-6 items-center justify-center rounded-full bg-brand-yellow text-[10px] font-semibold">
          JD
        </span>
      </div>

      <div className="px-4 pt-3.5">
        <p className="text-[10px] font-medium tracking-[0.14em] text-brand-subtle uppercase">
          Wallet
        </p>
        <p className="tabular mt-1 flex items-baseline gap-1.5 text-[26px] leading-none font-semibold tracking-tight">
          {balance.toLocaleString("en-US", {
            minimumFractionDigits: 2,
            maximumFractionDigits: 2,
          })}
          <span className="text-[12px] font-medium text-brand-subtle">USDC</span>
        </p>
        <div className="mt-3.5 grid grid-cols-2 gap-2">
          <span
            data-cursor="add-funds"
            className={cn(
              "flex h-8 items-center justify-center gap-1.5 rounded-[8px] bg-brand-black text-[12px] font-medium text-[#f6f2ea] transition-transform",
              within(t, AT.addFunds - 60, 160) && "scale-[0.97]",
            )}
          >
            <Plus className="size-3.5" />
            Add funds
          </span>
          <span className="flex h-8 items-center justify-center gap-1.5 rounded-[8px] border border-brand-black/20 text-[12px] font-medium">
            <ArrowUpRight className="size-3.5" />
            Withdraw
          </span>
        </div>

        <p className="mt-4 text-[10px] font-medium tracking-[0.14em] text-brand-subtle uppercase">
          Activity
        </p>
        <ul className="mt-1.5 grid text-[12px]">
          {credited ? (
            <li className="landing-scene-line -mx-2 flex items-center gap-2.5 rounded-[8px] bg-brand-green/[0.16] px-2 py-2">
              <span className="flex size-6 items-center justify-center rounded-full bg-brand-green text-brand-black">
                <Check className="size-3" />
              </span>
              <span className="min-w-0 flex-1">
                <span className="block truncate font-medium">Deposit</span>
                <span className="block text-[10.5px] text-brand-subtle">Just now · settled</span>
              </span>
              <span className="tabular font-medium">+250.00</span>
            </li>
          ) : null}
          <Activity label="Pro plan" when="Yesterday" amount="−49.00" />
          <Activity label="Deposit" when="Mon" amount="+500.00" />
          <Activity label="Payout to Maya" when="Aug 28" amount="−120.00" />
        </ul>
      </div>

      {sheetOpen ? <DepositSheet t={t} leaving={sheetLeaving} /> : null}
      {toast ? (
        <div className="landing-toast absolute inset-x-3 bottom-3 flex items-center gap-2 rounded-[10px] bg-brand-black px-3 py-2 text-[11.5px] font-medium text-[#f6f2ea] shadow-[0_12px_30px_-12px_rgb(0_0_0/0.6)]">
          <span className="flex size-4 items-center justify-center rounded-full bg-brand-green text-brand-black">
            <Check className="size-2.5" />
          </span>
          +250.00 USDC added to balance
        </div>
      ) : null}
    </div>
  );
}

function Activity({ label, when, amount }: { label: string; when: string; amount: string }) {
  return (
    <li className="flex items-center gap-2.5 py-2 text-brand-black/80">
      <span className="size-6 rounded-full bg-brand-black/[0.07]" />
      <span className="min-w-0 flex-1">
        <span className="block truncate">{label}</span>
        <span className="block text-[10.5px] text-brand-subtle">{when}</span>
      </span>
      <span className="tabular">{amount}</span>
    </li>
  );
}

/**
 * The add-funds sheet the app opens over its own page, powered by Payday:
 * the request the backend just created, paid from the user's own wallet.
 */
function DepositSheet({ t, leaving }: { t: number; leaving: boolean }) {
  const state =
    t < AT.pay ? "ready" : t < AT.confirm ? "signing" : t < AT.received ? "confirming" : "received";
  const walletOpen = within(t, AT.walletIn, AT.walletOut - AT.walletIn);
  const walletLeaving = t >= AT.walletOut - 220;
  const confirmed = t >= AT.confirm;
  const seconds = 3_598 - Math.floor((t - AT.sheetUp) / 1000);

  return (
    <>
      <div
        className={cn(
          "absolute inset-0 bg-brand-black/30",
          leaving ? "landing-scrim-out" : "landing-scrim-in",
        )}
      />
      <div
        className={cn(
          "absolute inset-x-0 bottom-0 rounded-t-[14px] bg-white px-4 pt-3.5 pb-4 shadow-[0_-12px_40px_-16px_rgb(0_0_0/0.35)]",
          leaving ? "landing-sheet-down" : "landing-sheet-up",
        )}
      >
        <div className="flex items-center justify-between">
          <p className="text-[13px] font-semibold tracking-tight">Add funds</p>
          <X className="size-3.5 text-brand-subtle" />
        </div>

        {state === "received" ? (
          <div className="landing-scene-line py-5 text-center">
            <span className="mx-auto flex size-9 items-center justify-center rounded-full bg-brand-green text-brand-black">
              <Check className="size-4" />
            </span>
            <p className="mt-3 text-[13px] font-semibold tracking-tight">Deposit received</p>
            <p className="mt-1 text-[11px] text-brand-subtle">250.00 USDC · settled on Monad</p>
          </div>
        ) : (
          <>
            <p className="tabular mt-3 flex items-baseline gap-1.5 text-[24px] leading-none font-semibold tracking-tight">
              250.00
              <span className="text-[12px] font-medium text-brand-subtle">USDC</span>
            </p>
            <dl className="mt-3 grid gap-1.5 text-[11px]">
              <div className="flex items-center justify-between gap-3">
                <dt className="text-brand-subtle">Network</dt>
                <dd className="font-medium">Monad</dd>
              </div>
              <div className="flex items-center justify-between gap-3">
                <dt className="text-brand-subtle">One-time address</dt>
                <dd className="flex items-center gap-1.5 font-mono">
                  {DEPOSIT_ADDRESS}
                  <Copy className="size-3 text-brand-subtle" />
                </dd>
              </div>
              <div className="flex items-center justify-between gap-3">
                <dt className="text-brand-subtle">Expires in</dt>
                <dd className="tabular font-medium">
                  {Math.floor(seconds / 60)}:{String(seconds % 60).padStart(2, "0")}
                </dd>
              </div>
            </dl>
            <span
              data-cursor="pay"
              className={cn(
                "mt-3.5 flex h-9 items-center justify-center gap-2 rounded-[8px] text-[12px] font-medium transition-[transform,background-color]",
                state === "ready"
                  ? "bg-brand-black text-[#f6f2ea]"
                  : "bg-brand-black/[0.08] text-brand-subtle",
                within(t, AT.pay - 60, 160) && "scale-[0.97]",
              )}
            >
              {state === "ready" ? (
                <Wallet className="size-3.5" />
              ) : (
                <Loader2 className="size-3.5 animate-spin" />
              )}
              {state === "ready"
                ? "Pay from wallet"
                : state === "signing"
                  ? "Confirm in your wallet…"
                  : "Confirming on-chain…"}
            </span>
          </>
        )}

        <p className="mt-3 flex items-center justify-center gap-1.5 text-[10px] text-brand-subtle">
          <span className="size-1.5 rounded-full bg-brand-green" />
          Secured by Payday
        </p>
      </div>

      {walletOpen ? (
        <WalletPrompt
          leaving={walletLeaving}
          confirmed={confirmed}
          pressed={within(t, AT.confirm - 60, 160)}
        />
      ) : null}
    </>
  );
}

/** The user's own wallet asking for the transfer, in a plain wallet's clothes. */
function WalletPrompt({
  leaving,
  confirmed,
  pressed,
}: {
  leaving: boolean;
  confirmed: boolean;
  pressed: boolean;
}) {
  return (
    <div
      className={cn(
        "absolute inset-x-5 top-[72px] rounded-[12px] border border-brand-black/10 bg-white p-3.5 shadow-[0_20px_50px_-16px_rgb(0_0_0/0.45)]",
        leaving ? "landing-sheet-leave" : "landing-sheet-enter",
      )}
    >
      <div className="flex items-center gap-2">
        <span className="flex size-5 items-center justify-center rounded-full bg-brand-black text-[#f6f2ea]">
          <Wallet className="size-2.5" />
        </span>
        <span className="text-[11.5px] font-semibold">Send 250.00 USDC</span>
      </div>
      <dl className="mt-2.5 grid gap-1 text-[10.5px]">
        <div className="flex justify-between gap-3">
          <dt className="text-brand-subtle">From</dt>
          <dd className="font-mono">{PAYER_WALLET}</dd>
        </div>
        <div className="flex justify-between gap-3">
          <dt className="text-brand-subtle">To</dt>
          <dd className="font-mono">{DEPOSIT_ADDRESS}</dd>
        </div>
        <div className="flex justify-between gap-3">
          <dt className="text-brand-subtle">Network fee</dt>
          <dd className="tabular">0.0004 MON</dd>
        </div>
      </dl>
      <div className="mt-3 grid grid-cols-2 gap-2 text-[11.5px] font-medium">
        <span className="flex h-8 items-center justify-center rounded-[8px] border border-brand-black/15">
          Reject
        </span>
        <span
          data-cursor="confirm"
          className={cn(
            "flex h-8 items-center justify-center gap-1.5 rounded-[8px] bg-brand-black text-[#f6f2ea] transition-transform",
            pressed && "scale-[0.97]",
          )}
        >
          {confirmed ? <Check className="size-3.5" /> : null}
          {confirmed ? "Sent" : "Confirm"}
        </span>
      </div>
    </div>
  );
}
