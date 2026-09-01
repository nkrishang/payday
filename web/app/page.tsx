import type { Metadata } from "next";
import { Integrate } from "@/components/landing/integrate";
import { InstallLine } from "@/components/landing/install";
import { TerminalDemo } from "@/components/landing/terminal";
import { Logo } from "@/components/ui/logo";

export const metadata: Metadata = {
  title: "Payday — stablecoin payments that settle themselves",
  alternates: { canonical: "/" },
};

const DOCS = "https://github.com/nkrishang/payday/tree/main/docs";
const GITHUB = "https://github.com/nkrishang/payday";

const STEPS = [
  {
    n: "01",
    title: "Create",
    body: "One authenticated call returns a payment with its own address — computed before any contract exists, and committed to the amount, payout wallet, deadline, and refund wallet. Those terms cannot be changed afterwards, by anyone.",
  },
  {
    n: "02",
    title: "Share",
    body: "Every payment carries a link. It shows the amount due, the exact token and network, a QR, a one-time address, and a live status the payer can watch. Anyone holding the link can pay it.",
  },
  {
    n: "03",
    title: "Settle",
    body: "Payday indexes finalized USDC transfers, accumulates partial payments, then sweeps the balance to your payout wallet before the deadline, or to your refund wallet after it.",
  },
];

const FEATURES = [
  {
    title: "One address per payment",
    body: "Counterfactual and single-use. Payday computes it before deploying anything, so a payer can send immediately and no one — including Payday — can redirect the funds.",
  },
  {
    title: "Finality, not optimism",
    body: "Only finalized transfers are credited, and there is deliberately no privileged “mark paid” endpoint. Every read reports the block it is committed through.",
  },
  {
    title: "Partial payments accumulate",
    body: "Transfers add up across transactions and blocks. Overpayment is forwarded with the rest; nothing is silently dropped.",
  },
  {
    title: "Automatic recovery",
    body: "Funds that arrive late, fall short, or land after settlement route to your refund wallet instead of getting stuck at an address nobody controls.",
  },
  {
    title: "Signed webhooks",
    body: "HMAC-signed lifecycle events with test deliveries, a durable retry schedule, and the full attempt history for every endpoint.",
  },
  {
    title: "An auditable ledger",
    body: "Every observed transfer is kept with its sender, transaction, block, disposition, and whether it was collected — in PostgreSQL you can query.",
  },
];

const ROUTING = [
  ["Exact amount, on time", "Full balance to your payout wallet"],
  ["Several partial transfers reaching the amount", "They fund one payment; full balance to payout"],
  ["Still short at the deadline", "Full balance to your refund wallet"],
  ["More than requested, on time", "Payout receives the amount and the excess"],
  ["Execution after the deadline", "Full balance to your refund wallet"],
  ["USDC arriving after settlement", "Collected to your refund wallet"],
];

