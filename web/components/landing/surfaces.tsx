import { Wallet } from "lucide-react";
import type { CSSProperties, ReactNode } from "react";
import { cn } from "@/lib/cn";
import { Countdown } from "./countdown";
import { Stage } from "./stage";
import { Editor, Highlighted, Monad, Usdc } from "./code";

/**
 * One deposit request, three ways to put it in front of the payer: the
 * page Gum hosts for it, the same request inside the merchant's own app,
 * and the fields alone for a merchant who builds the whole thing. Same
 * id, same address, same clock on all three.
 */

const REQUEST_ID = "dr_0198f80c…f700";
const ADDRESS = "0x9a3F…A0c2";

const HEADLESS_CODE = `const deposit = await gum.depositRequests.create({
  amount: "250.00",
  currency: "USDC",
  payer: { name: "Jordan Diaz" },
});

// Render it however you like.
deposit.address      // "${ADDRESS}"
deposit.expires_at   // "2026-09-16T15:20:00Z"
deposit.hosted_url   // "https://gum.money/pay/${REQUEST_ID}"`;

export function Surfaces() {
  return (
    <div>
      <div className="grid items-start gap-6 lg:grid-cols-[auto_minmax(0,1fr)_minmax(0,1fr)] lg:gap-8">
        <Surface index={3} label="Hosted" note="gum.money/pay/…">
          <Phone />
        </Surface>
        <Surface index={4} label="Embedded" note="in your own app">
          <Embedded />
        </Surface>
        <Surface index={5} label="Headless" note="your own UI">
          <div className="h-[420px] overflow-hidden rounded-[18px] bg-gum-black p-3 text-gum-white">
            <Editor
              file="server.ts"
              footer={
                <span className="flex items-center gap-2 text-gum-pink">
                  <span className="size-2 rounded-full bg-gum-pink" />
                  201 Created
                </span>
              }
            >
              <span className="text-[13px]">
                <Highlighted code={HEADLESS_CODE} />
              </span>
            </Editor>
          </div>
        </Surface>
      </div>
    </div>
  );
}

function Surface({
  index,
  label,
  note,
  children,
}: {
  index: number;
  label: string;
  note: string;
  children: ReactNode;
}) {
  return (
    <div data-reveal="scale" style={{ "--i": index } as CSSProperties} className="min-w-0">
      <p className="mb-3 flex items-baseline gap-2 text-[13px]">
        <span className="font-medium">{label}</span>
        <span className="text-gum-grey">{note}</span>
      </p>
      {children}
    </div>
  );
}

/** The hosted page, on a phone. */
function Phone() {
  return (
    <div className="mx-auto h-[420px] w-[236px] overflow-hidden rounded-[30px] border-[5px] border-gum-black bg-gum-white text-gum-black shadow-[0_24px_60px_-36px_rgb(18_18_18/0.6)]">
      <div className="flex items-center justify-between px-5 pt-3 text-[10px] font-semibold">
        <span className="tabular">9:41</span>
        <span className="h-[5px] w-[64px] rounded-full bg-gum-black" />
        <span className="flex gap-0.5">
          <span className="size-[5px] rounded-full bg-gum-black" />
          <span className="size-[5px] rounded-full bg-gum-black" />
          <span className="size-[5px] rounded-full bg-gum-black/30" />
        </span>
      </div>
      <div className="px-4 pt-5">
        <p className="flex items-center gap-2 text-[12px] font-semibold">
          <span className="flex size-5 items-center justify-center rounded-[6px] bg-gum-black text-[9px] font-bold text-gum-white">
            A
          </span>
          Acme
        </p>
        <p className="mt-5 text-[10px] font-medium tracking-[0.14em] text-gum-grey uppercase">
          Deposit
        </p>
        <p className="tabular mt-1 flex items-baseline gap-1.5 text-[28px] leading-none font-semibold tracking-tight">
          250.00
          <span className="flex items-center gap-1 text-[11px] font-medium text-gum-grey">
            <Usdc className="size-3.5" />
            USDC
          </span>
        </p>
        <p className="mt-2 text-[11px] text-gum-grey">
          For <span className="text-gum-black">Jordan Diaz</span> · <Countdown />
        </p>
        <div className="mt-5 grid gap-2 text-[11px]">
          <div className="flex items-center justify-between rounded-[8px] border border-gum-grey/30 px-2.5 py-2">
            <span className="text-gum-grey">Address</span>
            <span className="font-mono">{ADDRESS}</span>
          </div>
          <div className="flex items-center justify-between rounded-[8px] border border-gum-grey/30 px-2.5 py-2">
            <span className="text-gum-grey">Network</span>
            <span className="flex items-center gap-1 font-medium">
              <Monad className="size-3.5" />
              Monad
            </span>
          </div>
        </div>
        <span className="mt-4 flex h-10 items-center justify-center gap-2 rounded-[8px] bg-gum-black text-[12px] font-medium text-gum-white">
          <Wallet className="size-3.5" />
          Pay from wallet
        </span>
        <p className="mt-3 flex items-center justify-center gap-1.5 text-[10px] text-gum-grey">
          <span className="size-1.5 rounded-full bg-gum-pink" />
          Secured by Gum
        </p>
      </div>
    </div>
  );
}

