import type { Metadata } from "next";
import Image from "next/image";
import Link from "next/link";
import { GetStarted } from "@/components/landing/signup-dialog";
import { MerchantAuth } from "@/components/merchant-auth";

export const metadata: Metadata = {
  title: "Payday — accept stablecoins on your terms.",
  description:
    "Create a one-time programmable address for every deposit. Control who can fund it, when it expires and where it settles.",
  alternates: { canonical: "/" },
};

export default function Home() {
  return (
    <div className="landing flex min-h-screen flex-col bg-brand-white text-brand-black">
      <header className="landing-reveal">
        <nav className="mx-auto flex h-[88px] max-w-[1320px] items-center px-5 sm:px-8">
          <Link href="/" aria-label="Payday" className="rounded-[4px]">
            <Image
              src="/payday-logo-full-light.svg"
              width={2800}
              height={1000}
              priority
              sizes="100px"
              alt="Payday"
              className="h-auto w-[88px] sm:w-[100px]"
            />
          </Link>
        </nav>
      </header>

      <main className="flex flex-1 flex-col">
        <section className="mx-auto grid w-full max-w-[1320px] flex-1 items-center gap-12 px-5 pt-8 pb-16 sm:px-8 lg:grid-cols-[minmax(0,1.08fr)_minmax(0,1fr)] lg:gap-16 lg:pt-0 lg:pb-[clamp(48px,12vh,180px)]">
          <div>
            <h1 className="landing-reveal landing-delay-1 text-[clamp(40px,4.15vw,78px)] leading-[1.12] font-medium tracking-[-0.045em]">
              Accept <Highlight tone="yellow">stablecoins</Highlight> on
              <br className="hidden lg:block" /> <Highlight tone="green">your terms.</Highlight>
            </h1>
            <p className="landing-reveal landing-delay-2 mt-7 max-w-[600px] text-[18px] leading-[1.55] text-brand-subtle sm:text-[22px]">
              Create a{" "}
              <strong className="font-medium text-brand-black">
                one-time programmable address
              </strong>{" "}
              for every deposit. Control who can fund it, when it expires and where it settles.
            </p>
            <div className="landing-reveal landing-delay-3 mt-9 flex flex-wrap items-center gap-3.5">
              <MerchantAuth>
                <GetStarted />
              </MerchantAuth>
              <Link
                href="/docs"
                className="flex h-14 items-center justify-center gap-2.5 rounded-[8px] border border-brand-black px-6 text-[17px] font-medium text-brand-black transition-colors hover:bg-brand-black/[0.05]"
              >
                <BookIcon />
                Read Docs
              </Link>
            </div>
          </div>

          <div className="landing-reveal landing-delay-4 flex aspect-[774/424] w-full items-center justify-center rounded-[10px] border border-brand-black bg-white text-[clamp(18px,1.6vw,24px)] font-medium">
            Create deposit request
          </div>
        </section>
      </main>

      <footer className="landing-reveal landing-delay-4">
        <div className="mx-auto flex max-w-[1320px] flex-col gap-5 px-5 pb-8 sm:px-8 lg:flex-row lg:items-center lg:justify-between">
          <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
            <Image
              src="/payday-logo-full-light.svg"
              width={2800}
              height={1000}
              sizes="72px"
              alt="Payday"
              className="h-auto w-[72px]"
            />
            <p className="text-[15px] font-medium">Accept stablecoins on your terms.</p>
          </div>

          <div className="flex flex-wrap items-center gap-3">
            <a
              href="mailto:contact@payday.sh"
              className="mr-2 text-[15px] text-brand-subtle transition-colors hover:text-brand-black"
            >
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

/** A marker-pen block behind a run of the headline. */
function Highlight({ tone, children }: { tone: "yellow" | "green"; children: React.ReactNode }) {
  return (
    <mark
      className={tone === "yellow" ? "landing-mark bg-brand-yellow" : "landing-mark bg-brand-green"}
    >
      {children}
    </mark>
  );
}

/** Where the footer's social icons go. */
const SOCIAL_LINKS: Record<string, string> = {
  X: "https://x.com/paydaydotsh",
  GitHub: "https://github.com/nkrishang/payday",
};

function SocialLink({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <a
      href={SOCIAL_LINKS[label]}
      aria-label={label}
      className="flex size-8 items-center justify-center rounded-[6px] bg-brand-black text-brand-white transition-colors hover:bg-brand-black/85 [&>svg]:size-[16px]"
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
