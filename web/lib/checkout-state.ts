import type { Chain, PayerPayment, PaymentStatus, Token } from "@payday/sdk";
import { formatDisplayAmount } from "./format";

/**
 * The checkout renders one of these phases. They are derived from the server's
 * status plus two local facts, and nothing else.
 *
 * Three rules this module exists to enforce:
 *
 * 1. The payer's device clock never decides that a payment expired — chain time
 *    does. When the countdown reaches zero we move to `closing`, which says the
 *    deadline was reached and waits for the server to confirm, rather than
 *    claiming the payment is over.
 * 2. Payment instructions disappear the moment the payment stops being payable.
 *    Funds sent after the deadline route to the Payday recovery wallet, not
 *    back to the payer, so continuing to show an address would cause real loss.
 * 3. A gated invoice discloses nothing but the issuer and heading until the
 *    gateway says the content is unlocked. The API withholds the fields; this
 *    module turns their absence into a phase so no component ever reaches for
 *    a null amount or address.
 */
export type CheckoutPhase =
  | "verification_required"
  | "email_pending"
  /** A merchant-session invoice: only the merchant's app can open it. */
  | "app_required"
  /** The client secret from the fragment is being exchanged. */
  | "app_opening"
  | "awaiting"
  | "partial"
  | "confirming"
  | "closing"
  | "paid"
  | "settled"
  | "expired_empty"
  | "expired_funded"
  | "returned"
  | "attention";

export type CheckoutTone = "neutral" | "progress" | "success" | "warning";

export interface CheckoutLocalState {
  /** Whole seconds until the deadline, anchored to the gateway's clock. */
  secondsRemaining: number;
  /** Hash of a transfer this browser sent that the gateway has not yet credited. */
  pendingTxHash: string | null;
  /** A verification code was sent for this tab's session and awaits entry. */
  emailCodeSent?: boolean;
  /** A merchant client secret from the fragment is being exchanged right now. */
  exchangingClientSecret?: boolean;
}

export interface CheckoutView {
  phase: CheckoutPhase;
  tone: CheckoutTone;
  /** Short label for the status pill. */
  label: string;
  title: string;
  detail: string;
  /** Whether the address, QR, and wallet button may be shown. */
  showInstructions: boolean;
  /** Whether the payment can no longer change. */
  isTerminal: boolean;
}

/**
 * A payer payment whose mechanics are present. The API nulls every one of
 * these together while a gated invoice is locked, so components that render
 * an amount or an address take this type and never see a null.
 */
export type UnlockedPayerPayment = PayerPayment & {
  chain: Chain;
  token: Token;
  amount: string;
  amount_base_units: string;
  received: string;
  received_base_units: string;
  remaining: string;
  remaining_base_units: string;
  address: string;
};

/**
 * Narrows to the unlocked shape, or null while the content is withheld. One
 * flag drives every gated field on the API side; checking the fields too means
 * a response that disagrees with its own flag is treated as locked rather than
 * rendered with holes.
 */
export function unlockedPayment(payment: PayerPayment): UnlockedPayerPayment | null {
  if (!payment.content_unlocked) return null;
  const {
    chain,
    token,
    amount,
    amount_base_units,
    received,
    received_base_units,
    remaining,
    remaining_base_units,
    address,
  } = payment;
  if (
    chain === null ||
    token === null ||
    amount === null ||
    amount_base_units === null ||
    received === null ||
    received_base_units === null ||
    remaining === null ||
    remaining_base_units === null ||
    address === null
  ) {
    return null;
  }
  return {
    ...payment,
    chain,
    token,
    amount,
    amount_base_units,
    received,
    received_base_units,
    remaining,
    remaining_base_units,
    address,
  };
}

const TERMINAL: ReadonlySet<PaymentStatus> = new Set<PaymentStatus>([
  "settled",
  "returned",
  "needs_attention",
]);

export function isTerminalStatus(status: PaymentStatus): boolean {
  return TERMINAL.has(status);
}

const RECOVERY_NOTE =
  "The full balance goes to the Payday recovery wallet, which is not automatically the payer. Contact the merchant and Payday support for return handling.";

export function checkoutView(payment: PayerPayment, local: CheckoutLocalState): CheckoutView {
  const unlocked = unlockedPayment(payment);

  if (unlocked === null) {
    // Locked content comes before every lifecycle state: a payer who has not
    // verified is told nothing about the amount, not even that it was paid.
    return lockedView(payment, local);
  }

  return unlockedView(unlocked, local);
}

/**
 * The two locked phases, in the order a payer moves through them. The
 * facts come from the API for this tab's session; only "a code is on its
 * way" is local, because the API cannot know which tab asked.
 */
