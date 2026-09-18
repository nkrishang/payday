/**
 * The shapes fields are checked against, in one place so the forms that use
 * them cannot disagree — and so each mirrors exactly what the API would
 * refuse.
 */

/** 0x and 40 hex characters. */
export const PAYOUT_ADDRESS = /^0x[0-9a-fA-F]{40}$/;

export const EMAIL = /^[^@\s]+@[^@\s]+\.[^@\s]+$/;
