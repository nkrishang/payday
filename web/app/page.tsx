import type { Metadata } from "next";
import Image from "next/image";

export const metadata: Metadata = {
  title: "Payday — Make every stablecoin accountable",
  description:
    "Payday turns stablecoin transfers into verified customer deposits, ready for your application to credit.",
  alternates: { canonical: "/" },
};

const FEATURES = ["Non-custodial settlement", "Custom Verification Policy", "Built-in Attribution"];

export default function Home() {
  return (
    <div className="landing-page min-h-screen bg-brand-black text-brand-white">
      <header className="border-b border-brand-grey/25">
        <nav className="mx-auto flex h-[94px] max-w-[1360px] items-center justify-between px-6 sm:px-10 lg:px-14">
          <Image
            src="/payday-logo-full.svg"
            width={1600}
            height={400}
            priority
            alt="Payday"
            className="h-auto w-[174px] sm:w-[190px]"
          />

          <div className="flex items-center gap-5 sm:gap-9">
            <button
              type="button"
              className="hidden text-[18px] text-brand-grey underline underline-offset-4 sm:block"
            >
              Docs
            </button>
            <button
              type="button"
              className="hidden text-[18px] text-brand-grey underline underline-offset-4 sm:block"
            >
              Pricing
            </button>
            <button
              type="button"
              className="flex items-center gap-2 rounded-[8px] border border-brand-green px-4 py-3 text-[15px] font-medium text-brand-green sm:px-5 sm:text-[17px]"
            >
              Request a demo
              <ArrowUpRight />
            </button>
          </div>
        </nav>
      </header>

      <main>
        <section className="mx-auto grid max-w-[1360px] gap-12 px-6 py-16 sm:px-10 sm:py-24 lg:grid-cols-[0.95fr_1fr] lg:items-center lg:gap-6 lg:px-14 lg:pt-28 lg:pb-4">
          <div className="relative z-10">
            <h1 className="font-heading max-w-[650px] text-[44px] leading-[1.06] font-medium tracking-[-0.045em] sm:text-[60px]">
              Make every stablecoin
              <span className="block text-brand-yellow">
                accountable<span className="text-[1.14em] leading-none">.</span>
              </span>
            </h1>
            <p className="mt-8 max-w-[570px] text-[17px] leading-[1.7] text-brand-grey sm:text-[18px]">
              Payday turns stablecoin transfers into{" "}
              <span className="text-brand-green">verified customer deposits</span>, ready for your
              application to credit.
            </p>
            <button
              type="button"
              className="mt-10 flex items-center gap-3 rounded-[8px] bg-brand-green px-5 py-4 text-[17px] font-medium text-brand-black sm:text-[18px]"
            >
              Start Building
              <ArrowRight />
            </button>
          </div>

          <div
            role="img"
            aria-label="Hero animation placeholder"
            className="flex min-h-[390px] items-center justify-center rounded-[15px] border border-brand-grey/30 text-[14px] text-brand-grey sm:aspect-[25/18] sm:min-h-0 lg:max-w-[600px]"
          >
            Hero animation placeholder
          </div>
        </section>

        <section className="bg-brand-yellow px-6 py-5 text-brand-black sm:px-10 lg:px-14">
          <div className="mx-auto grid max-w-[1200px] gap-6 md:grid-cols-3 lg:gap-[clamp(40px,8.3vw,120px)]">
            {FEATURES.map((feature) => (
              <article
                key={feature}
                className="flex min-h-[240px] flex-col rounded-[15px] bg-brand-grey p-6 text-center"
              >
                <h2 className="font-heading text-[20px] font-semibold tracking-[-0.025em] sm:text-[21px]">
                  {feature}
                </h2>
                <div className="flex flex-1 items-center justify-center">
                  <p className="text-[18px] leading-[1.45]">
                    Still graphic.
                    <br />
                    Hover for effect
                    <br />
                    Click to flip
                  </p>
                </div>
              </article>
            ))}
          </div>
        </section>
      </main>

      <footer className="border-t border-brand-grey/25">
        <div className="mx-auto flex max-w-[1240px] flex-col gap-7 px-6 py-8 sm:px-10 lg:flex-row lg:items-center lg:justify-between xl:px-0">
          <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
            <Image
              src="/payday-logo-full.svg"
              width={1600}
              height={400}
              alt="Payday"
              className="h-auto w-[112px]"
            />
            <p className="text-[12px] text-brand-grey">
              Make every stablecoin <span className="text-brand-yellow">accountable.</span>
            </p>
          </div>

          <div className="flex flex-wrap items-center gap-4">
            <a href="mailto:contact@payday.sh" className="mr-1 text-[13px] text-brand-green">
              contact@payday.sh
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

function SocialLink({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <button
      type="button"
      aria-label={label}
      className="flex size-10 items-center justify-center border border-brand-grey/30 text-brand-white"
    >
      {children}
    </button>
  );
}

function ArrowRight() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" className="size-5" fill="none">
      <path d="M5 12h14M14 7l5 5-5 5" stroke="currentColor" strokeWidth="1.8" />
    </svg>
  );
}

function ArrowUpRight() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" className="size-5" fill="none">
      <path d="M7 17 17 7M8 7h9v9" stroke="currentColor" strokeWidth="1.8" />
    </svg>
  );
}

function XIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" className="size-[17px]" fill="currentColor">
      <path d="M18.9 2H22l-6.77 7.74L23.2 22h-6.24l-4.89-6.39L6.48 22H3.36l7.26-8.3L2.98 2h6.4l4.42 5.84L18.9 2Zm-1.1 17.84h1.72L8.44 4.05H6.6L17.8 19.84Z" />
    </svg>
  );
}

function GitHubIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" className="size-[18px]" fill="currentColor">
      <path d="M12 2a10 10 0 0 0-3.16 19.49c.5.09.68-.22.68-.48v-1.86c-2.78.6-3.37-1.18-3.37-1.18-.45-1.16-1.11-1.47-1.11-1.47-.91-.62.07-.61.07-.61 1 .07 1.53 1.03 1.53 1.03.9 1.53 2.35 1.09 2.92.83.09-.65.35-1.09.64-1.34-2.22-.25-4.56-1.11-4.56-4.94 0-1.09.39-1.98 1.03-2.68-.1-.25-.45-1.27.1-2.64 0 0 .84-.27 2.75 1.02A9.57 9.57 0 0 1 12 6.83c.85 0 1.69.11 2.49.34 1.91-1.29 2.75-1.02 2.75-1.02.55 1.37.2 2.39.1 2.64.64.7 1.03 1.59 1.03 2.68 0 3.84-2.34 4.68-4.57 4.93.36.31.68.92.68 1.86v2.75c0 .27.18.58.69.48A10 10 0 0 0 12 2Z" />
    </svg>
  );
}
