import { fireEvent, render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";
import { payment } from "@/test/fixtures";
import { QrPanel } from "./qr-panel";

const QR_URL = "https://api.example.test/v1/payer/payments/pay_1/qr";

describe("QrPanel", () => {
  it("requests the QR for the amount still due", () => {
    render(<QrPanel payment={payment()} qrUrl={QR_URL} />);
    expect(screen.getByRole("img")).toHaveAttribute("src", `${QR_URL}?v=25000000`);
  });

  it("re-requests it when a partial payment lowers the remainder", () => {
    const { rerender } = render(<QrPanel payment={payment()} qrUrl={QR_URL} />);
    rerender(
      <QrPanel
        payment={payment({ remaining: "15.00", remaining_base_units: "15000000" })}
        qrUrl={QR_URL}
      />,
    );
    expect(screen.getByRole("img")).toHaveAttribute("src", `${QR_URL}?v=15000000`);
  });

  it("removes itself when the gateway stops serving the code", () => {
    // The QR route answers 410 once the payment is no longer payable, which
    // reaches the browser as a load error. Showing a stale address after that
    // would invite a transfer that routes to the merchant's refund wallet.
    render(<QrPanel payment={payment()} qrUrl={QR_URL} />);
    fireEvent.error(screen.getByRole("img"));
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
  });
});