/**
 * The same request inside a perps exchange's own app, in that app's own
 * clothes: dark, dense, amber where it wants attention. The deposit dialog
 * is dressed to match, because that is what embedding means; only the
 * address, the clock and the "Secured by Gum" line are Gum's.
 */
const APP = { width: 520, height: 420 } as const;

/** A day of five-minute candles, the same every time. */
const CANDLES = (() => {
  let seed = 7;
  const random = () => {
    seed = (seed * 16_807) % 2_147_483_647;
    return seed / 2_147_483_647;
  };
  let price = 2_398;
  return Array.from({ length: 40 }, () => {
    const open = price;
    const close = open + (random() - 0.52) * 9;
    const high = Math.max(open, close) + random() * 4;
    const low = Math.min(open, close) - random() * 4;
    price = close;
    return { open, close, high, low };
  });
})();

const ASKS = [
  ["2,392.59", "0.0031", 0.9],
  ["2,392.41", "4.1805", 0.6],
  ["2,392.07", "0.4181", 0.35],
  ["2,391.87", "0.6272", 0.3],
  ["2,391.78", "0.1673", 0.25],
  ["2,391.53", "0.0209", 0.22],
] as const;
const BIDS = [
  ["2,391.40", "4.8077", 0.24],
  ["2,391.07", "0.0209", 0.28],
  ["2,390.79", "4.1805", 0.4],
  ["2,390.13", "26.446", 0.62],
  ["2,389.95", "4.1805", 0.85],
  ["2,389.26", "0.0251", 0.9],
] as const;

