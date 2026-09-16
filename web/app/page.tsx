import type { Metadata } from "next";
import Image from "next/image";
import Link from "next/link";
import type { CSSProperties, ReactNode } from "react";
import { ChainGrid } from "@/components/landing/chain-grid";
import { CheckoutScene } from "@/components/landing/checkout-card";
import { DepositLedger } from "@/components/landing/deposit-ledger";
import { EmbedModes } from "@/components/landing/embed-modes";
import { HeroScenes } from "@/components/landing/hero-scenes";
import { RESOURCES } from "@/components/landing/links";
import { ResourcesMenu } from "@/components/landing/nav";
import { Reveal } from "@/components/landing/reveal";
import { GetStarted } from "@/components/landing/signup-dialog";
import { MerchantAuth } from "@/components/merchant-auth";
import { PricingDialog } from "@/components/pricing-dialog";

export const metadata: Metadata = {
  title: "Gum — accept stablecoin deposits into your app.",
  description:
    "Create a unique programmable address for every deposit. Control who can fund it, when it expires and where it settles.",
  alternates: { canonical: "/" },
};

const SUPPORT_EMAIL = "support@gum.money";

/**
 * The landing page: a hero, then four beats that alternate between a pink
 * band and a section of the page's own ground, inside one column with a
 * rule down each side. Everything below the hero arrives as it is scrolled
 * to (`Reveal`); the hero arrives with the page.
 */
export default function Home() {
  return (
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

      <main className="mx-auto flex w-full max-w-[1360px] flex-1 flex-col border-gum-grey/30 xl:border-x">
        {/* The hero */}
        <section className="grid items-center gap-12 border-b border-gum-grey/30 px-5 pt-12 pb-14 sm:px-10 lg:grid-cols-[minmax(0,1fr)_minmax(0,600px)] lg:gap-10 lg:py-20">
          <div>
            <h1 className="landing-reveal text-[clamp(36px,3.6vw,52px)] leading-[1.12] font-medium tracking-[-0.055em]">
              Accept stablecoin deposits
              <br className="hidden lg:block" /> into your app.
            </h1>
            <p className="landing-reveal landing-delay-1 mt-6 max-w-[600px] text-[clamp(16px,1.3vw,19px)] leading-[1.55] text-gum-grey">
              Create a <Pink>unique programmable address</Pink> for <Pink>every deposit</Pink>.
              Control who can fund it, when it expires and where it settles.
            </p>
            <div className="landing-reveal landing-delay-2 mt-10 flex flex-wrap items-center gap-3">
              <MerchantAuth>
                <GetStarted />
              </MerchantAuth>
              <Link
                href="/docs"
                className="flex h-12 items-center justify-center gap-2.5 rounded-[6px] border border-gum-black px-5 text-[16px] font-medium text-gum-black transition-colors hover:bg-gum-black/[0.05]"
              >
                <BookIcon />
                Developers
              </Link>
            </div>
          </div>

          <div className="landing-reveal landing-delay-3">
            <HeroScenes />
          </div>
        </section>

        {/* Any source in, your chain out */}
        <Reveal
          aria-label="Chains"
          className="grid border-b border-gum-grey/30 lg:grid-cols-[minmax(0,47fr)_minmax(0,53fr)]"
        >
          <div
            data-reveal="wipe"
            className="bg-gum-pink px-5 py-9 text-gum-white sm:px-12 sm:py-11"
          >
            <p
              data-reveal=""
              style={step(1)}
              className="text-[clamp(24px,2.2vw,30px)] leading-[1.35] font-medium tracking-[-0.02em]"
            >
              Users pay from any source.
              <br />
              Settle funds where you want.
            </p>
          </div>
          <div className="min-w-0 border-t border-gum-grey/30 lg:border-t-0 lg:border-l">
            <ChainGrid />
          </div>
        </Reveal>

        {/* A unique address for every deposit */}
        <Reveal
          aria-labelledby="unique-address"
          className="grid items-center gap-10 border-b border-gum-grey/30 px-5 py-14 sm:px-10 lg:grid-cols-[minmax(0,1fr)_minmax(0,600px)] lg:gap-10 lg:py-16"
        >
          <div>
            <h2 id="unique-address" data-reveal="" className={sectionTitle}>
              A unique address for every
              <br />
              <s className="decoration-gum-pink decoration-[3px]">user</s>{" "}
              <span className="text-gum-pink">deposit</span>.
            </h2>
            <p data-reveal="" style={step(1)} className={sectionBody}>
              Get a dedicated flow for each deposit. Track its status, link it to a user, and route
              funds where they need to go.
            </p>
          </div>
          <div data-reveal="scale" style={step(2)}>
            <DepositLedger />
          </div>
        </Reveal>

        {/* Bring your own checkout */}
        <Reveal aria-label="Checkouts" className="border-b border-gum-grey/30">
          <div data-reveal="wipe" className="bg-gum-pink text-gum-white">
            <div className="grid items-stretch gap-8 px-5 py-8 sm:px-8 lg:grid-cols-[minmax(0,1fr)_350px_auto] lg:gap-0 lg:py-0 lg:pr-0">
              <p
                data-reveal=""
                style={step(1)}
                className="self-center text-[clamp(22px,1.8vw,26px)] leading-[1.35] font-medium tracking-[-0.02em] lg:py-11 lg:pr-10"
              >
                Bring your own checkout.
                <br />
                Gum works with any payment solution.
              </p>
              <CheckoutScene />
            </div>
          </div>
        </Reveal>

        {/* Hosted or headless */}
        <Reveal
          aria-labelledby="embed"
          className="grid items-center gap-10 px-5 py-14 sm:px-10 lg:grid-cols-[minmax(0,1fr)_minmax(0,600px)] lg:gap-10 lg:py-16"
        >
          <div>
            <h2 id="embed" data-reveal="" className={sectionTitle}>
              Embed right into your app.
              <br />
              Hosted or{" "}
              <span className="underline decoration-gum-pink decoration-[3px] underline-offset-[7px]">
                headless
              </span>
              .
            </h2>
            <p data-reveal="" style={step(1)} className={sectionBody}>
              Every deposit request comes with a hosted payment page. Redirect users, embed it in
              your app, or build the full flow yourself with our API and webhooks.
            </p>
          </div>
          <div data-reveal="scale" style={step(2)}>
            <EmbedModes />
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
  );
}

const sectionTitle = "text-[clamp(28px,2.5vw,34px)] leading-[1.25] font-medium tracking-[-0.03em]";
const sectionBody = "mt-5 max-w-[540px] text-[17px] leading-[1.6] text-gum-grey";

/** The reveal order of one thing inside a group; see globals.css. */
function step(index: number): CSSProperties {
  return { "--i": index } as CSSProperties;
}

/** A run of the subhead in the brand's colour. */
function Pink({ children }: { children: ReactNode }) {
  return <span className="text-gum-pink">{children}</span>;
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
