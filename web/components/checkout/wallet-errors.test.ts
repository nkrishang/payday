import { describe, expect, it } from "vitest";
import { walletErrorMessage } from "./wallet-errors";

describe("walletErrorMessage", () => {
  it("recognises a declined request by EIP-1193 code", () => {
    expect(walletErrorMessage({ code: 4001, message: "" })).toMatch(/declined/i);
  });

  it("recognises a declined request nested in a viem cause", () => {
    expect(
      walletErrorMessage({ name: "TransactionExecutionError", cause: { code: 4001 } }),
    ).toMatch(/declined/i);
  });

  it("recognises the wording wallets use when there is no code", () => {
    expect(walletErrorMessage(new Error("User rejected the request."))).toMatch(/declined/i);
    expect(walletErrorMessage({ shortMessage: "User denied transaction signature" })).toMatch(
      /declined/i,
    );
  });

  it("distinguishes missing gas from missing USDC", () => {
    expect(walletErrorMessage({ shortMessage: "Insufficient funds for gas" })).toMatch(/gas/i);
    expect(walletErrorMessage({ details: "ERC20: transfer amount exceeds balance" })).toMatch(
      /USDC/,
    );
  });

  it("explains an unknown network", () => {
    expect(walletErrorMessage({ code: 4902 })).toMatch(/add this network/i);
  });

  it("falls back without leaking internals", () => {
    const message = walletErrorMessage(new Error("execution reverted: 0xdeadbeef"));
    expect(message).toMatch(/could not be sent/i);
    expect(message).not.toContain("0xdeadbeef");
  });

  it("tolerates values that are not errors at all", () => {
    expect(walletErrorMessage(null)).toMatch(/could not be sent/i);
    expect(walletErrorMessage(undefined)).toMatch(/could not be sent/i);
    expect(walletErrorMessage("boom")).toMatch(/could not be sent/i);
  });
});
