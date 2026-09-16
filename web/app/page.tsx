import type { Metadata } from "next";
import Image from "next/image";
import Link from "next/link";
import type { CSSProperties, ReactNode } from "react";
import { GumMark } from "@/components/landing/gum-mark";
import { RESOURCES } from "@/components/landing/links";
import { ResourcesMenu } from "@/components/landing/nav";
import { Policies } from "@/components/landing/policies";
import { Reveal } from "@/components/landing/reveal";
import { Router } from "@/components/landing/router";
import { GetStarted } from "@/components/landing/signup-dialog";
import { Surfaces } from "@/components/landing/surfaces";
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
 * The landing page. One ruled column; the hero is the route a deposit
 * takes, under the headline, then two numbered chapters, then a pink close. The whole thing is
 * about one idea: an address that belongs to one deposit, so the deposit
 * sticks to whoever made it. A pink line down the left rule follows the
 * reader through it.
 */
export default function Home() {
  return (
    <MerchantAuth>
      <div className="landing flex min-h-screen flex-col bg-gum-white text-gum-black">
        <header className="landing-reveal border-b border-gum-grey/30">
          <nav className="mx-auto flex h-[60px] max-w-[1360px] items-center justify-between px-4 sm:h-[76px] sm:px-10">
            <Link href="/" aria-label="Gum" className="rounded-[4px]">
              <Image
                src="/gum/logo.png"
                width={1279}
                height={465}
                priority
                sizes="100px"
                alt="Gum"
                className="h-auto w-[70px] sm:w-[96px]"
              />
            </Link>

            <div className="flex items-center gap-4 text-[14px] text-gum-grey sm:gap-8 sm:text-[16px]">
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
          <section className="border-b border-gum-grey/30 px-4 pt-10 pb-10 sm:px-10 lg:pt-20 lg:pb-16">
            <div className="mx-auto flex max-w-[820px] flex-col items-center text-center">
              <p
                className="landing-reveal text-[10.5px] font-medium tracking-[0.14em] text-gum-grey uppercase sm:text-[12px] sm:tracking-[0.16em]"
                style={delay(0)}
              >
                Stablecoin deposits for your app
              </p>
              <h1 className="mt-4 text-[clamp(32px,8.8vw,88px)] leading-[0.98] font-medium tracking-[-0.055em] whitespace-nowrap sm:mt-5">
                <Words from={1}>Deposits that</Words>{" "}
                <span className="landing-stick" style={delay(4)}>
                  <span className="text-gum-pink">stick</span>.
                </span>
              </h1>
              <p
                className="landing-reveal mt-5 max-w-[680px] text-[15px] leading-[1.5] text-pretty text-gum-grey sm:mt-7 sm:text-[clamp(16px,1.35vw,20px)]"
                style={delay(6)}
              >
                Create a <Pink>unique programmable address</Pink> for <Pink>every deposit</Pink>.
                Control who can fund it and where it settles. Let your users pay from any source.
              </p>
              <div
                className="landing-reveal mt-7 flex items-center justify-center gap-2.5 sm:mt-8 sm:gap-3"
                style={delay(7)}
              >
                <GetStarted />
                <Link href="/docs" className={secondary}>
                  <BookIcon />
                  Developers
                </Link>
              </div>
            </div>

            <Reveal
              as="div"
              className="landing-reveal mt-14 lg:mt-20"
              aria-label="The route a deposit takes"
            >
              <Router />
            </Reveal>
          </section>

          <Chapter
            number="01"
            id="program"
            title={
              <>
                A unique address per <s className="decoration-gum-pink decoration-[3px]">user</s>{" "}
                <span className="text-gum-pink">deposit</span>.
              </>
            }
            body="Only credit incoming deposits that satisfy your app's payment policy. Every single deposit is configurable."
          >
            <Policies />
          </Chapter>

          <Chapter
            number="02"
            id="surfaces"
            title="Embed in your app."
            body="Every deposit request comes with a hosted payment page. Redirect users, embed it in your app, or build the full flow yourself with our API and webhooks."
          >
            <Surfaces />
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
                  We&rsquo;re in beta and completely free to use right now. No fees, no card, no
                  plans. When we do introduce pricing, we&rsquo;ll make sure it never disrupts a
                  workflow you&rsquo;ve already built.
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
          <div className="mx-auto flex h-[56px] max-w-[1360px] items-center justify-between gap-3 px-4 sm:h-[64px] sm:px-10">
            <Image
              src="/gum/logo-on-dark.png"
              width={1279}
              height={465}
              sizes="90px"
              alt="Gum"
              className="h-auto w-[62px] shrink-0 sm:w-[86px]"
            />

            <div className="flex min-w-0 items-center gap-2 sm:gap-4">
              <a
                href={`mailto:${SUPPORT_EMAIL}`}
                className="truncate text-[13px] text-gum-pink transition-colors hover:text-gum-white sm:mr-2 sm:text-[15px]"
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
  "flex h-11 items-center justify-center gap-2 rounded-[6px] border border-gum-black px-4 text-[15px] font-medium whitespace-nowrap text-gum-black transition-colors hover:bg-gum-black/[0.05] sm:h-12 sm:gap-2.5 sm:px-5 sm:text-[16px]";

/** The load-time stagger of one thing in the hero, in steps of 90ms. */
function delay(index: number): CSSProperties {
  return { animationDelay: `${index * 90}ms` };
}

/** The reveal order of one thing inside a group; see globals.css. */
function step(index: number): CSSProperties {
  return { "--i": index } as CSSProperties;
}

/** A run of the subhead in the brand's colour. */
function Pink({ children }: { children: ReactNode }) {
  return <span className="text-gum-pink">{children}</span>;
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
      <div className="grid gap-6 lg:grid-cols-[minmax(0,1fr)_minmax(0,1fr)] lg:items-start lg:gap-16">
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
      className="flex size-7 shrink-0 items-center justify-center rounded-[6px] text-gum-white transition-colors hover:text-gum-pink sm:size-8 [&>svg]:size-[16px] sm:[&>svg]:size-[18px]"
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
