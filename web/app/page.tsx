import type { Metadata } from "next";
import Image from "next/image";
import Link from "next/link";
import type { CSSProperties, ReactNode } from "react";
import { DepositLedger } from "@/components/landing/deposit-ledger";
import { EmbedModes } from "@/components/landing/embed-modes";
import { GumMark } from "@/components/landing/gum-mark";
import { HeroScenes } from "@/components/landing/hero-scenes";
import { RESOURCES } from "@/components/landing/links";
import { ResourcesMenu } from "@/components/landing/nav";
import { Policies } from "@/components/landing/policies";
import { Reveal } from "@/components/landing/reveal";
import { Router } from "@/components/landing/router";
import { GetStarted } from "@/components/landing/signup-dialog";
import { MerchantAuth } from "@/components/merchant-auth";
import { PricingDialog } from "@/components/pricing-dialog";

export const metadata: Metadata = {
  title: "Gum — stablecoin deposits that stick.",
  description:
    "Gum mints a programmable address for every deposit: who can fund it, when it expires, where it settles. Your user pays from any wallet, exchange or chain. You get one webhook.",
  alternates: { canonical: "/" },
};

const SUPPORT_EMAIL = "support@gum.money";

/**
 * The landing page. One ruled column; the hero is a film at full width,
 * then four numbered chapters between two pink bands. The whole thing is
 * about one idea: an address that belongs to one deposit, so the deposit
 * sticks to whoever made it. A pink line down the left rule follows the
 * reader through it.
 */