function lockedView(payment: PayerPayment, local: CheckoutLocalState): CheckoutView {
  const { requirements, payer_policy } = payment;

  // A merchant-session invoice has no step for the payer to take here: the
  // merchant's app opened it, or nothing will.
  if (payer_policy.mode === "merchant_session") {
    return local.exchangingClientSecret
      ? {
          phase: "app_opening",
          tone: "progress",
          label: "Opening",
          title: "Opening your payment",
          detail: `${payment.issuer_name} is opening this payment for you.`,
          showInstructions: false,
          isTerminal: false,
        }
      : {
          phase: "app_required",
          tone: "neutral",
          label: "Open from the app",
          title: `Open this payment from ${payment.issuer_name}`,
          detail:
            "The amount and payment details are shown once the app that issued this payment opens it for you.",
          showInstructions: false,
          isTerminal: false,
        };
  }

  // Invoice-level completion from another payer is not evidence for this tab.
  // Treat the internally inconsistent locked/complete response as a fresh gate.
  if (requirements.complete) {
    return {
      phase: "verification_required",
      tone: "neutral",
      label: "Verification required",
      title: "Verify to view this invoice",
      detail: "The amount, payment details, and attachment are shown once you verify.",
      showInstructions: false,
      isTerminal: false,
    };
  }

  if (local.emailCodeSent) {
    return {
      phase: "email_pending",
      tone: "progress",
      label: "Check your email",
      title: "Enter the code we sent",
      detail: `A one-time code was sent to ${payer_policy.expected_email_hint ?? "the expected mailbox"}. Enter it here to continue.`,
      showInstructions: false,
      isTerminal: false,
    };
  }

  return {
    phase: "verification_required",
    tone: "neutral",
    label: "Verification required",
    title: "Verify to view this invoice",
    detail:
      "The amount, payment details, and attachment are shown once the expected payer has verified.",
    showInstructions: false,
    isTerminal: false,
  };
}

function unlockedView(payment: UnlockedPayerPayment, local: CheckoutLocalState): CheckoutView {
  const received = BigInt(payment.received_base_units);

  if (payment.status === "needs_attention") {
    return {
      phase: "attention",
      tone: "warning",
      label: "Needs attention",
      title: "Settlement is paused",
      detail:
        payment.payer_message ??
        "Payday has paused this payment and an operator is resolving it. Do not send another payment.",
      showInstructions: false,
      isTerminal: true,
    };
  }

  if (payment.status === "settled") {
    // Settlement is exact: the merchant receives the invoice amount and any
    // remainder goes to the recovery wallet, so an overpaid payer is told where
    // the rest went rather than left to assume the merchant is holding it.
    const overpaid = received > BigInt(payment.amount_base_units);
    return {
      phase: "settled",
      tone: "success",
      label: "Paid",
      title: "Payment complete",
      detail:
        "Exactly the invoice amount reached the merchant. You can close this page." +
        (overpaid
          ? " Anything above the invoice amount went to the Payday recovery wallet; contact the merchant and Payday support for return handling."
          : ""),
      showInstructions: false,
      isTerminal: true,
    };
  }

  if (payment.status === "returned") {
    return {
      phase: "returned",
      tone: "warning",
      label: "Returned",
      title: "This payment was not completed in time",
      detail: RECOVERY_NOTE,
      showInstructions: false,
      isTerminal: true,
    };
  }

  if (payment.status === "expired") {
    return received === 0n
      ? {
          phase: "expired_empty",
          tone: "neutral",
          label: "Expired",
          title: "This payment link has expired",
          detail:
            "Nothing was sent to it. Ask the merchant for a new payment link — this address must not be used.",
          showInstructions: false,
          isTerminal: true,
        }
      : {
          phase: "expired_funded",
          tone: "warning",
          label: "Expired",
          title: "The deadline passed before this payment completed",
          detail: RECOVERY_NOTE,
          showInstructions: false,
          isTerminal: true,
        };
  }

  if (payment.status === "paid") {
    return {
      phase: "paid",
      tone: "progress",
      label: "Received",
      title: "Payment received",
      detail:
        "Payday is settling the invoice amount to the merchant. Nothing more is needed from you.",
      showInstructions: false,
      isTerminal: false,
    };
  }

  // Remaining: awaiting_payment and partially_paid, which are the payable states.
  const deadlineReached = !payment.payable || local.secondsRemaining <= 0;

  if (deadlineReached) {
    return {
      phase: "closing",
      tone: "warning",
      label: "Closing",
      title: "The deadline has been reached",
      detail:
        "Do not send funds now. Payday is confirming the final on-chain state; the chain's clock, not this page, decides the outcome.",
      showInstructions: false,
      isTerminal: false,
    };
  }

  if (local.pendingTxHash) {
    return {
      phase: "confirming",
      tone: "progress",
      label: "Confirming",
      title: "Transaction confirmed on-chain",
      detail:
        "Payday credits transfers once the network finalizes them, so this can lag your wallet by a moment. Keep this page open.",
      showInstructions: false,
      isTerminal: false,
    };
  }

  if (payment.status === "partially_paid") {
    return {
      phase: "partial",
      tone: "progress",
      label: "Partially paid",
      title: `Send the remaining ${formatDisplayAmount(payment.remaining)} ${payment.token.symbol}`,
      detail:
        "Transfers accumulate. If the total is still short at the deadline, the balance goes to the Payday recovery wallet.",
      showInstructions: true,
      isTerminal: false,
    };
  }

  return {
    phase: "awaiting",
    tone: "neutral",
    label: "Awaiting payment",
    title: "Amount due",
    detail: `Send exactly this amount of ${payment.token.symbol} on ${payment.chain.name}.`,
    showInstructions: true,
    isTerminal: false,
  };
}
