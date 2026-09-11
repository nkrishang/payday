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
              ? "transition-colors hover:text-brand-black"
              : "transition-colors hover:text-brand-white"),
        )}
      >
        {trigger}
      </Dialog.Trigger>

      <Dialog.Portal>
        <Dialog.Overlay
          className={cn(
            "landing-dialog-scrim fixed inset-0 z-50 backdrop-blur-[3px]",
            light ? "bg-brand-black/40" : "bg-black/75",
          )}
        />
        <Dialog.Content
          className={cn(
            "landing-dialog fixed top-1/2 left-1/2 z-50 w-[calc(100vw-28px)] max-w-[432px] -translate-x-1/2 -translate-y-1/2 rounded-[14px] border px-6 pt-6 pb-7 sm:px-7",
            light
              ? "border-brand-black bg-brand-white text-brand-black"
              : "border-brand-grey/25 bg-[#070707] text-brand-white",
          )}
        >
          <div className="flex items-start justify-between gap-4">
            <Image
              src={light ? "/payday-logo-full-light.svg" : "/payday-logo-full.svg"}
              width={light ? 2800 : 2929}
              height={1000}
              alt="Payday"
              className="mt-0.5 h-auto w-[84px]"
            />
            <Dialog.Close
              aria-label="Close"
              className={cn(
                "-mt-1.5 -mr-1.5 rounded-[6px] p-1.5 transition-colors",
                light
                  ? "text-brand-subtle hover:bg-brand-black/[0.06] hover:text-brand-black"
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
              light ? "text-brand-subtle" : "text-[#b0afa9]",
            )}
          >
            Payday is in beta and completely free to use right now — no fees, no plans to pick.
          </Dialog.Description>

          <p
            className={cn(
              "mt-4 text-[14.5px] leading-[1.6]",
              light ? "text-brand-subtle" : "text-[#b0afa9]",
            )}
          >
            {
              "When we do introduce pricing, we'll make sure it never disrupts a workflow you've already built."
            }
          </p>

          <p
            className={cn(
              "mt-6 border-t pt-4 text-[12px] leading-relaxed",
              light
                ? "border-brand-black/15 text-brand-subtle"
                : "border-brand-grey/20 text-brand-grey",
            )}
          >
            Questions in the meantime? Email{" "}
            <a
              href="mailto:contact@payday.sh"
              className={cn(
                "transition-colors",
                light
                  ? "font-medium text-brand-black hover:text-brand-black/70"
                  : "text-brand-green hover:text-brand-green/80",
              )}
            >
              contact@payday.sh
            </a>
            .
          </p>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