export default function Home() {
  return (
    <MerchantAuth>
      <div className="landing flex min-h-screen flex-col bg-gum-white text-gum-black">
        <header className="landing-reveal border-b border-gum-grey/30">
          <nav className="mx-auto flex h-[72px] max-w-[1360px] items-center justify-between px-5 sm:h-[76px] sm:px-10">
            <Link href="/" aria-label="Gum" className="rounded-[4px]">
              <Image
                src="/gum/logo.png"
                width={1279}
                height={465}
                priority
                sizes="100px"
                alt="Gum"
                className="h-auto w-[86px] sm:w-[96px]"
              />
            </Link>

            <div className="flex items-center gap-6 text-[16px] text-gum-grey sm:gap-8">
              <PricingDialog appearance="light" />
              <Link href="/docs" className="transition-colors hover:text-gum-black">
                Docs
              </Link>
              <ResourcesMenu />
            </div>
          </nav>
        </header>

        <main className="relative mx-auto flex w-full max-w-[1360px] flex-1 flex-col border-gum-grey/30 xl:border-x">
          <span
            aria-hidden="true"
            className="landing-progress absolute top-0 bottom-0 left-[-1px] hidden w-[2px] bg-gum-pink xl:block"
          />

          {/* The hero */}
          <section className="border-b border-gum-grey/30 px-5 pt-12 pb-10 sm:px-10 lg:pt-20 lg:pb-14">
            <div className="grid gap-10 lg:grid-cols-[minmax(0,1fr)_340px] lg:items-end">
              <div>
                <p
                  className="landing-reveal text-[12px] font-medium tracking-[0.16em] text-gum-grey uppercase"
                  style={delay(0)}
                >
                  Stablecoin deposits for your app
                </p>
                <h1 className="mt-5 text-[clamp(46px,6.4vw,92px)] leading-[0.98] font-medium tracking-[-0.055em]">
                  <Words from={1}>Deposits that</Words>{" "}
                  <span className="landing-stick text-gum-pink" style={delay(4)}>
                    stick.
                  </span>
                </h1>
                <p
                  className="landing-reveal mt-8 max-w-[620px] text-[clamp(16px,1.3vw,19px)] leading-[1.55] text-gum-grey"
                  style={delay(6)}
                >
                  Gum mints an address for every deposit: who can fund it, when it expires, where it
                  settles. Your user pays from any wallet, exchange or chain. You get one webhook,
                  and a balance that&rsquo;s right.
                </p>
                <div
                  className="landing-reveal mt-9 flex flex-wrap items-center gap-3"
                  style={delay(7)}
                >
                  <GetStarted />
                  <Link href="/docs" className={secondary}>
                    <BookIcon />
                    Developers
                  </Link>
                </div>
              </div>
              <div className="landing-reveal hidden justify-self-end lg:block" style={delay(3)}>
                <GumMark tone="pink" fill="load" className="w-[300px]" />
              </div>
            </div>

            <div className="landing-reveal mt-12 lg:mt-16" style={delay(9)}>
              <HeroScenes />
            </div>
          </section>

          {/* The idea, in one breath */}
          <Reveal aria-label="One address per deposit" className="border-b border-gum-grey/30">
            <div
              data-reveal="wipe"
              className="bg-gum-pink px-5 py-12 text-gum-white sm:px-10 lg:py-16"
            >
              <p
                data-reveal=""
                style={step(1)}
                className="text-[clamp(30px,4.2vw,60px)] leading-[1.08] font-medium tracking-[-0.045em]"
              >
                One address per deposit.
                <br />
                One webhook when it lands.
              </p>
            </div>
          </Reveal>

          <Chapter
            number="01"
            id="route"
            title={
              <>
                In from anywhere.
                <br />
                Out to your chain.
              </>
            }
            body="Your user pays from the wallet or exchange they already use, on the chain their funds are on. Gum takes it at the deposit's own address and settles it to your wallet, on the chain you chose."
          >
            <Router />
          </Chapter>

          <Chapter
            number="02"
            id="program"
            title={
              <>
                Program the <span className="text-gum-pink">address</span>.
              </>
            }
            body="A deposit address is not a wallet. It is a set of terms: who may fund it, for how long, in what, and where the money goes. Set them per deposit, from your backend."
          >
            <Policies />
          </Chapter>

          <Chapter
            number="03"
            id="books"
            title={
              <>
                Every deposit,
                <br />
                on the books.
              </>
            }
            body="Because each deposit has its own address, every one of them is its own record: status, payer, chain, amount. Nothing to reconcile by hand, nothing to guess from a shared wallet."
          >
            <div data-reveal="scale" style={step(2)} className="mx-auto w-full max-w-[880px]">
              <DepositLedger />
            </div>
          </Chapter>

          <Chapter
            number="04"
            id="embed"
            title={
              <>
                Your checkout,
                <br />
                or ours.
              </>
            }
            body="Every deposit request comes with a hosted page. Send your user to it, embed it, or take the fields and build the flow yourself with the API and webhooks."
          >
            <div data-reveal="scale" style={step(2)} className="mx-auto w-full max-w-[880px]">
              <EmbedModes />
            </div>
          </Chapter>

          {/* The close */}
          <Reveal aria-labelledby="closing" className="overflow-hidden bg-gum-pink text-gum-white">
            <div className="grid items-center gap-10 px-5 py-16 sm:px-10 lg:grid-cols-[minmax(0,1fr)_300px] lg:py-24">
              <div>
                <h2
                  id="closing"
                  data-reveal=""
                  className="text-[clamp(34px,4.6vw,66px)] leading-[1.02] font-medium tracking-[-0.05em]"
                >
                  Free while we&rsquo;re in beta.
                </h2>
                <p
                  data-reveal=""
                  style={step(1)}
                  className="mt-6 max-w-[560px] text-[clamp(16px,1.3vw,19px)] leading-[1.55] text-gum-white/85"
                >
                  No fees, no card, no plans. When pricing comes, it will not disrupt anything you
                  have built.
                </p>
                <div
                  data-reveal=""
                  style={step(2)}
                  className="mt-9 flex flex-wrap items-center gap-3"
                >
                  <GetStarted label="Start for free" tone="white" />
                  <Link
                    href="/docs"
                    className="flex h-12 items-center justify-center gap-2.5 rounded-[6px] border border-gum-white px-5 text-[16px] font-medium text-gum-white transition-colors hover:bg-gum-white/10"
                  >
                    <BookIcon />
                    Developers
                  </Link>
                </div>
              </div>
              <div className="hidden justify-self-end lg:block">
                <GumMark tone="white" fill="reveal" className="w-[280px]" />
              </div>
            </div>
          </Reveal>
        </main>

        <footer className="bg-gum-black text-gum-white">
          <div className="mx-auto flex max-w-[1360px] flex-col gap-4 px-5 py-5 sm:h-[64px] sm:flex-row sm:items-center sm:justify-between sm:px-10 sm:py-0">
            <Image
              src="/gum/logo-on-dark.png"
              width={1279}
              height={465}
              sizes="90px"
              alt="Gum"
              className="h-auto w-[86px]"
            />

            <div className="flex items-center gap-4">
              <a
                href={`mailto:${SUPPORT_EMAIL}`}
                className="mr-2 text-[15px] text-gum-pink transition-colors hover:text-gum-white"
              >
                {SUPPORT_EMAIL}
              </a>
              <SocialLink label="X">
                <XIcon />
              </SocialLink>
              <SocialLink label="GitHub">
                <GitHubIcon />
              </SocialLink>
            </div>
          </div>
        </footer>
      </div>
    </MerchantAuth>
  );
}

