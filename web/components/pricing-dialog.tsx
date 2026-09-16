"use client";

import * as Dialog from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import Image from "next/image";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * "Pricing" has nowhere to go yet — no plans, no page. This opens in place
 * of a link, standing in until there's a real page to send it to. Shared by
 * the landing page (its header and its open source card), the docs shell,
 * and the dashboard header.
 *
 * The dashboard and docs are dark, the landing page is light, and the
 * dialog follows whichever page opens it: `appearance` picks the palette,
 * and the trigger's words and styling are the caller's, since "Pricing" in a
 * header and "transparent pricing" mid-sentence are not the same link.
 */
export function PricingDialog({
  appearance = "dark",
  trigger = "Pricing",
  triggerClassName,
}: {
  appearance?: "dark" | "light";
  trigger?: ReactNode;
  triggerClassName?: string;
}) {
  const light = appearance === "light";

  return (
    <Dialog.Root>
      <Dialog.Trigger
        className={cn(
          triggerClassName ??
            (light
              ? "transition-colors hover:text-gum-black"
              : "transition-colors hover:text-brand-white"),
        )}
      >
        {trigger}
      </Dialog.Trigger>

      <Dialog.Portal>
        <Dialog.Overlay
          className={cn(
            "landing-dialog-scrim fixed inset-0 z-50 backdrop-blur-[3px]",
            light ? "bg-gum-black/40" : "bg-black/75",
          )}
        />
        <Dialog.Content
          className={cn(
            "landing-dialog fixed top-1/2 left-1/2 z-50 w-[calc(100vw-28px)] max-w-[432px] -translate-x-1/2 -translate-y-1/2 rounded-[14px] border px-6 pt-6 pb-7 sm:px-7",
            light
              ? "border-gum-black bg-gum-white text-gum-black"
              : "border-brand-grey/25 bg-[#070707] text-brand-white",
          )}
        >
          <div className="flex items-start justify-between gap-4">
            <Image
              src={light ? "/gum/logo.png" : "/payday-logo-full.svg"}
              width={light ? 1279 : 2929}
              height={light ? 465 : 1000}
              alt={light ? "Gum" : "Payday"}
              className={light ? "mt-0.5 h-auto w-[76px]" : "mt-0.5 h-auto w-[84px]"}
            />
            <Dialog.Close
              aria-label="Close"
              className={cn(
                "-mt-1.5 -mr-1.5 rounded-[6px] p-1.5 transition-colors",
                light
                  ? "text-gum-grey hover:bg-gum-black/[0.06] hover:text-gum-black"
                  : "text-brand-grey hover:bg-white/[0.06] hover:text-brand-white",
              )}
            >
              <X className="size-4" />
            </Dialog.Close>
          </div>

          <Dialog.Title className="font-heading mt-5 text-[27px] leading-[1.12] font-medium tracking-[-0.045em]">
            Free, for now{light ? "." : <span className="text-brand-yellow">.</span>}
          </Dialog.Title>
          <Dialog.Description
            className={cn(
              "mt-2.5 text-[14.5px] leading-[1.6]",
              light ? "text-gum-grey" : "text-[#b0afa9]",
            )}
          >
            {"We're in beta and completely free to use right now. No fees, no card, no plans."}
          </Dialog.Description>

          <p
            className={cn(
              "mt-4 text-[14.5px] leading-[1.6]",
              light ? "text-gum-grey" : "text-[#b0afa9]",
            )}
          >
            {
              "When we do introduce pricing, we'll make sure it never disrupts a workflow you've already built."
            }
          </p>

          <p
            className={cn(
              "mt-6 border-t pt-4 text-[12px] leading-relaxed",
              light ? "border-gum-grey/40 text-gum-grey" : "border-brand-grey/20 text-brand-grey",
            )}
          >
            Questions in the meantime? Email{" "}
            <a
              href={light ? "mailto:support@gum.money" : "mailto:contact@payday.sh"}
              className={cn(
                "transition-colors",
                light
                  ? "font-medium text-gum-black hover:text-gum-black/70"
                  : "text-brand-green hover:text-brand-green/80",
              )}
            >
              {light ? "support@gum.money" : "contact@payday.sh"}
            </a>
            .
          </p>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
