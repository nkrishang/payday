"use client";

import { ArrowUpRight, Check, Copy, Loader2, Plus, Wallet, X } from "lucide-react";
import Image from "next/image";
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * The hero's picture of the product: Payday inside a merchant's own app.
 *
 * One stage, three panels on a track, one in view at a time and each filling
 * the stage, so every part of the story is read at size. The merchant's app
 * sits in the middle. To its right is the backend call that creates a
 * deposit request; to its left, the webhook handler that credits the user.
 * The story slides from the app to the call and back, then from the app to
 * the handler and back, and the app is the one thing that persists: its
 * user taps Add funds, pays from a wallet in a dialog Payday powers, and
 * watches their balance go up.
 *
 * Everything is a pure function of one clock: every element reads `t`,
 * milliseconds into the loop, and derives what is typed, which panel is in
 * view, where the cursor is. Nothing is scheduled, so a tab that sleeps and
 * wakes picks up mid-story rather than firing a backlog of timers.
 *
 * The stage has a fixed size and scales to the card, so the composition
 * never reflows on a phone.
 */

const STAGE = { width: 640, height: 400 } as const;

/** The three panels, left to right, and which one the track shows at rest. */
const PANELS = ["handler", "app", "call"] as const;
type Panel = (typeof PANELS)[number];

const TOTAL_MS = 27_000;

/** The last beat of the loop, in which the whole stage fades before it starts over. */
const LOOP_FADE_MS = 400;

const DEPOSIT_ADDRESS = "0x9a3F…A0c2";

const BALANCE_BEFORE = 1_240;
const DEPOSIT = 250;

/* ------------------------------------------------------------------------ */
/* Timing                                                                   */
/* ------------------------------------------------------------------------ */

/** Every beat of the story, in milliseconds into the loop. */
const AT = {
  // Chapter 1: the user asks to add funds; the backend creates a request.
  toAddFunds: 500,
  addFunds: 1_300,
  toCall: 1_800,
  codeTyped: 2_500,
  response: 6_300,
  backToApp: 7_400,
  // Chapter 2: they pay from their wallet without leaving the app.
  dialogUp: 7_900,
  toPay: 9_000,
  pay: 9_800,
  /** The user signs in their wallet; the transfer is sent. */
  confirm: 11_200,
  received: 13_600,
  dialogDown: 15_400,
  // Chapter 3: the webhook lands and the app credits the balance.
  toHandler: 16_400,
  inbound: 17_000,
  runFrom: 17_400,
  credit: 19_400,
  responded: 20_000,
  backToWallet: 20_800,
  countFrom: 21_400,
  countTo: 22_400,
} as const;

const CHAPTERS = [
  { title: "Create the request", from: 0 },
  { title: "Your user pays in-app", from: AT.dialogUp - 400 },
  { title: "Credit off the webhook", from: AT.toHandler },
] as const;

/** Which panel the track shows at `t`. */
function panelAt(t: number): Panel {
  if (t >= AT.toCall && t < AT.backToApp) return "call";
  if (t >= AT.toHandler && t < AT.backToWallet) return "handler";
  return "app";
}

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
function useLoopClock(total: number, frozenAt: number | null, running: boolean): number {
  const [t, setT] = useState(0);
  // Where the loop is, kept outside the effect so a pause (the stage
  // scrolling out of view) resumes mid-story rather than from the top.
  const elapsedRef = useRef(0);

  useEffect(() => {
    if (frozenAt !== null || !running) return;
    let frame = 0;
    let last = performance.now();
    let shown = elapsedRef.current;
    const tick = (now: number) => {
      const dt = Math.min(now - last, 100);
      last = now;
      const elapsed = (elapsedRef.current + dt) % total;
      elapsedRef.current = elapsed;
      // Thirty frames a second is plenty for typing and a moving cursor.
      if (Math.abs(elapsed - shown) >= 33 || elapsed < shown) {
        shown = elapsed;
        setT(elapsed);
      }
      frame = requestAnimationFrame(tick);
    };
    frame = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(frame);
  }, [total, frozenAt, running]);

  return frozenAt ?? t;
}

/**
 * Whether the stage is on screen. Off screen, the loop stops: a phone
 * scrolling past the hero should not be re-rendering it thirty times a
 * second underneath the scroll.
 */
