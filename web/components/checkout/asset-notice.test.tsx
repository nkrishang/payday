import { render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { payment } from "@/test/fixtures";

const WITH_EXPLORER = JSON.stringify([
  {
    id: 143,
    name: "Monad",
    rpcUrl: "https://rpc.example.test",
    usdcAddress: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
    explorerUrl: "https://monadvision.com",
  },
]);
const WITHOUT_EXPLORER = JSON.stringify([
  {
    id: 143,
    name: "Monad",
    rpcUrl: "https://rpc.example.test",
    usdcAddress: "0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
  },
]);

/**
 * The chains, with their explorers, are read from the env at import time
 * (lib/config), so each case re-imports the component under its own env.
 */
async function importAssetNotice(chains: string) {
  vi.resetModules();
  process.env.NEXT_PUBLIC_CHAINS = chains;
  return import("./asset-notice");
}

describe("AssetNotice", () => {
  const original = process.env.NEXT_PUBLIC_CHAINS;

  afterEach(() => {
    vi.resetModules();
    process.env.NEXT_PUBLIC_CHAINS = original;
  });

  it("keeps the warning to the one sentence, and links the token contract to the chosen chain's explorer", async () => {
    const { AssetNotice } = await importAssetNotice(WITH_EXPLORER);

    render(<AssetNotice payment={payment()} />);

    expect(screen.queryByText(/A matching symbol/i)).not.toBeInTheDocument();
    expect(screen.getByText(/the network you chose/i)).toBeInTheDocument();
    const link = screen.getByRole("link", { name: /0x754704/i });
    expect(link).toHaveAttribute(
      "href",
      "https://monadvision.com/token/0x754704Bc059F8C67012fEd69BC8A327a5aafb603",
    );
    expect(link).toHaveAttribute("target", "_blank");
  });

  it("shows the bare address when the chosen chain has no explorer", async () => {
    const { AssetNotice } = await importAssetNotice(WITHOUT_EXPLORER);

    render(<AssetNotice payment={payment()} />);

    expect(screen.queryByRole("link")).not.toBeInTheDocument();
    expect(screen.getByTitle("0x754704Bc059F8C67012fEd69BC8A327a5aafb603")).toBeInTheDocument();
  });
});
