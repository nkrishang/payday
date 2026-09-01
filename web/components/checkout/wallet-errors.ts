/**
 * Wallet failures reach us as provider errors, viem errors, or bare objects,
 * depending on the wallet. This maps the ones a payer can act on to plain
 * language and leaves everything else generic rather than surfacing a stack.
 */
export function walletErrorMessage(error: unknown): string {
  const code = readCode(error);
  const message = readMessage(error);

  // EIP-1193: 4001 user rejected, 4100 unauthorized, 4902 chain not added.
  if (code === 4001 || /user rejected|user denied|rejected the request/i.test(message)) {
    return "You declined the request in your wallet.";
  }
  if (code === 4902 || /unrecognized chain|chain .* not added/i.test(message)) {
    return "Add this network to your wallet, then try again.";
  }
  if (/insufficient funds/i.test(message)) {
    return "Not enough gas in this wallet to send the transfer.";
  }
  if (/transfer amount exceeds balance|exceeds balance/i.test(message)) {
    return "This wallet does not hold enough USDC for the remaining amount.";
  }
  if (/chain mismatch|does not match the target chain/i.test(message)) {
    return "Your wallet is on a different network. Switch networks and try again.";
  }
  if (/timeout|timed out/i.test(message)) {
    return "The wallet did not respond. Try again.";
  }
  return "The transfer could not be sent. Try again, or pay by scanning the code instead.";
}

function readCode(error: unknown): number | null {
  if (typeof error !== "object" || error === null) return null;
  const record = error as Record<string, unknown>;
  if (typeof record.code === "number") return record.code;
  const cause = record.cause;
  if (typeof cause === "object" && cause !== null) {
    const causeCode = (cause as Record<string, unknown>).code;
    if (typeof causeCode === "number") return causeCode;
  }
  return null;
}

function readMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (typeof error !== "object" || error === null) return "";
  const record = error as Record<string, unknown>;
  const parts: string[] = [];
  if (typeof record.shortMessage === "string") parts.push(record.shortMessage);
  if (typeof record.details === "string") parts.push(record.details);
  if (typeof record.message === "string") parts.push(record.message);
  if (record.cause) parts.push(readMessage(record.cause));
  return parts.join(" ");
}
