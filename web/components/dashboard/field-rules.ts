/**
 * The shapes fields are checked against, in one place so the composer, the
 * setup form, and the identity manager cannot disagree — and so each mirrors
 * exactly what the API would refuse.
 */

/** 0x and 40 hex characters. */
export const PAYOUT_ADDRESS = /^0x[0-9a-fA-F]{40}$/;

export const EMAIL = /^[^@\s]+@[^@\s]+\.[^@\s]+$/;

/** USDC, up to six decimals. */
export const AMOUNT = /^\d{1,12}(\.\d{1,6})?$/;

/**
 * A payout address label: a short handle read in a picker. Twenty characters,
 * starting on a letter or digit, from a small printable set — the same rule
 * the API and the column enforce.
 */
export const LABEL = /^[A-Za-z0-9][A-Za-z0-9 ._'&()-]{0,19}$/;

/**
 * Two identities with the same name are the same row to whoever reads the
 * list, so the name is unique per account — case and surrounding space are not
 * a difference, which is the rule the column enforces too. `exceptId` is the
 * identity being renamed, which never clashes with itself.
 */
export function duplicateName(
  name: string,
  identities: ReadonlyArray<{ id: string; name: string }>,
  exceptId?: string,
): boolean {
  const wanted = name.trim().toLowerCase();
  if (!wanted) return false;
  return identities.some(
    (entry) => entry.id !== exceptId && entry.name.trim().toLowerCase() === wanted,
  );
}
