"use client";

import * as Dialog from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import Image from "next/image";
import type { ReactNode } from "react";
import { cn } from "@/lib/cn";

/**
 * "Pricing" has nowhere to go yet — no plans, no page. This opens in place
 * of a link, standing in until there's a real page to send it to. Shared by
 * the landing page, the dashboard header and the docs shell.
 *
 * The dialog is Gum's, whichever page opens it: it is portalled to the body
 * and carries the landing page's palette itself (`.landing-dialog`). Only
 * the trigger belongs to the page around it — `appearance` picks its hover
 * colour, and the trigger's words and styling are the caller's, since
 * "Pricing" in a header and "transparent pricing" mid-sentence are not the
 * same link.
 */
export function PricingDialog({
  appearance = "dark",
  trigger = "Pricing",
  triggerClassName,
}: {
  /** The ground the trigger sits on: the docs are dark, everything else light. */
  appearance?: "dark" | "light";
  trigger?: ReactNode;
  triggerClassName?: string;
}) {
  return (
    <Dialog.Root>
      <Dialog.Trigger
        className={cn(
          triggerClassName ??
            (appearance === "light"
              ? "transition-colors hover:text-gum-black"
              : "transition-colors hover:text-gum-white"),
        )}
      >
        {trigger}
      </Dialog.Trigger>

      <Dialog.Portal>
        <Dialog.Overlay className="landing-dialog-scrim fixed inset-0 z-50 bg-gum-black/40 backdrop-blur-[3px]" />
        <Dialog.Content className="landing-dialog fixed top-1/2 left-1/2 z-50 w-[calc(100vw-28px)] max-w-[432px] -translate-x-1/2 -translate-y-1/2 rounded-[14px] border border-gum-black bg-gum-white px-6 pt-6 pb-7 text-gum-black sm:px-7">
          <div className="flex items-start justify-between gap-4">
            <Image
              src="/gum/logo.svg"
              width={1280}
              height={465}
              alt="Gum"
              className="mt-0.5 h-auto w-[76px]"
            />
            <Dialog.Close
              aria-label="Close"
              className="-mt-1.5 -mr-1.5 rounded-[6px] p-1.5 text-gum-grey transition-colors hover:bg-gum-black/[0.06] hover:text-gum-black"
            >
              <X className="size-4" />
            </Dialog.Close>
          </div>

          <Dialog.Title className="font-heading mt-5 text-[27px] leading-[1.12] font-medium tracking-[-0.045em]">
            Free, for now.
          </Dialog.Title>
          <Dialog.Description className="mt-2.5 text-[14.5px] leading-[1.6] text-gum-grey">
            {"We're in beta and completely free to use right now. No fees, no card, no plans."}
          </Dialog.Description>

          <p className="mt-4 text-[14.5px] leading-[1.6] text-gum-grey">
            {
              "When we do introduce pricing, we'll make sure it never disrupts a workflow you've already built."
            }
          </p>

          <p className="mt-6 border-t border-gum-grey/40 pt-4 text-[12px] leading-relaxed text-gum-grey">
            Questions in the meantime? Email{" "}
            <a
              href="mailto:support@gum.money"
              className="font-medium text-gum-black transition-colors hover:text-gum-black/70"
            >
              support@gum.money
            </a>
            .
          </p>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
