import Image from "next/image";
import { ArrowRight, Check, LockKeyhole, LoaderCircle } from "lucide-react";

const DEPOSITS = [
  {
    initials: "OA",
    email: "omar@finsystems.com",
    reference: "REF # 2381",
    token: "usdc",
    amount: "2,500",
    checks: ["Email", "Wallet"],
    state: "Received",
  },
  {
    initials: "SM",
    email: "sam@banana.app",
    reference: "REF # 71144",
    token: "ausd",
    amount: "160",
    checks: ["Email"],
    state: "Waiting",
  },
  {
    initials: "JL",
    email: "jamie@lattice.co",
    reference: "REF # 5048",
    token: "usdc",
    amount: "4,850",
    checks: ["Email", "Wallet"],
    state: "Received",
  },
  {
    initials: "AN",
    email: "ana@northstar.io",
    reference: "REF # 31607",
    token: "ausd",
    amount: "920",
    checks: ["Email"],
    state: "Waiting",
  },
  {
    initials: "KM",
    email: "kira@workato.so",
    reference: "REF # 9926",
    token: "usdt",
    amount: "27,000",
    checks: ["Email", "Wallet"],
    state: "Unverified",
    selected: true,
  },
  {
    initials: "RT",
    email: "ravi@tandem.xyz",
    reference: "REF # 8042",
    token: "usdt",
    amount: "12,400",
    checks: ["Email", "Wallet"],
    state: "Received",
  },
] as const;

const TOKEN_LABELS = { usdc: "USDC", usdt: "USDT", ausd: "AUSD" } as const;
type ScenePhase = "a" | "b";

export function DepositScene() {
  return (
    <div className="deposit-scene">
      <div className="deposit-window scene-deposit-window">
        <DepositTrack />
      </div>

      <CheckoutPanel />
    </div>
  );
}

function DepositTrack() {
  return (
    <div className="deposit-track scene-deposit-track">
      {Array.from({ length: 4 }, (_, set) =>
        DEPOSITS.map((payment, index) => {
          const phase = index === 5 ? "a" : index === 2 ? "b" : undefined;

          return (
            <DepositRow
              key={`${set}-${payment.reference}`}
              payment={payment}
              phase={phase}
              ariaHidden={set !== 1}
            />
          );
        }),
      )}
    </div>
  );
}

function DepositRow({
  payment,
  phase,
  ariaHidden,
}: {
  payment: (typeof DEPOSITS)[number];
  phase: ScenePhase | undefined;
  ariaHidden: boolean;
}) {
  const isSelected = phase !== undefined;
  const isVerified = !isSelected && payment.state !== "Unverified";

  return (
    <article
      aria-hidden={ariaHidden || undefined}
      className={`deposit-row scene-deposit-row ${phase ? `scene-target scene-target-${phase}` : ""}`}
    >
      <div className="flex min-w-0 items-center gap-2.5">
        <span className="flex size-10 shrink-0 items-center justify-center rounded-full bg-[#292a28] text-[13px] text-brand-grey">
          {payment.initials}
        </span>
        <span className="truncate text-[13px] text-brand-yellow sm:text-[14px]">
          {payment.email}
        </span>
      </div>

      <div className="deposit-reference flex items-center gap-2 text-[13px] text-[#c8c8c3]">
        <Image src="/payment-icons/pdf.svg" alt="" width={22} height={22} className="size-[21px]" />
        <span className="whitespace-nowrap">{payment.reference}</span>
      </div>

      <div className="flex items-center gap-2 text-[13px] text-[#c8c8c3] sm:text-[14px]">
        <Image
          src={`/payment-icons/${payment.token}.svg`}
          alt={TOKEN_LABELS[payment.token]}
          width={30}
          height={30}
          className="size-7 shrink-0 rounded-full"
        />
        <span className="tabular whitespace-nowrap">{payment.amount}</span>
      </div>

      <div
        className={`deposit-checks flex items-center gap-2 ${isVerified ? "text-[#31ae58]" : "text-brand-grey"} ${phase ? "scene-selected-checks" : ""}`}
      >
        {isSelected ? (
          <span className="relative size-7 shrink-0">
            <Image
              src="/payment-icons/warning.svg"
              alt=""
              width={28}
              height={28}
              className="scene-selected-warning absolute inset-0 size-7"
            />
            <Image
              src="/payment-icons/verified.svg"
              alt=""
              width={28}
              height={28}
              className="scene-selected-verified absolute inset-0 size-7"
            />
          </span>
        ) : (
          <Image
            src={`/payment-icons/${isVerified ? "verified" : "warning"}.svg`}
            alt=""
            width={28}
            height={28}
            className="size-7 shrink-0"
          />
        )}
        <span className="whitespace-nowrap text-[13px]">
          {payment.checks.map((check, index) => (
            <span key={check}>
              {index > 0 && <span className="text-brand-grey/40"> | </span>}
              {check}
            </span>
          ))}
        </span>
      </div>

      {isSelected ? (
        <span className="deposit-status-wrap relative h-10 min-w-[118px] justify-self-end">
          <span className="status-badge status-unverified scene-unverified-status absolute inset-0">
            <span className="size-2.5 rounded-full bg-current" />
            Unverified
          </span>
          <span className="status-badge status-received scene-complete-status absolute inset-0">
            <span className="size-2.5 rounded-full bg-current" />
            Received
          </span>
        </span>
      ) : (
        <span className={`status-badge status-${payment.state.toLowerCase()}`}>
          <span className="size-2.5 rounded-full bg-current" />
          {payment.state}
        </span>
      )}
    </article>
  );
}