const secondary =
  "flex h-12 items-center justify-center gap-2.5 rounded-[6px] border border-gum-black px-5 text-[16px] font-medium text-gum-black transition-colors hover:bg-gum-black/[0.05]";

/** The load-time stagger of one thing in the hero, in steps of 90ms. */
function delay(index: number): CSSProperties {
  return { animationDelay: `${index * 90}ms` };
}

/** The reveal order of one thing inside a group; see globals.css. */
function step(index: number): CSSProperties {
  return { "--i": index } as CSSProperties;
}

/** Each word of a headline arriving on its own beat. */
function Words({ from, children }: { from: number; children: string }) {
  return (
    <>
      {children.split(" ").map((word, index) => (
        <span key={index} className="landing-reveal inline-block" style={delay(from + index)}>
          {word}
          {index < children.split(" ").length - 1 ? " " : null}
        </span>
      ))}
    </>
  );
}

/**
 * A numbered chapter: its number in the gutter beside the rule, a title
 * and a sentence across the top, and the picture underneath at full width.
 */
function Chapter({
  number,
  id,
  title,
  body,
  children,
}: {
  number: string;
  id: string;
  title: ReactNode;
  body: string;
  children: ReactNode;
}) {
  return (
    <Reveal
      aria-labelledby={id}
      className="relative border-b border-gum-grey/30 px-5 py-14 sm:px-10 lg:py-20"
    >
      <span
        aria-hidden="true"
        className="absolute top-14 -left-[38px] hidden w-[30px] text-right font-mono text-[12px] text-gum-pink xl:block lg:top-20"
      >
        {number}
      </span>
      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)] lg:items-end lg:gap-16">
        <h2
          id={id}
          data-reveal=""
          className="text-[clamp(30px,3.4vw,48px)] leading-[1.06] font-medium tracking-[-0.045em]"
        >
          {title}
        </h2>
        <p
          data-reveal=""
          style={step(1)}
          className="max-w-[520px] text-[16px] leading-[1.6] text-gum-grey lg:justify-self-end"
        >
          {body}
        </p>
      </div>
      <div className="mt-10 lg:mt-14">{children}</div>
    </Reveal>
  );
}

function SocialLink({ label, children }: { label: string; children: ReactNode }) {
  const href = RESOURCES.find((entry) => entry.label === label)?.href;
  return (
    <a
      href={href}
      target="_blank"
      rel="noopener noreferrer"
      aria-label={label}
      className="flex size-8 items-center justify-center rounded-[6px] text-gum-white transition-colors hover:text-gum-pink [&>svg]:size-[18px]"
    >
      {children}
    </a>
  );
}

function BookIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" className="size-5" fill="none">
      <path
        d="M12 6.5c-1.6-1.4-3.6-2-6-2H4v13h2c2.4 0 4.4.6 6 2 1.6-1.4 3.6-2 6-2h2v-13h-2c-2.4 0-4.4.6-6 2Zm0 0v13"
        stroke="currentColor"
        strokeWidth="1.6"
        strokeLinejoin="round"
      />
    </svg>
  );
}

function XIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" fill="currentColor">
      <path d="M17.75 3h3.07l-6.7 7.66L22 21h-6.17l-4.84-6.32L5.46 21H2.39l7.17-8.2L2 3h6.33l4.37 5.78L17.75 3Zm-1.08 16.16h1.7L7.4 4.74H5.57l11.1 14.42Z" />
    </svg>
  );
}

function GitHubIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" fill="currentColor">
      <path d="M12 2a10 10 0 0 0-3.16 19.49c.5.09.68-.22.68-.48v-1.86c-2.78.6-3.37-1.18-3.37-1.18-.45-1.16-1.11-1.47-1.11-1.47-.91-.62.07-.61.07-.61 1 .07 1.53 1.03 1.53 1.03.9 1.53 2.35 1.09 2.92.83.09-.65.35-1.09.64-1.34-2.22-.25-4.56-1.11-4.56-4.94 0-1.09.39-1.98 1.03-2.68-.1-.25-.45-1.27.1-2.64 0 0 .84-.27 2.75 1.02A9.57 9.57 0 0 1 12 6.83c.85 0 1.69.11 2.49.34 1.91-1.29 2.75-1.02 2.75-1.02.55 1.37.2 2.39.1 2.64.64.7 1.03 1.59 1.03 2.68 0 3.84-2.34 4.68-4.57 4.93.36.31.68.92.68 1.86v2.75c0 .27.18.58.69.48A10 10 0 0 0 12 2Z" />
    </svg>
  );
}
