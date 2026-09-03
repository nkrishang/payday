/**
 * Amount handling is exact throughout: USDC has six decimals and the API sends
 * every amount twice, as a display string and as integer `_base_units`. Base
 * units are the only value we do arithmetic on, always as BigInt. Nothing here
 * may touch a binary float.
 */

/** Groups the integer part in threes without going through Number. */
export function groupDigits(decimal: string): string {
  const [whole = "", fraction] = decimal.split(".");
  const sign = whole.startsWith("-") ? "-" : "";
  const digits = sign ? whole.slice(1) : whole;
  const grouped = digits.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
  return fraction === undefined ? `${sign}${grouped}` : `${sign}${grouped}.${fraction}`;
}

/** Exact base-units -> decimal string, with trailing zeros trimmed to `minFraction`. */
export function formatBaseUnits(
  baseUnits: string | bigint,
  decimals: number,
  minFraction = 2,
): string {
  const value = typeof baseUnits === "bigint" ? baseUnits : BigInt(baseUnits);
  const negative = value < 0n;
  const digits = (negative ? -value : value).toString().padStart(decimals + 1, "0");
  const whole = digits.slice(0, digits.length - decimals);
  let fraction = decimals === 0 ? "" : digits.slice(digits.length - decimals);
  while (fraction.length > minFraction && fraction.endsWith("0")) {
    fraction = fraction.slice(0, -1);
  }
  const body = fraction ? `${whole}.${fraction}` : whole;
  return `${negative ? "-" : ""}${groupDigits(body)}`;
}

/**
 * Presents an API display string without re-deriving it. The API sends USDC at
 * full precision ("25.000000"); a payer should read "25.00", but a fraction
 * that carries real value keeps every digit.
 */
export function formatDisplayAmount(amount: string, minFraction = 2): string {
  const [whole = "0", fraction = ""] = amount.split(".");
  let trimmed = fraction;
  while (trimmed.length > minFraction && trimmed.endsWith("0")) {
    trimmed = trimmed.slice(0, -1);
  }
  const padded = trimmed.padEnd(minFraction, "0");
  return groupDigits(padded ? `${whole}.${padded}` : whole);
}

export function truncateAddress(address: string, lead = 6, tail = 4): string {
  if (address.length <= lead + tail + 1) return address;
  return `${address.slice(0, lead)}…${address.slice(-tail)}`;
}

export function truncateHash(hash: string): string {
  return truncateAddress(hash, 10, 8);
}

/** Whole seconds remaining, floored at zero. */
export function clampSeconds(seconds: number): number {
  return seconds > 0 ? Math.floor(seconds) : 0;
}

/** "1d 04h", "3h 27m", "9:41", "0:08" — the largest two units that matter. */
export function formatCountdown(totalSeconds: number): string {
  const seconds = clampSeconds(totalSeconds);
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  const rest = seconds % 60;
  if (days > 0) return `${days}d ${String(hours).padStart(2, "0")}h`;
  if (hours > 0) return `${hours}h ${String(minutes).padStart(2, "0")}m`;
  return `${minutes}:${String(rest).padStart(2, "0")}`;
}

/** Received share of the requested amount, 0-100, exact until the final divide. */
export function receivedPercent(receivedBaseUnits: string, amountBaseUnits: string): number {
  const amount = BigInt(amountBaseUnits);
  if (amount <= 0n) return 0;
  const received = BigInt(receivedBaseUnits);
  if (received >= amount) return 100;
  return Number((received * 10000n) / amount) / 100;
}

export function explorerAddressUrl(explorerUrl: string | null, address: string): string | null {
  return explorerUrl ? `${explorerUrl}/address/${address}` : null;
}

export function explorerTxUrl(explorerUrl: string | null, hash: string): string | null {
  return explorerUrl ? `${explorerUrl}/tx/${hash}` : null;
}

/** "48.2 KB", "1.5 MB" — for an attachment's `byte_length`, which arrives as a decimal string. */
export function formatBytes(byteLength: string | number): string {
  const bytes = Number(byteLength);
  if (!Number.isFinite(bytes) || bytes < 0) return "";
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}
