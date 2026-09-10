import { Check, Lock, Paperclip } from "lucide-react";
import Image from "next/image";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * Illustrations built from HTML rather than SVG: where the picture is a
 * table, a bar, or a piece of the product itself, the page's own elements
 * draw it better than paths would.
 */

/** Where the USDC goes in each situation, drawn as a bar against the requested amount. */
export function RoutingFigure() {
  const rows: Array<{
    title: string;
    received: number;
    late?: boolean;
    outcome: ReactNode;
  }> = [
    {
      title: "Exact amount, on time",
      received: 1,
      outcome: (
        <>
          <Out tone="green">amount → your wallet</Out>
        </>
      ),
    },
    {
      title: "Several partial transfers reach the amount",
      received: 1,
      outcome: (
        <>
          <Out tone="green">amount → your wallet</Out>
        </>
      ),
    },
    {
      title: "Still short when the deadline passes",
      received: 0.6,
      outcome: (
        <>
          <Out tone="yellow">balance → payer&apos;s wallet</Out>
        </>
      ),
    },
    {
      title: "More than the amount, on time",
      received: 1.3,
      outcome: (
        <>
          <Out tone="green">amount → your wallet</Out>
          <Out tone="yellow">remainder → payer&apos;s wallet</Out>
        </>
      ),
    },
    {
      title: "Enough arrived, but execution is after the deadline",
      received: 1,
      late: true,
      outcome: (
        <>
          <Out tone="yellow">balance → payer&apos;s wallet</Out>
        </>
      ),
    },
    {
      title: "USDC arrives after settlement",
      received: 0.4,
      late: true,
      outcome: (
        <>
          <Out tone="yellow">forwarded → payer&apos;s wallet</Out>
        </>
      ),
    },
  ];

  return (
    <div className="flex flex-col gap-4">
      <div className="hidden grid-cols-[minmax(0,1.2fr)_minmax(200px,1fr)_minmax(0,1.2fr)] gap-4 text-[11px] tracking-[0.06em] text-faint uppercase sm:grid">
        <span>Situation</span>
        <span>Received vs. amount</span>
        <span>Routing</span>
      </div>
      {rows.map((row) => (
        <div
          key={row.title}
          className="grid grid-cols-1 gap-2 border-t border-line pt-4 sm:grid-cols-[minmax(0,1.2fr)_minmax(200px,1fr)_minmax(0,1.2fr)] sm:gap-4"
        >
          <p className="text-[14px] leading-snug text-ink">{row.title}</p>
          <div className="relative h-6 pt-2">
            <div className="relative h-2 rounded-full bg-line">
              <div
                className={cn(
                  "absolute inset-y-0 left-0 rounded-full",
                  row.late ? "bg-brand-yellow/70" : "bg-brand-green",
                )}
                style={{ width: `${Math.min(row.received, 1) * 70}%` }}
              />
              {row.received > 1 ? (
                <div
                  className="absolute inset-y-0 rounded-r-full bg-brand-yellow/70"
                  style={{ left: "70%", width: `${(row.received - 1) * 70}%` }}
                />
              ) : null}
              <div
                aria-hidden="true"
                className="absolute -top-1.5 h-5 w-px bg-ink"
                style={{ left: "70%" }}
              />
              <span
                className="absolute -top-5 -translate-x-1/2 font-mono text-[10px] text-faint"
                style={{ left: "70%" }}
              >
                amount
              </span>
            </div>
          </div>
          <div className="flex flex-wrap gap-1.5">{row.outcome}</div>
        </div>
      ))}
    </div>
  );
}

function Out({ tone, children }: { tone: "green" | "yellow"; children: ReactNode }) {
  return (
    <span
      className={cn(
        "inline-flex h-6 items-center rounded-full border px-2.5 font-mono text-[11px] whitespace-nowrap",
        tone === "green"
          ? "border-brand-green/50 text-brand-green"
          : "border-brand-yellow/50 text-brand-yellow",
      )}
    >
      {children}
    </span>
  );
}

/** The webhook signature header, annotated part by part. */
export function SignatureFigure() {
  return (
    <div className="flex flex-col gap-4">
      <p className="overflow-x-auto font-mono text-[13px] whitespace-nowrap text-ink">
        <span className="text-faint">Payday-Signature: </span>
        <span className="rounded bg-brand-green/15 px-1 text-brand-green">v1</span>
        <span className="text-faint">,</span>
        <span className="rounded bg-brand-yellow/15 px-1 text-brand-yellow">t=1756728000</span>
        <span className="text-faint">,</span>
        <span className="rounded bg-term-id/15 px-1 text-term-id">sha256=6f1a…9c0e</span>
      </p>
      <dl className="grid grid-cols-1 gap-x-6 gap-y-2 text-[13.5px] sm:grid-cols-[max-content_1fr]">
        <dt className="font-mono text-brand-green">v1</dt>
        <dd className="text-muted">The scheme. Reject anything else.</dd>
        <dt className="font-mono text-brand-yellow">t</dt>
        <dd className="text-muted">
          Unix seconds when Payday signed the delivery. Reject a timestamp older than your tolerance
          (five minutes is usual).
        </dd>
        <dt className="font-mono text-term-id">sha256</dt>
        <dd className="text-muted">
          Hex HMAC-SHA256, keyed with the endpoint&apos;s secret, over the bytes{" "}
          <code>v1.&lt;t&gt;.&lt;raw body&gt;</code>. Compare in constant time.
        </dd>
      </dl>
    </div>
  );
}

/**
 * The hosted checkout, locked and unlocked, side by side. The same card the
 * landing page animates, held still so a reader can see what a gated
 * request withholds and what it never shows.
 */
