import { describe, expect, it } from "vitest";
import {
  formatBaseUnits,
  formatBytes,
  formatCountdown,
  formatDisplayAmount,
  groupDigits,
  receivedPercent,
  truncateAddress,
} from "./format";

describe("formatBaseUnits", () => {
  it("converts six-decimal USDC exactly", () => {
    expect(formatBaseUnits("25000000", 6)).toBe("25.00");
    expect(formatBaseUnits("1", 6)).toBe("0.000001");
    expect(formatBaseUnits("0", 6)).toBe("0.00");
  });

  it("stays exact past the range where floats lose precision", () => {
    // 9007199254740993 base units is Number.MAX_SAFE_INTEGER + 2.
    expect(formatBaseUnits("9007199254740993", 6)).toBe("9,007,199,254.740993");
    expect(formatBaseUnits("123456789012345678901234567890", 6)).toBe(
      "123,456,789,012,345,678,901,234.56789",
    );
  });

  it("trims trailing zeros only down to the minimum fraction", () => {
    expect(formatBaseUnits("1500000", 6)).toBe("1.50");
    expect(formatBaseUnits("1500000", 6, 0)).toBe("1.5");
    expect(formatBaseUnits("1000000", 6, 0)).toBe("1");
  });

  it("accepts bigint input", () => {
    expect(formatBaseUnits(25_000_000n, 6)).toBe("25.00");
  });
});

describe("groupDigits", () => {
  it("groups the integer part only", () => {
    expect(groupDigits("1234567.891")).toBe("1,234,567.891");
    expect(groupDigits("100")).toBe("100");
    expect(groupDigits("1000")).toBe("1,000");
  });
});

describe("formatDisplayAmount", () => {
  it("pads to two decimals without re-deriving the value", () => {
    expect(formatDisplayAmount("25")).toBe("25.00");
    expect(formatDisplayAmount("25.5")).toBe("25.50");
  });

  it("trims the API's full six-decimal precision down to what a payer reads", () => {
    expect(formatDisplayAmount("25.000000")).toBe("25.00");
    expect(formatDisplayAmount("15.000000")).toBe("15.00");
    expect(formatDisplayAmount("0.000000")).toBe("0.00");
  });

  it("keeps every digit that carries value", () => {
    expect(formatDisplayAmount("1234.567000")).toBe("1,234.567");
    expect(formatDisplayAmount("0.000001")).toBe("0.000001");
  });
});

describe("receivedPercent", () => {
  it("computes a share without floating point on the inputs", () => {
    expect(receivedPercent("0", "25000000")).toBe(0);
    expect(receivedPercent("12500000", "25000000")).toBe(50);
    expect(receivedPercent("25000000", "25000000")).toBe(100);
  });

  it("caps at 100 for an overpayment and tolerates a zero amount", () => {
    expect(receivedPercent("50000000", "25000000")).toBe(100);
    expect(receivedPercent("1", "0")).toBe(0);
  });
});

describe("formatCountdown", () => {
  it("shows the two units that matter", () => {
    expect(formatCountdown(90061)).toBe("1d 01h");
    expect(formatCountdown(12420)).toBe("3h 27m");
    expect(formatCountdown(581)).toBe("9:41");
    expect(formatCountdown(8)).toBe("0:08");
  });

  it("floors at zero rather than going negative", () => {
    expect(formatCountdown(-5)).toBe("0:00");
  });
});

describe("truncateAddress", () => {
  it("keeps both ends recognisable", () => {
    expect(truncateAddress("0x9a3f0000000000000000000000000000000000c2")).toBe("0x9a3f…00c2");
  });

  it("leaves short values alone", () => {
    expect(truncateAddress("0x1234")).toBe("0x1234");
  });
});

describe("formatBytes", () => {
  it("reads an attachment's decimal byte_length at a sensible unit", () => {
    expect(formatBytes("512")).toBe("512 B");
    expect(formatBytes("48211")).toBe("47.1 KB");
    expect(formatBytes("5242880")).toBe("5.0 MB");
  });

  it("says nothing for a value that is not a byte count", () => {
    expect(formatBytes("nope")).toBe("");
  });
});
