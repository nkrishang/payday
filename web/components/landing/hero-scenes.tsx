"use client";

import { ArrowRight, Check, Loader2, Wallet } from "lucide-react";
import Image from "next/image";
import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { AmountDue } from "@/components/checkout/amount";
import { Countdown } from "@/components/checkout/countdown";
import { RequestDetails } from "@/components/checkout/request-details";
import { Resolved } from "@/components/checkout/resolved";
import { Labeled } from "@/components/dashboard/labeled";
import { Button } from "@/components/ui/button";
import { controlStyles } from "@/components/ui/field";
import { StatusDot } from "@/components/ui/status-dot";
import type { CheckoutView, ReadyPayerDepositRequest } from "@/lib/checkout-state";
import { cn } from "@/lib/cn";
import { formatDisplayAmount, truncateAddress } from "@/lib/format";

/**
 * The hero's picture of the product: three scenes, one after another, on a
 * loop. A merchant issues a deposit request from the dashboard's composer, the
 * payer funds it from the hosted checkout, and the merchant's own app credits
 * it off the webhook. Each scene is built from the components the real pages
 * use — the composer's fields and step bar, the checkout's request document,
 * amount, address and outcome — driven by a clock instead of a person.
 *
 * Everything is a pure function of the clock: a scene reads `t`, its own
 * milliseconds since it began, and derives what is typed, which step is
 * open, where the cursor is. Nothing is scheduled, so a tab that sleeps and
 * wakes picks up mid-scene rather than firing a backlog of timers.
 *
 * The scenes are laid out on a fixed stage and scaled to whatever width the
 * card gets, so the composition never reflows on a phone.
 */

const STAGE = { width: 640, height: 384 } as const;

const SCENES = [
  { title: "Issue a deposit request", duration: 11_000 },
  { title: "The payer deposits", duration: 10_000 },
  { title: "Your app credits it", duration: 8_500 },
] as const;

/** A beat between scenes, in which the old one leaves and the new one arrives. */
const CROSSFADE_MS = 450;

const TOTAL_MS = SCENES.reduce((sum, scene) => sum + scene.duration, 0);

function sceneAt(index: number): (typeof SCENES)[number] {
  return SCENES[index] ?? SCENES[0];
}

const PAYER_WALLET = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
const DEPOSIT_ADDRESS = "0x9a3F5c18B0e4A7d2C9f6E1b3a8D4c7F2E5b9A0c2";
const TOKEN_ADDRESS = "0x754704Bc059F8C67012fEd69BC8A327a5aafb603";
const SETTLEMENT_TX = "0x8f3a2c9e1b7d4f60a5c3e8d2b9f14a7c6e0d3b5f2a8c1e9d7b4f6a3c0e2d5b8f";

/**
 * The request as the payer route would return it once the wallet is bound:
 * the shape the checkout's components take, with the same story's numbers.
 */
const REQUEST: ReadyPayerDepositRequest = {
  id: "dr_0198f80c-8d2f-7dc1-a369-90556a64f700",
  issuer_name: "Acme Corp",
  heading: "March retainer",
  payer_policy: { mode: "verified_email", expected_email_hint: "c***@globex.com" },
  requirements: {
    email: "approved",
    wallet: "approved",
    merchant_session: "not_required",
    complete: true,
  },
  status: "awaiting_deposit",
  payable: true,
  expires_at: "2026-09-02T11:57:00Z",
  server_timestamp: "1788000000",
  settlement_tx_hash: null,
  settlement_explorer_url: null,
  payer_message: null,
  content_unlocked: true,
  networks: [
    {
      chain: { id: "143", name: "Monad", native_symbol: "MON" },
      token: { symbol: "USDC", address: TOKEN_ADDRESS, decimals: 6 },
    },
  ],
  chain: { id: "143", name: "Monad", native_symbol: "MON" },
  token: { symbol: "USDC", address: TOKEN_ADDRESS, decimals: 6 },
  amount: "10.50",
  amount_base_units: "10500000",
  received: "0",
  received_base_units: "0",
  remaining: "10.50",
  remaining_base_units: "10500000",
  payer_wallet: PAYER_WALLET,
  address: DEPOSIT_ADDRESS,
  address_explorer_url: null,
  deposit_uri: `ethereum:${TOKEN_ADDRESS}@143/transfer?address=${DEPOSIT_ADDRESS}&uint256=10500000`,
  details: {
    amount: "10.50",
    amount_base_units: "10500000",
    payer: { name: "Globex LLC" },
    notes: null,
    reference: null,
    attachment: null,
  },
};