function CheckoutPanel() {
  return (
    <aside className="checkout-panel" aria-label="Deposit verification preview">
      <div className="checkout-variant checkout-variant-a" aria-hidden="true">
        <CheckoutPanelContent payment={DEPOSITS[5]} />
      </div>
      <div className="checkout-variant checkout-variant-b" aria-hidden="true">
        <CheckoutPanelContent payment={DEPOSITS[2]} />
      </div>
    </aside>
  );
}

function CheckoutPanelContent({ payment }: { payment: (typeof DEPOSITS)[number] }) {
  return (
    <>
      <div className="checkout-upper">
        <div className="checkout-qr-wrap">
          <Image src="/payday-qr.svg" alt="QR code linking to payday.sh" width={116} height={116} />
          <span className="qr-scanner" aria-hidden="true" />
        </div>

        <div className="checkout-details">
          <div className="checkout-concealed" aria-hidden="true">
            <span className="skeleton-bar w-full" />
            <span className="skeleton-bar w-[72%]" />
            <span className="skeleton-bar w-[88%]" />
          </div>
          <div className="checkout-verified">
            <p className="flex items-center gap-2 text-[15px] font-medium text-brand-white">
              Verified <Check className="size-4 rounded-full bg-[#24bd68] p-0.5 text-brand-black" />
            </p>
            <p className="mt-3 text-[12px] leading-relaxed text-brand-grey">
              {payment.email}
              <br />
              {payment.checks.join(" · ")} confirmed
            </p>
          </div>
        </div>
      </div>

      <button
        type="button"
        tabIndex={-1}
        className="checkout-pay"
        aria-label="Animated deposit button preview"
      >
        <span className="checkout-amount">
          <span className="checkout-amount-concealed" aria-hidden="true">
            <span className="size-7 rounded-full bg-[#777271]" />
            <span className="skeleton-bar h-3 w-[74px]" />
          </span>
          <span className="checkout-amount-revealed">
            <Image
              src={`/payment-icons/${payment.token}.svg`}
              alt={TOKEN_LABELS[payment.token]}
              width={30}
              height={30}
            />
            <span className="tabular">{payment.amount}</span>
          </span>
        </span>
        <span className="checkout-pay-action">
          <span className="pay-state pay-locked">
            <LockKeyhole /> Pay
          </span>
          <span className="pay-state pay-ready">
            Pay <ArrowRight />
          </span>
          <span className="pay-state pay-loading">
            <LoaderCircle className="animate-spin" /> Processing
          </span>
          <span className="pay-state pay-complete">
            <Check /> Deposited
          </span>
        </span>
      </button>
    </>
  );
}
