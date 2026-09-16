import { Wallet } from "lucide-react";
import type { CSSProperties, ReactNode } from "react";
import { Countdown } from "./countdown";
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
        <Surface index={4} label="Embedded" note="in your app">
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

/** The same request as a dialog the merchant's app opens over its own page. */
function Embedded() {
  return (
    <div className="relative h-[420px] overflow-hidden rounded-[18px] bg-gum-black p-3">
      <div className="relative h-full overflow-hidden rounded-[12px] bg-gum-white text-gum-black">
        <div className="flex items-center gap-2.5 border-b border-gum-black/[0.08] px-4 py-2.5 text-[12px]">
          <span className="flex size-5 items-center justify-center rounded-[6px] bg-gum-black text-[9px] font-bold text-gum-white">
            A
          </span>
          <span className="font-semibold">Acme</span>
          <span className="ml-4 text-gum-grey">Dashboard</span>
          <span className="font-medium">Wallet</span>
          <span className="text-gum-grey">Settings</span>
          <span className="ml-auto flex size-6 items-center justify-center rounded-full bg-gum-pink text-[9px] font-semibold text-gum-white">
            JD
          </span>
        </div>
        <div className="px-4 pt-4">
          <p className="text-[10px] font-medium tracking-[0.14em] text-gum-grey uppercase">
            Balance
          </p>
          <p className="tabular mt-1 text-[24px] leading-none font-semibold tracking-tight">
            1,240.00
          </p>
          <span className="mt-4 flex h-8 w-[120px] items-center justify-center rounded-[8px] bg-gum-black text-[11px] font-medium text-gum-white">
            + Add funds
          </span>
        </div>

        <div className="absolute inset-0 bg-gum-black/35" />
        <div className="absolute top-1/2 left-1/2 w-[248px] -translate-x-1/2 -translate-y-1/2 rounded-[14px] bg-gum-white p-4 shadow-[0_30px_80px_-24px_rgb(0_0_0/0.5)]">
          <p className="text-[13px] font-semibold tracking-tight">Add funds</p>
          <p className="tabular mt-3 flex items-baseline gap-1.5 text-[26px] leading-none font-semibold tracking-tight">
            250.00
            <span className="flex items-center gap-1 text-[11px] font-medium text-gum-grey">
              <Usdc className="size-3.5" />
              USDC
            </span>
          </p>
          <dl className="mt-3 grid gap-1.5 text-[11px]">
            <div className="flex items-center justify-between gap-3">
              <dt className="text-gum-grey">Deposit address</dt>
              <dd className="font-mono">{ADDRESS}</dd>
            </div>
            <div className="flex items-center justify-between gap-3">
              <dt className="text-gum-grey">Network</dt>
              <dd className="flex items-center gap-1 font-medium">
                <Monad className="size-3.5" />
                Monad · <Countdown />
              </dd>
            </div>
          </dl>
          <span className="mt-4 flex h-9 items-center justify-center gap-2 rounded-[8px] bg-gum-black text-[12px] font-medium text-gum-white">
            <Wallet className="size-3.5" />
            Pay from wallet
          </span>
          <p className="mt-3 flex items-center justify-center gap-1.5 text-[10px] text-gum-grey">
            <span className="size-1.5 rounded-full bg-gum-pink" />
            Secured by Gum
          </p>
        </div>
      </div>
    </div>
  );
}