const SETTLED: ReadyPayerDepositRequest = {
  ...REQUEST,
  status: "settled",
  received: "10.50",
  received_base_units: "10500000",
  remaining: "0",
  remaining_base_units: "0",
  settlement_tx_hash: SETTLEMENT_TX,
};

/** The checkout's own words for the states the scene passes through. */
const VIEWS: Record<"awaiting" | "confirming" | "settled", CheckoutView> = {
  awaiting: {
    phase: "awaiting",
    tone: "neutral",
    label: "Awaiting deposit",
    title: "Send the deposit",
    detail:
      "Pay from the wallet you signed with. The deposit is credited once it is final on-chain.",
    showInstructions: true,
    showWalletStep: false,
    isTerminal: false,
  },
  confirming: {
    phase: "confirming",
    tone: "progress",
    label: "Confirming",
    title: "Transaction confirmed on-chain",
    detail: "Payday is settling the deposit to the merchant.",
    showInstructions: false,
    showWalletStep: false,
    isTerminal: false,
  },
  settled: {
    phase: "settled",
    tone: "success",
    label: "Settled",
    title: "Deposit complete",
    detail: "Exactly the requested amount reached the merchant. You can close this page.",
    showInstructions: false,
    showWalletStep: false,
    isTerminal: true,
  },
};

/* ------------------------------------------------------------------------ */
/* The clock                                                                */
/* ------------------------------------------------------------------------ */

/**
 * Milliseconds into the loop, advancing with the frame rate and wrapping at
 * the end. A frame after a long pause counts as a short one, so a background
 * tab does not skip a scene when it comes back.
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
      // Thirty frames a second is plenty for typing and a moving cursor, and
      // half the renders of a full-rate loop.
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
    // Deferred, like every other client-only read here.
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

/** The first `n` characters of `text` for a typist starting at `start`. */
function typed(t: number, text: string, start: number, perChar = 70): string {
  if (t < start) return "";
  return text.slice(0, Math.min(text.length, Math.floor((t - start) / perChar)));
}

/** True for the `hold` milliseconds after `at`. */
function within(t: number, at: number, hold: number): boolean {
  return t >= at && t < at + hold;
}

/** Linear progress from `from` to `to`, clamped to 0-1. */
function progress(t: number, from: number, to: number): number {
  if (t <= from) return 0;
  if (t >= to) return 1;
  return (t - from) / (to - from);
}

/* ------------------------------------------------------------------------ */
/* The director                                                             */
/* ------------------------------------------------------------------------ */

export function HeroScenes() {
  const reduced = useReducedMotion();
  const pinned = usePinnedMoment();
  // Without motion, hold the checkout at its settled state: the one frame
  // that says the most on its own.
  const frozenAt = pinned ?? (reduced ? SCENES[0].duration + 9_000 : null);
  const t = useLoopClock(TOTAL_MS, frozenAt);

  let index = 0;
  let local = t;
  while (index < SCENES.length - 1 && local >= sceneAt(index).duration) {
    local -= sceneAt(index).duration;
    index += 1;
  }
  const scene = sceneAt(index);
  const leaving = !reduced && local >= scene.duration - CROSSFADE_MS;

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
      className="landing-scene relative w-full overflow-hidden rounded-[10px] border border-brand-black bg-surface"
      style={{ aspectRatio: `${STAGE.width} / ${STAGE.height}` }}
      aria-label={`How Payday works: ${SCENES.map((entry) => entry.title).join(", ")}`}
      role="img"
    >
      <div
        className="absolute top-0 left-0 origin-top-left"
        style={{ width: STAGE.width, height: STAGE.height, transform: `scale(${scale})` }}
      >
        <div
          key={index}
          className={cn(
            "absolute inset-0",
            leaving ? "landing-scene-leave" : "landing-scene-enter",
          )}
        >
          {index === 0 ? <ComposerScene t={local} /> : null}
          {index === 1 ? <CheckoutScene t={local} /> : null}
          {index === 2 ? <CreditScene t={local} /> : null}
        </div>

        <SceneStrip index={index} progress={progress(local, 0, scene.duration)} />
      </div>
    </div>
  );
}

