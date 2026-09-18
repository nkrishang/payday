import Image from "next/image";
import { GitHubIcon, XIcon } from "@/components/landing/icons";
import { RESOURCES } from "@/components/landing/links";

const SUPPORT = RESOURCES.find((entry) => entry.label === "Support")!;

/**
 * The site's one footer: a black bar with the wordmark, the support
 * address, and the two off-site marks. The landing page ends on it under
 * the pink close; the docs end on it under their dark reading room.
 */
export function SiteFooter() {
  return (
    <footer className="border-t border-gum-white/10 bg-gum-black text-gum-white">
      <div className="mx-auto flex h-[56px] max-w-[1360px] items-center justify-between gap-3 px-4 sm:h-[64px] sm:px-10">
        <Image
          src="/gum/logo-on-dark.svg"
          width={1280}
          height={465}
          sizes="90px"
          alt="Gum"
          className="h-auto w-[62px] shrink-0 sm:w-[86px]"
        />

        <div className="flex min-w-0 items-center gap-2 sm:gap-4">
          <a
            href={SUPPORT.href}
            className="truncate text-[13px] text-gum-pink transition-colors hover:text-gum-white sm:mr-2 sm:text-[15px]"
          >
            {SUPPORT.href.replace(/^mailto:/, "")}
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
  );
}

function SocialLink({ label, children }: { label: string; children: React.ReactNode }) {
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
