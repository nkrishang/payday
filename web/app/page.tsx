import type { Metadata } from "next";
import Link from "next/link";
import type { CSSProperties, ReactNode } from "react";
import { GumMark } from "@/components/landing/gum-mark";
import { Policies } from "@/components/landing/policies";
import { Reveal } from "@/components/landing/reveal";
import { Router } from "@/components/landing/router";
import { GetStarted } from "@/components/landing/signup-dialog";
import { Surfaces } from "@/components/landing/surfaces";
import { MerchantAuth } from "@/components/merchant-auth";
import { SiteFooter } from "@/components/site-footer";
import { SiteHeader } from "@/components/site-header";

export const metadata: Metadata = {
  title: "Gum — stablecoin deposits that stick.",
  description:
    "Gum mints a programmable address for every deposit: who can fund it, when it expires, where it settles. Your user pays from any wallet, exchange or chain. You get one webhook.",
  alternates: { canonical: "/" },
};

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
        <SiteHeader reveal />

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

        <SiteFooter />
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