/** Which scene is playing, with its progress, along the foot of the card. */
function SceneStrip({ index, progress: fraction }: { index: number; progress: number }) {
  return (
    <ol className="absolute inset-x-0 bottom-0 flex items-center gap-4 border-t border-line bg-surface px-4 py-2.5">
      {SCENES.map((scene, at) => (
        <li key={scene.title} className="flex min-w-0 flex-1 items-center gap-2">
          <span className="h-[3px] w-full overflow-hidden rounded-full bg-line">
            <span
              className="block h-full origin-left rounded-full bg-brand-green"
              style={{ transform: `scaleX(${at < index ? 1 : at === index ? fraction : 0})` }}
            />
          </span>
        </li>
      ))}
      <li className="shrink-0 text-[11px] font-medium text-muted">
        <span className="tabular">{index + 1}</span>
        <span className="ml-1.5">{sceneAt(index).title}</span>
      </li>
    </ol>
  );
}

/* ------------------------------------------------------------------------ */
/* The cursor                                                               */
/* ------------------------------------------------------------------------ */

/**
 * A pointer that glides to whatever element carries `data-cursor={target}`
 * and presses when told. It measures the target in the stage's own space, so
 * the layout is free to be whatever the components make it.
 */
function Cursor({ target, pressed }: { target: string | null; pressed: boolean }) {
  // The anchor is always in the tree, so the stage is its parent from the
  // first layout effect on; a ref on the stage itself would still be
  // unattached when a scene mounts with a target already chosen.
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
      x: (rect.left - box.left + rect.width * 0.62) / scale,
      y: (rect.top - box.top + rect.height * 0.58) / scale,
    });
  }, [target]);

  return (
    <>
      <span ref={anchorRef} hidden />
      {at ? (
        <svg
          aria-hidden="true"
          viewBox="0 0 24 24"
          className="landing-cursor pointer-events-none absolute top-0 left-0 z-20 size-5 drop-shadow-[0_1px_2px_rgb(0_0_0/0.35)]"
          style={{ transform: `translate(${at.x}px, ${at.y}px) scale(${pressed ? 0.82 : 1})` }}
        >
          <path
            d="M5.5 3.2 19 12.1l-6.1 1.3 3.3 6.3-2.6 1.3-3.3-6.3L5.5 19.3Z"
            fill="#0f0f0e"
            stroke="#f6f2ea"
            strokeWidth="1.4"
            strokeLinejoin="round"
          />
        </svg>
      ) : null}
    </>
  );
}

/* ------------------------------------------------------------------------ */
/* Scene 1: the composer                                                    */
/* ------------------------------------------------------------------------ */

const STEPS = ["Amount", "Billing", "Verification", "Review"] as const;

/** When each beat of the first scene lands, in milliseconds. */
const S1 = {
  amountTyped: 500,
  toBilling: 2_100,
  payerTyped: 2_700,
  reasonTyped: 3_900,
  toVerification: 5_500,
  emailChosen: 6_100,
  emailTyped: 6_500,
  toReview: 8_100,
  issue: 9_300,
  issued: 10_000,
} as const;

