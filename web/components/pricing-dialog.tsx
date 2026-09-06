"use client";

import * as Dialog from "@radix-ui/react-dialog";
import { X } from "lucide-react";
import Image from "next/image";

/**
 * "Pricing" has nowhere to go yet — no plans, no page. This opens in place
 * of a link, standing in until there's a real page to send it to. Shared by
 * the landing header (app/page.tsx) and the dashboard header
 * (dashboard/shell.tsx), which mirrors it.
 */
export function PricingDialog() {
  return (
    <Dialog.Root>
      <Dialog.Trigger className="transition-colors hover:text-brand-white">Pricing</Dialog.Trigger>

      <Dialog.Portal>
        <Dialog.Overlay className="landing-dialog-scrim fixed inset-0 z-50 bg-black/75 backdrop-blur-[3px]" />
        <Dialog.Content className="landing-dialog fixed top-1/2 left-1/2 z-50 w-[calc(100vw-28px)] max-w-[432px] -translate-x-1/2 -translate-y-1/2 rounded-[14px] border border-brand-grey/25 bg-[#070707] px-6 pt-6 pb-7 text-brand-white sm:px-7">
          <div className="flex items-start justify-between gap-4">
            <Image
              src="/payday-logo-full.svg"
              width={3200}
              height={1000}
              alt="Payday"
              className="mt-0.5 h-auto w-[84px]"
            />
            <Dialog.Close
              aria-label="Close"
              className="-mt-1.5 -mr-1.5 rounded-[6px] p-1.5 text-brand-grey transition-colors hover:bg-white/[0.06] hover:text-brand-white"
            >
              <X className="size-4" />
            </Dialog.Close>
          </div>

          <Dialog.Title className="font-heading mt-5 text-[27px] leading-[1.12] font-medium tracking-[-0.045em]">
            Free, for now<span className="text-brand-yellow">.</span>
          </Dialog.Title>
          <Dialog.Description className="mt-2.5 text-[14.5px] leading-[1.6] text-[#b0afa9]">
            Payday is in beta and completely free to use right now — no fees, no plans to pick.
          </Dialog.Description>

          <p className="mt-4 text-[14.5px] leading-[1.6] text-[#b0afa9]">
            {
              "When we do introduce pricing, we'll make sure it never disrupts a workflow you've already built on Payday."
            }
          </p>

          <p className="mt-6 border-t border-brand-grey/20 pt-4 text-[12px] leading-relaxed text-brand-grey">
            Questions in the meantime? Email{" "}
            <a
              href="mailto:contact@payday.sh"
              className="text-brand-green transition-colors hover:text-brand-green/80"
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