export function CheckoutMock() {
  return (
    <div className="grid grid-cols-1 gap-4 md:grid-cols-2">
      <MockCard
        label="Gated request, before verification"
        locked
        issuer="Acme LLC"
        heading="March retainer"
      />
      <MockCard
        label="After verification and the wallet step"
        issuer="Acme LLC"
        heading="March retainer"
      />
    </div>
  );
}

function MockCard({
  label,
  issuer,
  heading,
  locked = false,
}: {
  label: string;
  issuer: string;
  heading: string;
  locked?: boolean;
}) {
  return (
    <div className="flex flex-col gap-2">
      <p className="text-[12px] tracking-[0.04em] text-faint uppercase">{label}</p>
      <div className="rounded-[14px] border border-brand-white/10 bg-[#111210] p-4 text-brand-white shadow-[0_18px_50px_rgb(0_0_0/28%)]">
        <div className="flex items-start justify-between gap-3">
          <div>
            <p className="text-[11px] text-[#94938d]">{issuer} requests</p>
            <p className="font-heading mt-0.5 text-[16px] font-medium">{heading}</p>
          </div>
          {locked ? (
            <span className="inline-flex items-center gap-1.5 rounded-full border border-brand-yellow/50 px-2 py-0.5 text-[11px] text-brand-yellow">
              <Lock className="size-3" />
              Verify to view
            </span>
          ) : (
            <span className="inline-flex items-center gap-1.5 rounded-full border border-[#31ae58]/60 px-2 py-0.5 text-[11px] text-[#31ae58]">
              <Check className="size-3" />
              Verified
            </span>
          )}
        </div>

        <div className="mt-4 grid grid-cols-[88px_1fr] gap-4">
          <div className="flex size-[88px] items-center justify-center overflow-hidden rounded-[8px] bg-[#f6f2ea]">
            {locked ? (
              <div className="size-full bg-[repeating-linear-gradient(45deg,#e4e0d8_0_6px,#f6f2ea_6px_12px)]" />
            ) : (
              <Image src="/payday-qr.svg" alt="" width={88} height={88} className="size-[88px]" />
            )}
          </div>
          <div className="flex flex-col gap-2 text-[12px]">
            <Row k="Amount" v={locked ? null : "25.00 USDC"} />
            <Row k="Network" v={locked ? null : "Base · native USDC"} />
            <Row k="Address" v={locked ? null : "0x8f3C…a1D2"} mono />
            <Row k="Deadline" v="in 23h 41m" />
            {locked ? null : (
              <p className="flex items-center gap-1.5 text-[#94938d]">
                <Paperclip className="size-3" />
                request.pdf
              </p>
            )}
          </div>
        </div>

        <div className="mt-4 grid h-11 grid-cols-[1fr_96px] overflow-hidden rounded-[8px] bg-[#ebe9e4] text-[#10110f]">
          <span className="flex items-center px-3 text-[15px] font-medium">
            {locked ? <Skeleton width={72} /> : "25.00 USDC"}
          </span>
          <span
            className={cn(
              "flex items-center justify-center text-[13px] font-semibold",
              locked ? "bg-[#8fa079] text-[#393d36]" : "bg-brand-green text-[#10110f]",
            )}
          >
            {locked ? "Locked" : "Pay now"}
          </span>
        </div>
      </div>
    </div>
  );
}

function Row({ k, v, mono = false }: { k: string; v: string | null; mono?: boolean }) {
  return (
    <p className="flex items-center justify-between gap-3">
      <span className="text-[#94938d]">{k}</span>
      {v === null ? <Skeleton width={90} /> : <span className={cn(mono && "font-mono")}>{v}</span>}
    </p>
  );
}

function Skeleton({ width }: { width: number }) {
  return (
    <span
      aria-label="withheld"
      className="inline-block h-3 rounded-full bg-[#3d3b3c]"
      style={{ width }}
    />
  );
}

/** What the payer page discloses, by policy and moment. */
export function DisclosureTable() {
  const columns = ["Always", "After verification", "After the wallet step"];
  const rows: Array<[string, boolean, boolean, boolean]> = [
    ["Issuer name and heading", true, true, true],
    ["Status, deadline, whether it is payable", true, true, true],
    ["Amount, remaining, the networks offered", false, true, true],
    ["Payer name, notes, reference, PDF", false, true, true],
    ["Chosen chain and token, one-time address, QR, wallet button", false, false, true],
  ];
  return (
    <table className="w-full text-[13.5px]">
      <thead>
        <tr className="text-left text-[11px] tracking-[0.06em] text-faint uppercase">
          <th className="pb-2 font-medium">Shown</th>
          {columns.map((column) => (
            <th key={column} className="pb-2 text-center font-medium">
              {column}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {rows.map(([label, ...cells]) => (
          <tr key={label} className="border-t border-line">
            <td className="py-2.5 pr-3 text-ink">{label}</td>
            {cells.map((shown, index) => (
              <td key={index} className="py-2.5 text-center">
                {shown ? (
                  <Check className="mx-auto size-4 text-brand-green" aria-label="shown" />
                ) : (
                  <span className="text-faint" aria-label="withheld">
                    —
                  </span>
                )}
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}

/** Never shown to a payer, whatever the policy. */
export function NeverShown() {
  const items = [
    "Your payout address",
    "The recovery address",
    "Metadata",
    "The customer record",
    "The expected email (only a masked hint)",
    "The payer reference",
  ];
  return (
    <ul className="grid grid-cols-1 gap-2 sm:grid-cols-2">
      {items.map((item) => (
        <li key={item} className="flex items-center gap-2 text-[13.5px] text-muted">
          <Lock className="size-3.5 shrink-0 text-faint" />
          {item}
        </li>
      ))}
    </ul>
  );
}
