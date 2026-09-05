import { PaydayError } from "@payday/sdk";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { payerClient } from "@/lib/payday";
import { AttachmentLink } from "./attachment-link";

const ATTACHMENT = {
  id: "0198f80c-8d2f-7dc1-a369-90556a64f7aa",
  filename: "INV-1042.pdf",
  mime_type: "application/pdf" as const,
  byte_length: "48211",
  sha256: "0x9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08",
};

const DOWNLOAD_URL = "https://bucket.example.test/uploads/x.pdf?X-Amz-Signature=abc";

describe("AttachmentLink", () => {
  const open = vi.fn<typeof window.open>();

  beforeEach(() => {
    open.mockReset();
    vi.spyOn(window, "open").mockImplementation(open);
  });

  afterEach(() => {
    vi.restoreAllMocks();
  });

  it("fetches the signed URL only when asked, then opens it in a new tab", async () => {
    const fetchDescriptor = vi
      .spyOn(payerClient.depositRequests, "attachment")
      .mockResolvedValue({ ...ATTACHMENT, download_url: DOWNLOAD_URL });
    const tab = { opener: window, location: { replace: vi.fn() }, close: vi.fn() };
    open.mockReturnValue(tab as unknown as Window);

    const { container } = render(<AttachmentLink paymentId="dr_1" attachment={ATTACHMENT} />);

    // Nothing is fetched, and no URL is in the markup, until the click.
    expect(fetchDescriptor).not.toHaveBeenCalled();
    expect(container.innerHTML).not.toContain("X-Amz-Signature");
    expect(screen.getByText("PDF · 47.1 KB")).toBeInTheDocument();

    fireEvent.click(screen.getByRole("button", { name: /INV-1042\.pdf/ }));

    await waitFor(() => expect(open).toHaveBeenCalledWith("", "_blank"));
    expect(tab.location.replace).toHaveBeenCalledWith(DOWNLOAD_URL);
    expect(fetchDescriptor).toHaveBeenCalledWith("dr_1");
    expect(container.innerHTML).not.toContain("X-Amz-Signature");
  });

  it("presents this tab's session so a gated deposit request's descriptor is minted for it", async () => {
    const fetchDescriptor = vi
      .spyOn(payerClient.depositRequests, "attachment")
      .mockResolvedValue({ ...ATTACHMENT, download_url: DOWNLOAD_URL });
    open.mockReturnValue({ opener: null, location: { replace: vi.fn() } } as unknown as Window);

    const { container } = render(
      <AttachmentLink paymentId="dr_1" attachment={ATTACHMENT} payerSession="pps_token" />,
    );
    fireEvent.click(screen.getByRole("button", { name: /INV-1042\.pdf/ }));

    await waitFor(() => expect(fetchDescriptor).toHaveBeenCalledWith("dr_1", "pps_token"));
    // The session is a header on the request, never part of the page.
    expect(container.innerHTML).not.toContain("pps_token");
  });

  it("falls back to a plain link when the browser blocks the tab", async () => {
    vi.spyOn(payerClient.depositRequests, "attachment").mockResolvedValue({
      ...ATTACHMENT,
      download_url: DOWNLOAD_URL,
    });
    open.mockReturnValue(null);

    render(<AttachmentLink paymentId="dr_1" attachment={ATTACHMENT} />);
    fireEvent.click(screen.getByRole("button", { name: /INV-1042\.pdf/ }));

    const link = await screen.findByRole("link", { name: /Open INV-1042\.pdf/ });
    expect(link).toHaveAttribute("href", DOWNLOAD_URL);
    expect(link).toHaveAttribute("rel", expect.stringContaining("noopener"));
  });

  it("tells a locked payer to verify first", async () => {
    vi.spyOn(payerClient.depositRequests, "attachment").mockRejectedValue(
      new PaydayError("Verify first", "verification_required", 401),
    );

    render(<AttachmentLink paymentId="dr_1" attachment={ATTACHMENT} />);
    fireEvent.click(screen.getByRole("button", { name: /INV-1042\.pdf/ }));

    expect(await screen.findByRole("alert")).toHaveTextContent(/verify first/i);
    expect(open).toHaveBeenCalledWith("", "_blank");
  });
});