function ComposerScene({ t }: { t: number }) {
  const step = t < S1.toBilling ? 0 : t < S1.toVerification ? 1 : t < S1.toReview ? 2 : 3;
  const amount = typed(t, "10.50", S1.amountTyped, 160);
  const payer = typed(t, "Globex LLC", S1.payerTyped);
  const reason = typed(t, "March retainer", S1.reasonTyped);
  const verified = t >= S1.emailChosen;
  const email = typed(t, "cfo@globex.com", S1.emailTyped);
  const issuing = within(t, S1.issue, S1.issued - S1.issue);
  const issued = t >= S1.issued;

  const target =
    t < S1.toBilling - 300
      ? null
      : step === 0
        ? "continue"
        : step === 1
          ? t < S1.toVerification - 300
            ? null
            : "continue"
          : step === 2
            ? t < S1.emailChosen - 300
              ? null
              : t < S1.emailTyped
                ? "verified"
                : t < S1.toReview - 300
                  ? null
                  : "continue"
            : t < S1.issue - 400
              ? null
              : "issue";
  const pressed =
    within(t, S1.toBilling - 120, 140) ||
    within(t, S1.toVerification - 120, 140) ||
    within(t, S1.emailChosen - 120, 140) ||
    within(t, S1.toReview - 120, 140) ||
    within(t, S1.issue - 120, 140);

  return (
    <div className="absolute inset-0 px-6 pt-4 pb-12">
      <Cursor target={target} pressed={pressed} />

      <div className="flex items-start justify-between gap-4">
        <h2 className="text-[17px] leading-tight font-medium tracking-[-0.03em]">
          New deposit request<span className="text-brand-yellow">.</span>
        </h2>
        <span className="text-[12px] font-medium text-muted">Cancel</span>
      </div>

      <ol className="mt-2.5 flex gap-2">
        {STEPS.map((entry, index) => (
          <li key={entry} className="flex-1">
            <span className="block h-[3px] overflow-hidden rounded-full bg-line">
              <span
                className={cn(
                  "block h-full origin-left rounded-full bg-brand-green transition-transform duration-500 ease-out",
                  index <= step ? "scale-x-100" : "scale-x-0",
                )}
              />
            </span>
            <span
              className={cn(
                "mt-1.5 block text-[11px] font-medium transition-colors",
                index === step ? "text-ink" : "text-faint",
              )}
            >
              <span className="tabular">{index + 1}</span>
              <span className="ml-1.5">{entry}</span>
            </span>
          </li>
        ))}
      </ol>

      <div className="mt-3.5 grid grid-cols-[minmax(0,1fr)_200px] items-start gap-5">
        <div className="min-w-0">
          <div key={step} className="landing-scene-step">
            {step === 0 ? (
              <Labeled label="Amount" required>
                <span className="relative block">
                  <span
                    className={cn(
                      controlStyles,
                      "tabular flex h-12 items-center pr-[70px] text-[22px] font-medium tracking-tight",
                      amount ? "text-ink" : "text-faint",
                    )}
                  >
                    {amount || "0.00"}
                    {within(t, S1.amountTyped - 400, 1_300) ? <Caret /> : null}
                  </span>
                  <span
                    aria-hidden="true"
                    className="absolute top-1/2 right-3.5 flex -translate-y-1/2 items-center gap-1.5 text-[12px] text-faint"
                  >
                    <Usdc className="size-4" />
                    USDC
                  </span>
                </span>
              </Labeled>
            ) : null}

            {step === 1 ? (
              <div className="grid grid-cols-2 gap-x-4 gap-y-3">
                <Labeled label="Payer" required>
                  <FakeInput
                    value={payer}
                    placeholder="Globex LLC"
                    typing={within(t, S1.payerTyped - 300, 1_200)}
                  />
                </Labeled>
                <Labeled label="Email">
                  <FakeInput value="cfo@globex.com" placeholder="" />
                </Labeled>
                <Labeled label="Reason" required>
                  <FakeInput
                    value={reason}
                    placeholder="March retainer"
                    typing={within(t, S1.reasonTyped - 300, 1_400)}
                  />
                </Labeled>
                <Labeled label="Reference">
                  <FakeInput value="INV-1042" placeholder="" mono />
                </Labeled>
              </div>
            ) : null}

            {step === 2 ? (
              <div className="grid gap-3">
                <div role="radiogroup" aria-label="Payer policy" className="grid grid-cols-2 gap-2">
                  <Policy
                    label="Permissionless"
                    description="Anyone holding the link can view and fund it."
                    checked={!verified}
                  />
                  <Policy
                    cursor="verified"
                    label="Verified email"
                    description="The payer proves the expected mailbox first."
                    checked={verified}
                  />
                </div>
                {verified ? (
                  <Labeled label="Expected payer email" required>
                    <FakeInput
                      value={email}
                      placeholder=""
                      typing={within(t, S1.emailTyped - 200, 1_300)}
                    />
                  </Labeled>
                ) : null}
              </div>
            ) : null}

            {step === 3 ? (
              <dl
                aria-label="Request summary"
                className="grid gap-2 rounded-[12px] border border-line bg-surface px-3.5 py-3 text-[12.5px]"
              >
                <Row label="Amount">
                  <span className="tabular inline-flex items-center gap-1">
                    10.50 <Usdc className="size-3.5" /> USDC
                  </span>
                </Row>
                <Row label="Issued by">Acme Corp</Row>
                <Row label="Payer">
                  Globex LLC <span className="text-muted">· cfo@globex.com</span>
                </Row>
                <Row label="Verification">Verified email</Row>
                <Row label="Expires in">24 hours</Row>
              </dl>
            ) : null}
          </div>

          <div className="mt-3.5 flex items-center justify-end gap-3">
            {issued ? (
              <span className="flex items-center gap-2 text-[13px] font-medium text-success">
                <span className="flex size-5 items-center justify-center rounded-full border border-success/40">
                  <Check className="size-3" />
                </span>
                Issued · link emailed to cfo@globex.com
              </span>
            ) : step === 3 ? (
              <Button data-cursor="issue" type="button" disabled={issuing}>
                {issuing ? <Loader2 className="size-4 animate-spin" /> : null}
                Issue deposit request
              </Button>
            ) : (
              <Button data-cursor="continue" type="button">
                Continue
                <ArrowRight className="size-4" />
              </Button>
            )}
          </div>
        </div>

        <aside className="min-w-0 rounded-[14px] border border-line bg-surface p-4">
          <p className="text-[10px] tracking-[0.12em] text-faint uppercase">Deposit request</p>
          <p className="mt-2 flex items-center gap-1.5">
            <span
              className={cn(
                "tabular text-[26px] leading-none font-medium tracking-[-0.03em] transition-colors",
                amount ? "text-ink" : "text-faint/50",
              )}
            >
              {amount ? formatDisplayAmount(amount) : "0.00"}
            </span>
            <span className="flex items-center gap-1 text-[12px] text-faint">
              <Usdc className="size-3.5" />
              USDC
            </span>
          </p>
          {reason ? <p className="mt-1 text-[12px] text-muted">{reason}</p> : null}
          <dl className="mt-3 grid gap-2 border-t border-line pt-3 text-[12px]">
            <Line label="From" value="Acme Corp" />
            <Line label="To" value={payer} />
            <Line label="Verification" value={verified ? "Verified email" : ""} />
            <Line label="Expires in" value="24 hours" />
            <Line
              label="Settles to"
              value={truncateAddress("0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC")}
              mono
            />
          </dl>
        </aside>
      </div>
    </div>
  );
}

