import type { Metadata } from "next";
import Image from "next/image";
import { Feather } from "lucide-react";
import { PaymentScene } from "@/components/landing/payment-scene";

export const metadata: Metadata = {
  title: "Payday — Make every stablecoin accountable",
  description:
    "Payday turns stablecoin transfers into verified customer deposits, ready for your application to credit.",
  alternates: { canonical: "/" },
};

export default function Home() {
  return (
    <div className="landing-page min-h-screen overflow-hidden bg-brand-black text-brand-white">
      <header className="landing-reveal border-b border-brand-grey/20">
        <nav className="mx-auto flex h-[76px] max-w-[1240px] items-center justify-between px-5 sm:px-8">
          <Image
            src="/payday-logo-full.svg"
            width={1600}
            height={400}
            priority
            alt="Payday"
            className="h-auto w-[126px] sm:w-[152px]"
          />

          <div className="flex items-center gap-6 text-[15px] text-brand-grey sm:gap-9 sm:text-[16px]">
            <a
              href="https://github.com/nkrishang/payday/tree/main/docs"
              className="transition-colors hover:text-brand-white"
            >
              Docs
            </a>
            <a href="#pricing" className="transition-colors hover:text-brand-white">
              Pricing
            </a>
          </div>
        </nav>
      </header>

      <main>
        <section className="landing-hero mx-auto flex min-h-[370px] max-w-[1120px] flex-col items-center px-5 pt-[clamp(48px,6vh,64px)] text-center sm:px-8">
          <h1 className="landing-reveal landing-delay-1 font-heading text-[clamp(38px,5vw,64px)] leading-[1.06] font-medium tracking-[-0.05em] text-balance">
            Make every stablecoin <span className="text-brand-yellow">accountable.</span>
          </h1>
          <p className="landing-reveal landing-delay-2 mt-6 max-w-[610px] text-[16px] leading-[1.7] text-brand-grey sm:text-[18px]">
            Payday turns stablecoin transfers into{" "}
            <span className="text-brand-green">verified customer deposits</span>,
            <br className="hidden sm:block" /> ready for your application to credit.
          </p>
          <div className="landing-reveal landing-delay-3 mt-9 flex w-full max-w-[410px] flex-col justify-center gap-3.5 min-[440px]:flex-row">
            <a
              href="https://github.com/nkrishang/payday"
              className="flex h-14 items-center justify-center gap-3 rounded-[8px] bg-brand-green px-5 text-[16px] font-medium text-brand-black transition-transform hover:-translate-y-0.5"
            >
              Start Building
              <ArrowRight />
            </a>
            <a
              href="mailto:contact@payday.sh?subject=Payday%20demo"
              className="flex h-14 items-center justify-center rounded-[8px] border border-brand-green px-5 text-[16px] font-medium text-brand-green transition-colors hover:bg-brand-green/10"
            >
              Request a demo
            </a>
          </div>
        </section>

        <section
          aria-label="Recent payments"
          className="landing-reveal landing-delay-4 payment-stage"
        >
          <PaymentScene />
        </section>
      </main>

      <footer className="border-t border-brand-grey/20 bg-brand-black">
        <div className="mx-auto flex max-w-[1240px] flex-col gap-4 px-5 py-3.5 sm:px-8 lg:flex-row lg:items-center lg:justify-between">
          <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
            <Image
              src="/payday-logo-full.svg"
              width={1600}
              height={400}
              alt="Payday"
              className="h-auto w-[88px]"
            />
            <p className="text-[11px] text-brand-grey">
              Make every stablecoin <span className="text-brand-yellow">accountable.</span>
            </p>
          </div>

          <div className="flex flex-wrap items-center gap-2">
            <a href="mailto:contact@payday.sh" className="mr-3 text-[12px] text-brand-green">
              contact@payday.sh
            </a>
            <SocialLink label="X">
              <XIcon />
            </SocialLink>
            <SocialLink label="LinkedIn">
              <LinkedInIcon />
            </SocialLink>
            <SocialLink label="Blog">
              <Feather />
            </SocialLink>
            <SocialLink label="GitHub">
              <GitHubIcon />
            </SocialLink>
            <SocialLink label="YouTube">
              <YouTubeIcon />
            </SocialLink>
          </div>
        </div>
      </footer>
    </div>
  );
}

function SocialLink({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <a
      href={label === "GitHub" ? "https://github.com/nkrishang/payday" : "#"}
      aria-label={label}
      className="flex size-10 items-center justify-center border border-brand-grey/25 text-brand-white transition-colors hover:border-brand-green hover:text-brand-green [&>svg]:size-[16px]"
    >
      {children}
    </a>
  );
}

function ArrowRight() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" className="size-5" fill="none">
      <path d="M5 12h14M14 7l5 5-5 5" stroke="currentColor" strokeWidth="1.8" />
    </svg>
  );
}

function XIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" fill="none">
      <path d="m5 5 14 14M19 5 5 19" stroke="currentColor" strokeWidth="1.5" />
    </svg>
  );
}

function LinkedInIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" fill="currentColor">
      <path d="M6.5 8.4H3.2V19h3.3V8.4ZM4.85 3A1.92 1.92 0 1 0 4.84 6.84 1.92 1.92 0 0 0 4.85 3ZM19 12.92c0-3.2-1.7-4.69-4-4.69a3.45 3.45 0 0 0-3.13 1.72V8.4H8.55V19h3.32v-5.25c0-1.38.26-2.72 1.98-2.72 1.7 0 1.72 1.59 1.72 2.81V19H19v-6.08Z" />
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

function YouTubeIcon() {
  return (
    <svg aria-hidden="true" viewBox="0 0 24 24" fill="none">
      <rect x="3" y="6" width="18" height="12" rx="3" stroke="currentColor" strokeWidth="1.6" />
      <path d="m10 9.5 5 2.5-5 2.5v-5Z" fill="currentColor" />
    </svg>
  );
}
