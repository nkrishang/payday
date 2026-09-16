import Image from "next/image";
import { Countdown } from "./countdown";
import { PayWith } from "./pay-with";

/**
 * A deposit as a checkout that could be anyone's shows it: the amount, the
 * one-time address it settles to, a clock, and a pay button whose provider
 * keeps changing. Gum is none of the chrome; it is the address.
 */
export function CheckoutCard({ index, leaving }: { index: number; leaving: boolean }) {
  return (
    <div className="flex flex-col bg-gum-white p-4 text-gum-black">
      <div className="flex items-baseline justify-between gap-3">
        <p className="text-[11px] font-medium tracking-[0.14em] text-gum-grey uppercase">Deposit</p>
        <p className="text-[11px] text-gum-grey">
          Expires in <Countdown />
        </p>
      </div>
      <p className="tabular mt-1 flex items-baseline gap-2 text-[26px] leading-none font-semibold tracking-tight">
        250.00
        <span className="flex items-center gap-1.5 text-[13px] font-medium text-gum-grey">
          <Image
            src="/payment-icons/usdc.svg"
            width={64}
            height={64}
            alt=""
            className="size-4 rounded-full"
          />
          USDC
        </span>
      </p>
      <div className="mt-3 flex items-center justify-between gap-3 text-[12px]">
        <span className="text-gum-grey">Deposit address</span>
        <span className="font-mono">0x9a3F…A0c2</span>
      </div>
      <div className="mt-3.5">
        <PayWith index={index} leaving={leaving} />
      </div>
    </div>
  );
}