/** A control with a value the clock typed, in the composer's own field style. */
function FakeInput({
  value,
  placeholder,
  typing = false,
  mono = false,
}: {
  value: string;
  placeholder: string;
  typing?: boolean;
  mono?: boolean;
}) {
  return (
    <span
      className={cn(
        controlStyles,
        "flex h-10 items-center",
        mono && "font-mono text-[12.5px]",
        value ? "text-ink" : "text-faint",
      )}
    >
      {value || placeholder}
      {typing ? <Caret /> : null}
    </span>
  );
}

function Caret() {
  return (
    <span
      aria-hidden="true"
      className="landing-caret ml-px inline-block h-[1.1em] w-px bg-ink align-middle"
    />
  );
}

function Policy({
  label,
  description,
  checked,
  cursor,
}: {
  label: string;
  description: string;
  checked: boolean;
  cursor?: string;
}) {
  return (
    <span
      data-cursor={cursor}
      className={cn(
        "flex gap-2.5 rounded-[12px] border px-3 py-2.5 transition-colors",
        checked ? "border-brand-green/60 bg-brand-green/[0.07]" : "border-line bg-surface",
      )}
    >
      <span
        aria-hidden="true"
        className={cn(
          "mt-0.5 flex size-3.5 shrink-0 items-center justify-center rounded-full border transition-colors",
          checked ? "border-brand-green bg-brand-green" : "border-line-strong",
        )}
      >
        {checked ? <span className="size-1.5 rounded-full bg-brand-black" /> : null}
      </span>
      <span>
        <span className="block text-[12.5px] font-medium">{label}</span>
        <span className="mt-0.5 block text-[11px] leading-snug text-muted">{description}</span>
      </span>
    </span>
  );
}