function Embedded() {
  const width = 300;
  const height = 150;
  const prices = CANDLES.flatMap((candle) => [candle.high, candle.low]);
  const min = Math.min(...prices);
  const max = Math.max(...prices);
  const y = (price: number) => ((max - price) / (max - min)) * (height - 12) + 6;
  const step = width / CANDLES.length;
  const last = CANDLES[CANDLES.length - 1]!;

  return (
    <div className="overflow-hidden rounded-[18px] bg-gum-black p-3">
      <Stage
        width={APP.width}
        height={APP.height}
        label="A perpetuals exchange's app, dark and dense, with Gum's deposit dialog open over it in the app's own style."
        className="rounded-[12px] bg-[#0e0f12] text-[#e8e9ec]"
      >
        <div className="flex h-9 items-center gap-4 border-b border-white/[0.07] px-3 text-[10.5px]">
          <span className="flex items-center gap-1.5 font-semibold tracking-[0.14em]">
            <span className="size-3 rounded-full bg-[#f5a524]" />
            EMBER
          </span>
          <span>Trade</span>
          <span className="text-[#7c818c]">Portfolio</span>
          <span className="text-[#7c818c]">Vaults</span>
          <span className="text-[#7c818c]">Referrals</span>
          <span className="ml-auto flex h-6 items-center rounded-[5px] bg-[#f5a524] px-2.5 text-[10px] font-semibold text-[#0e0f12]">
            Deposit
          </span>
          <span className="flex size-6 items-center justify-center rounded-full bg-[#2a2d34] text-[9px] font-semibold">
            JD
          </span>
        </div>

        <div className="flex h-9 items-center gap-4 border-b border-white/[0.07] px-3 text-[10px]">
          <span className="font-semibold">
            ETH<span className="text-[#7c818c]">USD</span>
            <span className="ml-1.5 rounded-[4px] border border-white/15 px-1 text-[8.5px] font-normal text-[#7c818c]">
              20x
            </span>
          </span>
          <span className="tabular font-semibold">
            2,391.51 <span className="text-[#e5484d]">-1.00%</span>
          </span>
          <span className="tabular text-[#7c818c]">
            Mark <span className="text-[#e8e9ec]">2,391.69</span>
          </span>
          <span className="tabular text-[#7c818c]">
            Funding <span className="text-[#30a46c]">+0.0018%</span>
          </span>
          <span className="tabular text-[#7c818c]">
            24h vol <span className="text-[#e8e9ec]">$1.46m</span>
          </span>
        </div>

        <div className="grid grid-cols-[minmax(0,1fr)_160px]">
          <div className="border-r border-white/[0.07] px-3 pt-2">
            <div className="flex items-center gap-3 text-[9.5px] text-[#7c818c]">
              <span className="text-[#e8e9ec]">ETH/USDT-P · 5m</span>
              <span className="tabular">
                O <span className="text-[#30a46c]">2,390.12</span> H{" "}
                <span className="text-[#30a46c]">2,392.66</span> L{" "}
                <span className="text-[#30a46c]">2,390.07</span> C{" "}
                <span className="text-[#30a46c]">2,391.51</span>
              </span>
            </div>
            <svg viewBox={`0 0 ${width + 44} ${height}`} className="mt-1 block w-full">
              {[0, 0.25, 0.5, 0.75, 1].map((f) => (
                <g key={f}>
                  <line
                    x1="0"
                    x2={width}
                    y1={6 + f * (height - 12)}
                    y2={6 + f * (height - 12)}
                    stroke="#ffffff"
                    strokeOpacity="0.06"
                  />
                  <text
                    x={width + 8}
                    y={6 + f * (height - 12) + 3}
                    fontSize="8"
                    fill="#7c818c"
                    style={{ fontFamily: "inherit" }}
                  >
                    {(max - f * (max - min)).toFixed(0)}
                  </text>
                </g>
              ))}
              {CANDLES.map((candle, index) => {
                const up = candle.close >= candle.open;
                const x = index * step + step / 2;
                const top = y(Math.max(candle.open, candle.close));
                const bottom = y(Math.min(candle.open, candle.close));
                const color = up ? "#30a46c" : "#e5484d";
                return (
                  <g key={index}>
                    <line x1={x} x2={x} y1={y(candle.high)} y2={y(candle.low)} stroke={color} />
                    <rect
                      x={x - step * 0.3}
                      y={top}
                      width={step * 0.6}
                      height={Math.max(1, bottom - top)}
                      fill={color}
                    />
                  </g>
                );
              })}
              <line
                x1="0"
                x2={width}
                y1={y(last.close)}
                y2={y(last.close)}
                stroke="#30a46c"
                strokeDasharray="2 3"
              />
            </svg>
            <div className="flex justify-between pb-2 text-[8.5px] text-[#7c818c]">
              <span>09:00</span>
              <span>12:00</span>
              <span>15:00</span>
              <span>18:00</span>
            </div>
          </div>

          <div className="px-2 pt-2 text-[9px]">
            <div className="flex gap-3 border-b border-white/[0.07] pb-1.5">
              <span>Order book</span>
              <span className="text-[#7c818c]">Trades</span>
            </div>
            <div className="mt-1.5 grid gap-[3px]">
              {ASKS.map(([price, size, depth]) => (
                <Level key={price} price={price} size={size} depth={depth} tone="ask" />
              ))}
              <div className="tabular flex items-center justify-between py-1 text-[10px]">
                <span className="text-[#e5484d]">2,391.42 ↓</span>
                <span className="text-[#7c818c]">0.13 bps</span>
              </div>
              {BIDS.map(([price, size, depth]) => (
                <Level key={price} price={price} size={size} depth={depth} tone="bid" />
              ))}
            </div>
          </div>
        </div>

        <div className="absolute inset-x-0 bottom-0 flex h-8 items-center gap-4 border-t border-white/[0.07] px-3 text-[9.5px] text-[#7c818c]">
          <span className="text-[#e8e9ec]">Positions</span>
          <span>Open orders</span>
          <span>Order history</span>
          <span>Funding</span>
        </div>

        {/* Gum's dialog, in the app's clothes. */}
        <div className="absolute inset-0 bg-[#0e0f12]/70" />
        <div className="absolute top-1/2 left-1/2 w-[236px] -translate-x-1/2 -translate-y-1/2 rounded-[12px] border border-white/[0.1] bg-[#16181d] p-4 shadow-[0_30px_80px_-24px_rgb(0_0_0/0.8)]">
          <p className="flex items-center justify-between text-[12px] font-semibold">
            Deposit
            <span className="text-[#7c818c]">✕</span>
          </p>
          <p className="tabular mt-3 flex items-baseline gap-1.5 text-[26px] leading-none font-semibold tracking-tight">
            250.00
            <span className="flex items-center gap-1 text-[11px] font-medium text-[#7c818c]">
              <Usdc className="size-3.5" />
              USDC
            </span>
          </p>
          <dl className="mt-3 grid gap-1.5 text-[10.5px]">
            <div className="flex items-center justify-between gap-3">
              <dt className="text-[#7c818c]">Deposit address</dt>
              <dd className="font-mono">{ADDRESS}</dd>
            </div>
            <div className="flex items-center justify-between gap-3">
              <dt className="text-[#7c818c]">Network</dt>
              <dd className="flex items-center gap-1 font-medium">
                <Monad className="size-3.5" />
                Monad · <Countdown />
              </dd>
            </div>
          </dl>
          <span className="mt-4 flex h-9 items-center justify-center gap-2 rounded-[6px] bg-[#f5a524] text-[11.5px] font-semibold text-[#0e0f12]">
            <Wallet className="size-3.5" />
            Pay from wallet
          </span>
          <p className="mt-3 flex items-center justify-center gap-1.5 text-[9.5px] text-[#7c818c]">
            <span className="size-1.5 rounded-full bg-gum-pink" />
            Secured by Gum
          </p>
        </div>
      </Stage>
    </div>
  );
}

/** One level of the book, with its depth drawn behind the numbers. */
function Level({
  price,
  size,
  depth,
  tone,
}: {
  price: string;
  size: string;
  depth: number;
  tone: "ask" | "bid";
}) {
  return (
    <div className="tabular relative flex items-center justify-between px-1 py-[1px]">
      <span
        className={cn(
          "absolute inset-y-0 left-0",
          tone === "ask" ? "bg-[#e5484d]/20" : "bg-[#30a46c]/20",
        )}
        style={{ width: `${depth * 100}%` }}
      />
      <span className={cn("relative", tone === "ask" ? "text-[#e5484d]" : "text-[#30a46c]")}>
        {price}
      </span>
      <span className="relative text-[#c5c8cf]">{size}</span>
    </div>
  );
}
