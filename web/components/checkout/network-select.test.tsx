import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, it, vi } from "vitest";
import { NETWORKS as networks } from "@/test/fixtures";
import { NetworkSelect } from "./network-select";

/** The selector must offer every configured chain until an attestation
 * starts, then hold the payer's choice until it completes. */
describe("NetworkSelect", () => {
  it("enables the configured chains and reports the selection", async () => {
    const onSelect = vi.fn();
    render(<NetworkSelect networks={networks} selected={null} onSelect={onSelect} />);
    const monad = screen.getByRole("radio", { name: /Monad/ });
    const base = screen.getByRole("radio", { name: /Base/ });
    expect(monad).toBeEnabled();
    expect(base).toBeEnabled();
    await userEvent.click(base);
    expect(onSelect).toHaveBeenCalledWith("8453");
  });

  it("states a pinned network instead of offering a choice of one", () => {
    const [monad] = networks;
    render(<NetworkSelect networks={[monad!]} selected="143" onSelect={vi.fn()} />);
    expect(screen.queryByRole("radiogroup")).toBeNull();
    expect(screen.getByTestId("pinned-network")).toHaveTextContent(/Monad/);
    expect(screen.getByTestId("pinned-network")).toHaveTextContent(/set by the merchant/);
  });

  it("locks every chain while an attestation is in flight", () => {
    render(<NetworkSelect networks={networks} selected="143" onSelect={vi.fn()} disabled />);
    expect(screen.getByRole("radio", { name: /Monad/ })).toBeDisabled();
    expect(screen.getByRole("radio", { name: /Base/ })).toBeDisabled();
    expect(screen.getByRole("radio", { name: /Monad/ })).toBeChecked();
  });
});