function Row({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="flex items-baseline justify-between gap-4">
      <dt className="text-faint">{label}</dt>
      <dd className="min-w-0 truncate text-right">{children}</dd>
    </div>
  );
}

function Line({ label, value, mono = false }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="flex items-baseline justify-between gap-3">
      <dt className="text-faint">{label}</dt>
      <dd
        className={cn(
          "min-w-0 truncate text-right",
          mono && "font-mono text-[11.5px]",
          !value && "text-faint",
        )}
      >
        {value || "—"}
      </dd>
    </div>
  );
}

function Usdc({ className }: { className?: string }) {
  return (
    <Image
      src="/payment-icons/usdc.svg"
      width={64}
      height={64}
      alt=""
      className={cn("shrink-0 rounded-full", className)}
    />
  );
}

/* ------------------------------------------------------------------------ */
/* Scene 2: the checkout                                                    */
/* ------------------------------------------------------------------------ */

const S2 = {
  toPay: 1_600,
  walletOpen: 1_900,
  confirm: 3_600,
  walletClosed: 4_300,
  confirming: 6_000,
  settled: 7_200,
} as const;

function CheckoutScene({ t }: { t: number }) {
  const detailsRef = useRef<HTMLDivElement>(null);
  const [detailsHeight, setDetailsHeight] = useState(0);
  // Once the deposit is in, the card scrolls the request document out of
  // the way so the outcome stands where the amount stood.
  useLayoutEffect(() => {
    setDetailsHeight(detailsRef.current?.offsetHeight ?? 0);
  }, []);

  const phase = t < S2.confirming ? "awaiting" : t < S2.settled ? "confirming" : "settled";
  const view = VIEWS[phase];
  const sent = t >= S2.confirm;
  const signing = within(t, S2.walletOpen, S2.confirm - S2.walletOpen);
  const walletOpen = within(t, S2.walletOpen, S2.walletClosed - S2.walletOpen);
  const leaving = t >= S2.walletClosed - 250 && t < S2.walletClosed;
  const seconds = 86_321 - Math.floor(t / 1000);

  const target =
    t < S2.toPay - 500
      ? null
      : t < S2.walletOpen + 700
        ? "pay"
        : t < S2.walletClosed
          ? "confirm"
          : null;
  const pressed = within(t, S2.toPay + 160, 140) || within(t, S2.confirm - 120, 140);

  return (
    <div className="absolute inset-0 bg-canvas pb-10">
      <Cursor target={target} pressed={pressed} />

      <div
        className="absolute top-3 left-1/2 w-[380px] origin-top"
        style={{ transform: "translateX(-50%) scale(0.8)" }}
      >
        <div className="overflow-hidden rounded-[16px] border border-line bg-surface">
          <div className="relative z-10 flex items-center justify-between gap-3 border-b border-line bg-surface px-5 py-3">
            <span className="flex items-center gap-2 text-[13px] font-medium">
              <StatusDot tone={view.tone} pulse={!view.isTerminal} />
              {view.label}
            </span>
            {view.showInstructions ? <Countdown seconds={seconds} /> : null}
          </div>

          <div
            className="transition-transform duration-700 ease-[cubic-bezier(0.16,1,0.3,1)]"
            style={{
              transform: phase === "awaiting" ? "none" : `translateY(-${detailsHeight}px)`,
            }}
          >
            <div ref={detailsRef}>
              <RequestDetails payment={REQUEST} />
            </div>

            {phase === "awaiting" ? (
              <section aria-label={view.title} className="px-5 py-5">
                <AmountDue payment={REQUEST} />
                <div className="mt-5">
                  <Button data-cursor="pay" size="lg" disabled={sent}>
                    {sent ? (
                      <Loader2 className="size-4 animate-spin" />
                    ) : (
                      <Wallet className="size-4" />
                    )}
                    {sent
                      ? "Waiting for confirmation…"
                      : signing
                        ? "Confirm in your wallet…"
                        : "Pay 10.50 USDC"}
                  </Button>
                </div>
              </section>
            ) : (
              <Resolved
                payment={phase === "settled" ? SETTLED : REQUEST}
                view={view}
                pendingTxHash={phase === "settled" ? null : SETTLEMENT_TX}
              />
            )}
          </div>
        </div>
      </div>

      {walletOpen ? <WalletSheet leaving={leaving} confirmed={sent} /> : null}
    </div>
  );
}