export default function Home() {
  return (
    <div className="bg-canvas">
      <header className="sticky top-0 z-40 border-b border-line bg-canvas/85 backdrop-blur-md">
        <nav className="mx-auto flex h-16 max-w-5xl items-center justify-between px-5 sm:px-8">
          <Logo className="h-[22px]" />
          <a
            href={GITHUB}
            className="rounded-md px-3 py-1.5 text-[13px] font-medium text-muted transition-colors hover:text-ink"
          >
            GitHub
          </a>
        </nav>
      </header>

      <main className="mx-auto max-w-5xl px-5 sm:px-8">
        <section className="grid gap-12 py-20 sm:py-28 lg:grid-cols-[minmax(0,1fr)_minmax(0,1.05fr)] lg:items-center lg:gap-16">
          <div>
            <h1 className="text-[38px] leading-[1.08] font-semibold tracking-[-0.02em] text-balance sm:text-[46px]">
              Minimal API to accept and catalog stablecoin payments.
            </h1>
            <p className="mt-5 max-w-[52ch] text-[16px] leading-relaxed text-muted">
              Create an invoice and get a unique, shareable payment link. Each payment is routed
              through a unique, non-custodial, one-time smart-address with a memo, programmed to
              deliver funds to the destination of your choice.
            </p>
            <div className="mt-8 max-w-[420px]">
              <InstallLine />
            </div>
          </div>
          <TerminalDemo />
        </section>

        <Section title="How it works" eyebrow="Three steps">
          <ol className="grid gap-px overflow-hidden rounded-[16px] border border-line bg-line sm:grid-cols-3">
            {STEPS.map((step) => (
              <li key={step.n} className="bg-surface p-6">
                <span className="tabular font-mono text-[12px] text-faint">{step.n}</span>
                <h3 className="mt-3 text-[15px] font-semibold tracking-tight">{step.title}</h3>
                <p className="mt-2 text-[14px] leading-relaxed text-muted">{step.body}</p>
              </li>
            ))}
          </ol>
        </Section>

        <Section title="Integrate in one call" eyebrow="API">
          <Integrate />
        </Section>

        <Section title="Where the money goes" eyebrow="Routing">
          <p className="mb-6 max-w-[62ch] text-[15px] leading-relaxed text-muted">
            Chain time decides every one of these outcomes — not a payer&rsquo;s device clock, and
            not the moment a wallet says the transaction was sent. The refund address is
            merchant-controlled exception handling, not an automatic return to the payer.
          </p>
          <dl className="overflow-hidden rounded-[16px] border border-line">
            {ROUTING.map(([situation, outcome], index) => (
              <div
                key={situation}
                className={`flex flex-col gap-1 bg-surface px-5 py-4 sm:flex-row sm:items-baseline sm:gap-6 ${
                  index > 0 ? "border-t border-line" : ""
                }`}
              >
                <dt className="text-[14px] font-medium sm:w-[46%] sm:shrink-0">{situation}</dt>
                <dd className="text-[14px] text-muted">{outcome}</dd>
              </div>
            ))}
          </dl>
        </Section>

        <Section title="Built to be boring about money" eyebrow="Guarantees">
          <ul className="grid gap-px overflow-hidden rounded-[16px] border border-line bg-line sm:grid-cols-2">
            {FEATURES.map((feature) => (
              <li key={feature.title} className="bg-surface p-6">
                <h3 className="text-[15px] font-semibold tracking-tight">{feature.title}</h3>
                <p className="mt-2 text-[14px] leading-relaxed text-muted">{feature.body}</p>
              </li>
            ))}
          </ul>
        </Section>

        <section className="border-t border-line py-20 text-center">
          <h2 className="text-[26px] font-semibold tracking-[-0.01em]">
            Take a payment in about a minute.
          </h2>
          <p className="mx-auto mt-3 max-w-[48ch] text-[15px] leading-relaxed text-muted">
            Install the CLI, create an invoice, and share the link. Payday credits only finalized
            transfers — there is no privileged endpoint that marks a payment paid.
          </p>
          <div className="mx-auto mt-8 max-w-[420px]">
            <InstallLine />
          </div>
        </section>
      </main>

      <footer className="border-t border-line">
        <div className="mx-auto flex max-w-5xl flex-col gap-4 px-5 py-8 sm:flex-row sm:items-center sm:justify-between sm:px-8">
          <Logo className="h-[18px] text-muted" />
          <div className="flex flex-wrap gap-x-5 gap-y-2 text-[13px] text-muted">
            <a href={`${DOCS}/concepts.md`} className="rounded transition-colors hover:text-ink">
              Concepts
            </a>
            <a
              href={`${DOCS}/api-reference.md`}
              className="rounded transition-colors hover:text-ink"
            >
              API
            </a>
            <a href="https://status.payday.sh" className="rounded transition-colors hover:text-ink">
              Status
            </a>
            <a href={GITHUB} className="rounded transition-colors hover:text-ink">
              GitHub
            </a>
            <a
              href="mailto:support@payday.sh"
              className="rounded transition-colors hover:text-ink"
            >
              Support
            </a>
          </div>
        </div>
      </footer>
    </div>
  );
}

function Section({
  title,
  eyebrow,
  children,
}: {
  title: string;
  eyebrow: string;
  children: React.ReactNode;
}) {
  return (
    <section className="border-t border-line py-16 sm:py-20">
      <p className="text-[11px] font-medium tracking-[0.14em] text-faint uppercase">{eyebrow}</p>
      <h2 className="mt-2.5 mb-8 text-[26px] font-semibold tracking-[-0.01em]">{title}</h2>
      {children}
    </section>
  );
}
