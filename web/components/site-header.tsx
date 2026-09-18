"use client";

import { LogOut } from "lucide-react";
import Image from "next/image";
import Link from "next/link";
import type { ReactNode } from "react";
import { ResourcesMenu } from "@/components/landing/nav";
import { PricingDialog } from "@/components/pricing-dialog";
import { cn } from "@/lib/cn";

/**
 * The one header the site has: the wordmark on the left, Pricing, Docs and
 * Resources on the right, on a rule. The landing page, the dashboard and
 * the hosted checkout all render this same bar, so moving between them
 * never changes the chrome. A page with a session appends "Sign out" after
 * a rule; the checkout can slot a status word in the same place.
 */
export function SiteHeader({
  home = "/",
  tone = "light",
  reveal = false,
  collapse = false,
  signOut,
  trailing,
}: {
  /** Where the wordmark goes: the landing page, or the dashboard once signed in. */
  home?: string;
  /** The ground it sits on: Gum's white everywhere but the docs, which read dark. */
  tone?: "light" | "dark";
  /** The landing page's load animation. */
  reveal?: boolean;
  /**
   * Folds Pricing, Docs and Resources away on a phone, for a page whose
   * `trailing` carries a drawer toggle that offers them instead.
   */
  collapse?: boolean;
  /**
   * Shows "Sign out". Present but without a handler while the session is
   * still being restored, so the control stands in place, disabled, rather
   * than popping in once there is someone to sign out.
   */
  signOut?: { onClick?: (() => void) | undefined } | undefined;
  /** Something small after the menu — the checkout's "Reconnecting…". */
  trailing?: ReactNode;
}) {
  const dark = tone === "dark";
  return (
    <header
      className={cn(
        "relative z-40 border-b",
        dark ? "border-gum-white/10" : "border-gum-grey/30",
        reveal && "landing-reveal",
      )}
    >
      <nav className="mx-auto flex h-[60px] max-w-[1360px] items-center justify-between px-4 sm:h-[76px] sm:px-10">
        <Link href={home} aria-label="Gum" className="rounded-[4px]">
          <Image
            src={dark ? "/gum/logo-on-dark.svg" : "/gum/logo.svg"}
            width={1280}
            height={465}
            priority
            sizes="100px"
            alt="Gum"
            className="h-auto w-[70px] sm:w-[96px]"
          />
        </Link>

        <div
          className={cn(
            "flex items-center gap-4 text-[14px] whitespace-nowrap sm:gap-8 sm:text-[16px]",
            dark ? "text-gum-white/60" : "text-gum-grey",
          )}
        >
          <div className={cn("flex items-center gap-4 sm:gap-8", collapse && "hidden sm:flex")}>
            <PricingDialog appearance={tone} />
            <Link
              href="/docs"
              className={cn(
                "transition-colors",
                dark ? "hover:text-gum-white" : "hover:text-gum-black",
              )}
            >
              Docs
            </Link>
            <ResourcesMenu tone={tone} />
          </div>

          {signOut || trailing ? (
            <>
              <span
                aria-hidden="true"
                className={cn(
                  "h-5 w-px",
                  dark ? "bg-gum-white/15" : "bg-gum-grey/30",
                  collapse && "hidden sm:block",
                )}
              />
              {trailing}
              {signOut ? (
                // On a phone the wordmark, the three items and this label do
                // not fit on the rule together, so the icon carries it alone
                // there and the label is its accessible name.
                <button
                  type="button"
                  onClick={signOut.onClick}
                  disabled={!signOut.onClick}
                  aria-label="Sign out"
                  className={cn(
                    "inline-flex items-center gap-2 transition-colors disabled:pointer-events-none",
                    dark ? "hover:text-gum-white" : "hover:text-gum-black",
                  )}
                >
                  <LogOut aria-hidden="true" className="size-4" />
                  <span className="hidden sm:inline">Sign out</span>
                </button>
              ) : null}
            </>
          ) : null}
        </div>
      </nav>
    </header>
  );
}
