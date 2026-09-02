import { render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { payerClient } from "@/lib/payday";
import { payment } from "@/test/fixtures";
import { QrPanel } from "./qr-panel";

const SVG = new Blob(["<svg/>"], { type: "image/svg+xml" });

describe("QrPanel", () => {
  const createObjectURL = vi.fn<(blob: Blob) => string>();
  const revokeObjectURL = vi.fn<(url: string) => void>();

  beforeEach(() => {
    let n = 0;
    createObjectURL.mockReset().mockImplementation(() => `blob:qr-${++n}`);
    revokeObjectURL.mockReset();
    vi.stubGlobal("URL", Object.assign(URL, { createObjectURL, revokeObjectURL }));
  });

  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  it("fetches the QR for the amount still due, with the session in a header, and shows it from an object URL", async () => {
    const qr = vi.spyOn(payerClient.payments, "qr").mockResolvedValue(SVG);

    render(<QrPanel payment={payment()} payerSession="pps_token" />);

    const img = await screen.findByRole("img");
    expect(img).toHaveAttribute("src", "blob:qr-1");
    expect(qr).toHaveBeenCalledWith("pay_0198f80c-8d2f-7dc1-a369-90556a64f700", "pps_token", {
      signal: expect.any(AbortSignal),
    });
    expect(img.getAttribute("src")).not.toContain("pps_token");
  });

  it("re-fetches when a partial payment lowers the remainder, and revokes the old image", async () => {
    vi.spyOn(payerClient.payments, "qr").mockResolvedValue(SVG);
    const { rerender } = render(<QrPanel payment={payment()} />);
    await screen.findByRole("img");

    rerender(<QrPanel payment={payment({ remaining: "15.00", remaining_base_units: "15000000" })} />);

    await waitFor(() => expect(screen.getByRole("img")).toHaveAttribute("src", "blob:qr-2"));
    expect(revokeObjectURL).toHaveBeenCalledWith("blob:qr-1");
  });

  it("removes itself when the gateway stops serving the code", async () => {
    // The QR route answers 410 once the payment is no longer payable, which
    // reaches this component as a failed fetch. Showing a stale address after
    // that would invite a transfer that routes to the Payday recovery wallet
    // rather than back to the payer.
    vi.spyOn(payerClient.payments, "qr").mockRejectedValue(new Error("410"));

    const { container } = render(<QrPanel payment={payment()} />);

    await waitFor(() => expect(container).toBeEmptyDOMElement());
    expect(screen.queryByRole("img")).not.toBeInTheDocument();
  });
});