function useInView(ref: React.RefObject<HTMLElement | null>): boolean {
  const [inView, setInView] = useState(true);
  useEffect(() => {
    const element = ref.current;
    if (!element) return;
    const observer = new IntersectionObserver(
      ([entry]) => setInView(entry?.isIntersecting ?? true),
      { threshold: 0.05 },
    );
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref]);
  return inView;
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
  // Without motion, hold the dialog the backend just opened in the app.
  const frozenAt = pinned ?? (reduced ? 8_600 : null);
  const frameRef = useRef<HTMLDivElement>(null);
  const inView = useInView(frameRef);
  const t = useLoopClock(TOTAL_MS, frozenAt, inView);

  // The stage is laid out at its own width and scaled to the frame. Until
  // the first measurement has been applied it stays invisible, so the
  // first paint on a phone never shows it at full size for a frame before
  // it snaps down; it fades in at the right size instead.
  const [scale, setScale] = useState<number | null>(null);
  useLayoutEffect(() => {
    const frame = frameRef.current;
    if (!frame) return;
    const fit = (width: number) => setScale(width / STAGE.width);
    fit(frame.clientWidth);
    const observer = new ResizeObserver(([entry]) => {
      if (entry) fit(entry.contentRect.width);
    });
    observer.observe(frame);
    return () => observer.disconnect();
  }, []);

  const chapter = CHAPTERS.reduce(
    (current, entry, index) => (t >= entry.from ? index : current),
    0,
  );
  const panel = panelAt(t);
  const fading = frozenAt === null && t >= TOTAL_MS - LOOP_FADE_MS;

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
          scale === null
            ? "invisible"
            : fading
              ? "landing-stage-fade-out"
              : "landing-stage-fade-in",
        )}
        style={{ width: STAGE.width, height: STAGE.height, transform: `scale(${scale ?? 1})` }}
      >
        {/* A quiet green glow behind the stage: the product's "go" colour, faded almost to nothing. */}
        <div
          aria-hidden="true"
          className="pointer-events-none absolute top-[-160px] right-[-140px] size-[460px] rounded-full bg-brand-green/[0.09] blur-[90px]"
        />

        <div
          className="landing-track absolute inset-x-0 top-0 bottom-[52px] flex"
          style={{ transform: `translateX(${-PANELS.indexOf(panel) * STAGE.width}px)` }}
        >
          <Slot>
            <WebhookHandler t={t} />
          </Slot>
          <Slot>
            <MerchantApp t={t} />
          </Slot>
          <Slot>
            <CreateCall t={t} />
          </Slot>
        </div>

        <Cursor target={cursorTarget(t)} pressed={cursorPressed(t)} />
        <Chapters t={t} chapter={chapter} />
      </div>
    </div>
  );
}

/** One stage-width cell of the track, with the panel inset from its edges. */
function Slot({ children }: { children: ReactNode }) {
  return (
    <div className="relative h-full shrink-0 p-5 pb-0" style={{ width: STAGE.width }}>
      {children}
    </div>
  );
}

/** Where the pointer is heading, if anywhere. */
function cursorTarget(t: number): string | null {
  if (within(t, AT.toAddFunds, AT.addFunds + 400 - AT.toAddFunds)) return "add-funds";
  if (within(t, AT.toPay, AT.pay + 500 - AT.toPay)) return "pay";
  return null;
}

function cursorPressed(t: number): boolean {
  return within(t, AT.addFunds - 60, 160) || within(t, AT.pay - 60, 160);
}

