import type { PayerPayment, PaymentStatus } from "@payday/sdk";
import { formatDisplayAmount } from "./format";

/**
 * The checkout renders one of these phases. They are derived from the server's
 * status plus two local facts, and nothing else.
 *
 * Two rules this module exists to enforce:
 *
 * 1. The payer's device clock never decides that a payment expired — chain time
 *    does. When the countdown reaches zero we move to `closing`, which says the
 *    deadline was reached and waits for the server to confirm, rather than
 *    claiming the payment is over.
 * 2. Payment instructions disappear the moment the payment stops being payable.
 *    Funds sent after the deadline route to the merchant's refund address, not
 *    back to the payer, so continuing to show an address would cause real loss.
 */
export type CheckoutPhase =
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

const TERMINAL: ReadonlySet<PaymentStatus> = new Set<PaymentStatus>([
  "settled",
  "returned",
  "needs_attention",
]);

export function isTerminalStatus(status: PaymentStatus): boolean {
  return TERMINAL.has(status);
}

const REFUND_NOTE =
  "The full balance goes to the merchant's refund address, which is not automatically the payer. Contact the merchant to arrange a return.";

export function checkoutView(payment: PayerPayment, local: CheckoutLocalState): CheckoutView {
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
    return {
      phase: "settled",
      tone: "success",
      label: "Paid",
      title: "Payment complete",
      detail: "The full balance reached the merchant. You can close this page.",
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
      detail: REFUND_NOTE,
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
          detail: REFUND_NOTE,
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
      detail: "Payday is settling the balance to the merchant. Nothing more is needed from you.",
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
        "Transfers accumulate. If the total is still short at the deadline, the balance goes to the merchant's refund address.",
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