/** The payer's wallet asking for the transfer, in a plain wallet's clothes. */
function WalletSheet({ leaving, confirmed }: { leaving: boolean; confirmed: boolean }) {
  return (
    <div
      className={cn(
        "absolute right-6 bottom-14 w-[224px] rounded-[14px] border border-line-strong bg-surface p-4 shadow-[0_18px_40px_-16px_rgb(15_15_14/0.35)]",
        leaving ? "landing-sheet-leave" : "landing-sheet-enter",
      )}
    >
      <div className="flex items-center gap-2">
        <span className="flex size-6 items-center justify-center rounded-full bg-brand-black text-brand-white">
          <Wallet className="size-3" />
        </span>
        <span className="text-[12px] font-medium">Transfer request</span>
      </div>
      <p className="tabular mt-3 text-[22px] leading-none font-semibold tracking-tight">
        10.50 <span className="text-[13px] font-medium text-muted">USDC</span>
      </p>
      <dl className="mt-3 grid gap-1 text-[11px]">
        <div className="flex justify-between gap-3">
          <dt className="text-faint">From</dt>
          <dd className="font-mono">{truncateAddress(PAYER_WALLET)}</dd>
        </div>
        <div className="flex justify-between gap-3">
          <dt className="text-faint">To</dt>
          <dd className="font-mono">{truncateAddress(DEPOSIT_ADDRESS)}</dd>
        </div>
        <div className="flex justify-between gap-3">
          <dt className="text-faint">Network</dt>
          <dd>Monad</dd>
        </div>
      </dl>
      <div className="mt-3 grid grid-cols-2 gap-2">
        <Button variant="secondary" size="sm" disabled={confirmed}>
          Reject
        </Button>
        <Button data-cursor="confirm" size="sm" disabled={confirmed}>
          {confirmed ? <Check className="size-3.5" /> : null}
          {confirmed ? "Sent" : "Confirm"}
        </Button>
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------------ */
/* Scene 3: the merchant's app                                              */
/* ------------------------------------------------------------------------ */

const S3 = {
  linesFrom: 300,
  perLine: 170,
  delivered: 3_000,
  credited: 3_500,
  countTo: 4_400,
} as const;

const EVENT_LINES = [
  `{`,
  `  "id": "evt_0198f80c-4444-7dc1",`,
  `  "type": "deposit_request.settled",`,
  `  "data": {`,
  `    "deposit_request": {`,
  `      "id": "dr_0198f80c-8d2f-7dc1",`,
  `      "status": "settled",`,
  `      "amount": "10.500000",`,
  `      "reference": "INV-1042",`,
  `      "payer_reference": "user_123"`,
  `    }`,
  `  }`,
  `}`,
] as const;

const BALANCE_BEFORE = 240;
const BALANCE_AFTER = 250.5;

function CreditScene({ t }: { t: number }) {
  const shown = Math.min(
    EVENT_LINES.length,
    Math.max(0, Math.floor((t - S3.linesFrom) / S3.perLine) + 1),
  );
  const delivered = t >= S3.delivered;
  const credited = t >= S3.credited;
  const balance =
    BALANCE_BEFORE + (BALANCE_AFTER - BALANCE_BEFORE) * ease(progress(t, S3.credited, S3.countTo));

  return (
    <div className="absolute inset-0 grid grid-cols-[minmax(0,1fr)_248px] gap-5 bg-canvas px-6 pt-5 pb-12">
      <div className="relative min-w-0 overflow-hidden rounded-[14px] border border-line bg-[#0f0f0e] text-[#f6f2ea]">
        <div className="grid gap-1 border-b border-white/10 px-4 py-2.5 font-mono text-[11px]">
          <span className="flex items-center gap-2.5">
            <span className="rounded-[5px] bg-brand-green px-1.5 py-0.5 font-semibold text-brand-black">
              POST
            </span>
            <span className="truncate text-white/80">/payday/webhook</span>
          </span>
          <span className="flex items-center gap-1.5 whitespace-nowrap text-white/50">
            <span>Payday-Event-Type:</span>
            <span className="text-brand-yellow">deposit_request.settled</span>
          </span>
        </div>
        <pre className="px-4 py-3 font-mono text-[11.5px] leading-[1.55]">
          {EVENT_LINES.slice(0, shown).map((line, index) => (
            <span key={index} className="landing-scene-line block whitespace-pre">
              {line}
            </span>
          ))}
        </pre>
        {delivered ? (
          <div className="landing-scene-line absolute right-3 bottom-3 flex items-center gap-2 rounded-full border border-brand-green/40 bg-brand-green/10 px-2.5 py-1 font-mono text-[11px] text-brand-green">
            <span className="size-1.5 rounded-full bg-brand-green" />
            200 OK · 41 ms
          </div>
        ) : null}
      </div>

      <div className="min-w-0 rounded-[14px] border border-line bg-surface p-4">
        <div className="flex items-center gap-2.5">
          <span className="flex size-7 items-center justify-center rounded-full bg-brand-yellow text-[12px] font-semibold text-brand-black">
            G
          </span>
          <span className="min-w-0">
            <span className="block truncate text-[13px] font-medium">Globex LLC</span>
            <span className="block text-[11px] text-faint">user_123 · your app</span>
          </span>
        </div>

        <p className="mt-4 text-[10px] font-medium tracking-[0.14em] text-faint uppercase">
          Balance
        </p>
        <p className="tabular mt-1 flex items-baseline gap-1.5 text-[28px] leading-none font-semibold tracking-tight">
          {balance.toFixed(2)}
          <span className="text-[13px] font-medium text-muted">USDC</span>
        </p>

        <ul className="mt-4 grid gap-2 border-t border-line pt-3 text-[12px]">
          {credited ? (
            <li className="landing-scene-line flex items-center justify-between gap-3 rounded-[10px] border border-success/30 bg-success/[0.06] px-3 py-2">
              <span className="flex min-w-0 items-center gap-2">
                <StatusDot tone="success" />
                <span className="min-w-0">
                  <span className="block truncate font-medium">Deposit credited</span>
                  <span className="block font-mono text-[10.5px] text-faint">dr_0198f80c…f700</span>
                </span>
              </span>
              <span className="tabular shrink-0 font-medium text-success">+10.50</span>
            </li>
          ) : null}
          <li className="flex items-center justify-between gap-3 px-3 py-1.5 text-muted">
            <span className="flex items-center gap-2">
              <StatusDot tone="neutral" />
              Subscription
            </span>
            <span className="tabular">−12.00</span>
          </li>
          <li className="flex items-center justify-between gap-3 px-3 py-1.5 text-muted">
            <span className="flex items-center gap-2">
              <StatusDot tone="neutral" />
              Deposit credited
            </span>
            <span className="tabular">+52.00</span>
          </li>
        </ul>
      </div>
    </div>
  );
}

function ease(fraction: number): number {
  return 1 - Math.pow(1 - fraction, 3);
}
