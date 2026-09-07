import { render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { payment } from "@/test/fixtures";

/**
 * The explorer is read from the env at import time (lib/config), so each case
 * re-imports the component under its own env.
 */
async function importAssetNotice() {
  vi.resetModules();
  return import("./asset-notice");
}

describe("AssetNotice", () => {
  beforeEach(() => {
    delete process.env.NEXT_PUBLIC_EXPLORER_BASE_URL;
  });

  afterEach(() => {
    vi.resetModules();
    delete process.env.NEXT_PUBLIC_EXPLORER_BASE_URL;
  });

  it("keeps the warning to the one sentence, and links the token contract to the explorer's token page", async () => {
    process.env.NEXT_PUBLIC_EXPLORER_BASE_URL = "https://monadvision.com";
    const { AssetNotice } = await importAssetNotice();

    render(<AssetNotice payment={payment()} />);

    expect(screen.queryByText(/A matching symbol/i)).not.toBeInTheDocument();
    const link = screen.getByRole("link", { name: /0x754704/i });
    expect(link).toHaveAttribute(
      "href",
      "https://monadvision.com/token/0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
    );
    expect(link).toHaveAttribute("target", "_blank");
  });

  it("shows the bare address when no explorer is configured", async () => {
    const { AssetNotice } = await importAssetNotice();

    render(<AssetNotice payment={payment()} />);

    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    expect(screen.getByTitle("0x754704Bc059F8C67012fEd69BC8A327a5aafb603")).toBeInTheDocument();
  });
});