/** The three chapters along the foot of the stage, with the current one filling. */
function Chapters({ t, chapter }: { t: number; chapter: number }) {
  return (
    <ol className="absolute inset-x-0 bottom-0 grid grid-cols-3 gap-5 px-5 pb-4">
      {CHAPTERS.map((entry, index) => {
        const next = CHAPTERS[index + 1]?.from ?? TOTAL_MS;
        const fraction =
          index < chapter ? 1 : index === chapter ? progress(t, entry.from, next) : 0;
        return (
          <li key={entry.title} className="min-w-0">
            <span className="block h-[2px] w-full overflow-hidden rounded-full bg-brand-white/10">
              <span
                className="block h-full origin-left rounded-full bg-brand-green"
                style={{ transform: `scaleX(${fraction})` }}
              />
            </span>
            <span
              className={cn(
                "mt-2 block truncate text-[12px] font-medium transition-colors duration-300",
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
            "landing-cursor pointer-events-none absolute top-0 left-0 z-30 size-6 drop-shadow-[0_2px_4px_rgb(0_0_0/0.5)]",
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
  if (t < AT.credit + 400) return CREDIT_LINE;
  if (t < AT.responded) return 9;
  return null;
}

/** An editor panel: a file's name up top, its lines, and a footer for what came back. */
function Editor({
  file,
  footer,
  children,
}: {
  file: string;
  footer: ReactNode;
  children: ReactNode;
}) {
  return (
    <div className="flex h-full flex-col overflow-hidden rounded-[14px] border border-white/[0.08] bg-[#161614]">
      <div className="flex items-center gap-3 border-b border-white/[0.08] px-4 py-2.5 font-mono text-[12.5px]">
        <span className="flex gap-1.5" aria-hidden="true">
          <span className="size-2.5 rounded-full bg-brand-white/15" />
          <span className="size-2.5 rounded-full bg-brand-white/15" />
          <span className="size-2.5 rounded-full bg-brand-white/15" />
        </span>
        <span className="text-[#f6f2ea]">{file}</span>
        <span className="ml-auto text-[#8b8780]">your backend</span>
      </div>
      <pre className="min-h-0 flex-1 overflow-hidden px-5 py-4 font-mono text-[14px] leading-[1.6] tracking-[-0.02em] text-[#d8d2c6]">
        {children}
      </pre>
      <div className="flex min-h-[46px] items-center gap-3 border-t border-white/[0.08] px-5 font-mono text-[13px]">
        {footer}
      </div>
    </div>
  );
}

function CreateCall({ t }: { t: number }) {
  const code = typed(t, CREATE_CODE, AT.codeTyped, 16);
  const typing = within(t, AT.codeTyped, CREATE_CODE.length * 16 + 300);
  const responded = t >= AT.response;

  return (
    <Editor
      file="server.ts"
      footer={
        responded ? (
          <span className="landing-scene-line flex min-w-0 items-center gap-3">
            <span className="flex shrink-0 items-center gap-2 text-brand-green">
              <span className="size-2 rounded-full bg-brand-green" />
              201 Created
            </span>
            <span className="truncate text-[#d8d2c6]">
              dr_0198f80c…f700 <span className="text-[#8b8780]">·</span> awaiting_deposit
            </span>
          </span>
        ) : (
          <span className="text-[#8b8780]">{t >= AT.toCall ? "…" : ""}</span>
        )
      }
    >
      <span className="block text-[#7a766f]">{CREATE_COMMENT}</span>
      {code ? <Highlighted code={code} /> : null}
      {typing || !code ? <Caret /> : null}
    </Editor>
  );
}

function WebhookHandler({ t }: { t: number }) {
  const running = runningLine(t);
  const inbound = t >= AT.inbound;
  const responded = t >= AT.responded;
  const lines = WEBHOOK_CODE.split("\n");

  return (
    <Editor
      file="webhook.ts"
      footer={
        <>
          {inbound ? (
            <span className="landing-scene-line flex min-w-0 items-center gap-2.5 text-[#d8d2c6]">
              <span className="rounded-[5px] bg-brand-yellow px-1.5 py-0.5 text-[11px] font-semibold text-brand-black">
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
            <span className="landing-scene-line ml-auto flex shrink-0 items-center gap-2 text-brand-green">
              <span className="size-2 rounded-full bg-brand-green" />
              200 OK
            </span>
          ) : null}
        </>
      }
    >
      {lines.map((line, index) => (
        <span
          key={index}
          className={cn(
            "-mx-5 block px-5 transition-colors duration-200",
            running === index && "bg-brand-green/[0.14]",
            index === CREDIT_LINE && t >= AT.credit && "text-brand-green",
          )}
        >
          <Highlighted code={line} />
          {line === "" ? " " : null}
        </span>
      ))}
    </Editor>
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
      className="landing-caret ml-px inline-block h-[1.15em] w-[8px] bg-[#f6f2ea]/80 align-text-bottom"
    />
  );
}

/* ------------------------------------------------------------------------ */
/* The merchant's app                                                       */
/* ------------------------------------------------------------------------ */

function MerchantApp({ t }: { t: number }) {
  const dialogOpen = t >= AT.dialogUp && t < AT.dialogDown + 320;
  const dialogLeaving = t >= AT.dialogDown;
  const credited = t >= AT.countFrom;
  const balance = BALANCE_BEFORE + DEPOSIT * easeOut(progress(t, AT.countFrom, AT.countTo));
  const toast = within(t, AT.countFrom + 500, 3_800);

  return (
    <div className="relative h-full overflow-hidden rounded-[14px] bg-brand-white text-brand-black shadow-[0_24px_60px_-24px_rgb(0_0_0/0.8)]">
      <div className="flex items-center gap-2.5 border-b border-brand-black/[0.08] px-5 py-3">
        <span className="flex size-6 items-center justify-center rounded-[7px] bg-brand-black text-[12px] font-bold text-[#f6f2ea]">
          A
        </span>
        <span className="text-[15px] font-semibold tracking-tight">Acme</span>
        <span className="ml-6 text-[13px] text-brand-subtle">Dashboard</span>
        <span className="text-[13px] font-medium">Wallet</span>
        <span className="text-[13px] text-brand-subtle">Settings</span>
        <span className="ml-auto flex size-7 items-center justify-center rounded-full bg-brand-yellow text-[11px] font-semibold">
          JD
        </span>
      </div>

      <div className="grid grid-cols-[minmax(0,1fr)_minmax(0,1.15fr)] gap-8 px-6 pt-6">
        <div>
          <p className="text-[11px] font-medium tracking-[0.14em] text-brand-subtle uppercase">
            Balance
          </p>
          <p className="tabular mt-2 flex items-baseline gap-2 text-[38px] leading-none font-semibold tracking-tight">
            {balance.toLocaleString("en-US", {
              minimumFractionDigits: 2,
              maximumFractionDigits: 2,
            })}
            <Token />
          </p>
          <div className="mt-6 grid gap-2.5">
            <span
              data-cursor="add-funds"
              className={cn(
                "flex h-11 items-center justify-center gap-2 rounded-[10px] bg-brand-black text-[14px] font-medium text-[#f6f2ea] transition-transform",
                within(t, AT.addFunds - 60, 160) && "scale-[0.97]",
              )}
            >
              <Plus className="size-4" />
              Add funds
            </span>
            <span className="flex h-11 items-center justify-center gap-2 rounded-[10px] border border-brand-black/20 text-[14px] font-medium">
              <ArrowUpRight className="size-4" />
              Withdraw
            </span>
          </div>
        </div>

        <div>
          <p className="text-[11px] font-medium tracking-[0.14em] text-brand-subtle uppercase">
            Activity
          </p>
          <ul className="mt-1 grid text-[14px]">
            {credited ? (
              <li className="landing-scene-line -mx-3 flex items-center gap-3 rounded-[10px] bg-brand-green/[0.18] px-3 py-2.5">
                <Usdc className="size-8" />
                <span className="min-w-0 flex-1">
                  <span className="block truncate font-medium">Deposit</span>
                  <span className="flex items-center gap-1.5 text-[12px] whitespace-nowrap text-brand-subtle">
                    Settled on <Monad className="size-3.5" /> Monad
                  </span>
                </span>
                <span className="tabular font-semibold">+250.00</span>
              </li>
            ) : null}
            <Activity label="Pro plan" when="Yesterday" amount="−49.00" />
            <Activity
              label="Deposit"
              when="Monday"
              amount="+500.00"
              icon={<Usdc className="size-8" />}
            />
            {credited ? null : <Activity label="Payout to Maya" when="Aug 28" amount="−120.00" />}
          </ul>
        </div>
      </div>

      {dialogOpen ? <DepositDialog t={t} leaving={dialogLeaving} /> : null}
      {toast ? (
        <div className="landing-toast absolute bottom-4 left-1/2 flex -translate-x-1/2 items-center gap-2.5 rounded-[12px] bg-brand-black px-4 py-2.5 text-[13.5px] font-medium whitespace-nowrap text-[#f6f2ea] shadow-[0_12px_30px_-12px_rgb(0_0_0/0.6)]">
          <Usdc className="size-5" />
          +250.00 USDC added to your balance
        </div>
      ) : null}
    </div>
  );
}

function Activity({
  label,
  when,
  amount,
  icon,
}: {
  label: string;
  when: string;
  amount: string;
  icon?: ReactNode;
}) {
  return (
    <li className="flex items-center gap-3 py-3 text-brand-black/80">
      {icon ?? <span className="size-8 rounded-full bg-brand-black/[0.07]" />}
      <span className="min-w-0 flex-1">
        <span className="block truncate">{label}</span>
        <span className="block text-[12px] text-brand-subtle">{when}</span>
      </span>
      <span className="tabular">{amount}</span>
    </li>
  );
}

/**
 * The add-funds dialog the app opens over its own page, powered by Payday:
 * the request the backend just created, paid from the user's own wallet.
 */
function DepositDialog({ t, leaving }: { t: number; leaving: boolean }) {
  const state =
    t < AT.pay ? "ready" : t < AT.confirm ? "signing" : t < AT.received ? "confirming" : "received";
  const seconds = 3_598 - Math.floor((t - AT.dialogUp) / 1000);

  return (
    <>
      <div
        className={cn(
          "absolute inset-0 bg-brand-black/35",
          leaving ? "landing-scrim-out" : "landing-scrim-in",
        )}
      />
      <div
        className={cn(
          "absolute top-1/2 left-1/2 w-[400px] -translate-x-1/2 -translate-y-1/2 rounded-[16px] bg-brand-white px-6 pt-5 pb-5 shadow-[0_30px_80px_-24px_rgb(0_0_0/0.5)]",
          leaving ? "landing-dialog-out" : "landing-dialog-in",
        )}
      >
        <div className="flex items-center justify-between">
          <p className="text-[16px] font-semibold tracking-tight">Add funds</p>
          <X className="size-4 text-brand-subtle" />
        </div>

        {state === "received" ? (
          <div className="landing-scene-line py-6 text-center">
            <span className="mx-auto flex size-12 items-center justify-center rounded-full bg-brand-green text-brand-black">
              <Check className="size-6" />
            </span>
            <p className="mt-4 text-[18px] font-semibold tracking-tight">Deposit received</p>
            <p className="mt-2 flex items-center justify-center gap-1.5 text-[13.5px] text-brand-subtle">
              <Usdc className="size-4" /> 250.00 USDC · settled on <Monad className="size-4" />{" "}
              Monad
            </p>
          </div>
        ) : (
          <>
            <p className="tabular mt-4 flex items-baseline gap-2 text-[36px] leading-none font-semibold tracking-tight">
              250.00
              <Token />
            </p>
            <dl className="mt-4 grid gap-2 text-[13.5px]">
              <div className="flex items-center justify-between gap-3">
                <dt className="text-brand-subtle">One-time address</dt>
                <dd className="flex items-center gap-2 font-mono">
                  {DEPOSIT_ADDRESS}
                  <Copy className="size-3.5 text-brand-subtle" />
                </dd>
              </div>
              <div className="flex items-center justify-between gap-3">
                <dt className="text-brand-subtle">Network</dt>
                <dd className="flex items-center gap-1.5 font-medium">
                  <Monad className="size-4" />
                  Monad <span className="text-brand-subtle">·</span>{" "}
                  <span className="tabular">
                    expires in {Math.floor(seconds / 60)}:{String(seconds % 60).padStart(2, "0")}
                  </span>
                </dd>
              </div>
            </dl>
            <span
              data-cursor="pay"
              className={cn(
                "mt-5 flex h-12 items-center justify-center gap-2.5 rounded-[10px] text-[14.5px] font-medium transition-[transform,background-color]",
                state === "ready"
                  ? "bg-brand-black text-[#f6f2ea]"
                  : "bg-brand-black/[0.08] text-brand-subtle",
                within(t, AT.pay - 60, 160) && "scale-[0.97]",
              )}
            >
              {state === "ready" ? (
                <Wallet className="size-4" />
              ) : (
                <Loader2 className="size-4 animate-spin" />
              )}
              {state === "ready" ? (
                "Pay from wallet"
              ) : state === "signing" ? (
                "Confirm in your wallet…"
              ) : (
                <>
                  Confirming on <Monad className="size-4" /> Monad…
                </>
              )}
            </span>
          </>
        )}

        <p className="mt-4 flex items-center justify-center gap-2 text-[12px] text-brand-subtle">
          <span className="size-2 rounded-full bg-brand-green" />
          Secured by Payday
        </p>
      </div>
    </>
  );
}

/* ------------------------------------------------------------------------ */
/* Marks                                                                    */
/* ------------------------------------------------------------------------ */

function Usdc({ className }: { className?: string }) {
  return (
    <Image
      src="/payment-icons/usdc.svg"
      width={64}
      height={64}
      loading="eager"
      alt=""
      className={cn("shrink-0 rounded-full", className)}
    />
  );
}

function Monad({ className }: { className?: string }) {
  return (
    <Image
      src="/payment-icons/monad.svg"
      width={64}
      height={64}
      loading="eager"
      alt=""
      className={cn("shrink-0", className)}
    />
  );
}

/** "USDC" beside an amount, with its mark. */
function Token() {
  return (
    <span className="flex items-center gap-1.5 text-[15px] font-medium text-brand-subtle">
      <Usdc className="size-5" />
      USDC
    </span>
  );
}
