-- A payout address label is a short handle a merchant reads in a picker, not
-- free text: at most 20 characters, starting alphanumeric, and drawn from a
-- small printable set. Bounding it here as well as at the API keeps the column
-- honest whatever writes to it.
ALTER TABLE payout_addresses DROP CONSTRAINT payout_addresses_label_check;

ALTER TABLE payout_addresses ADD CONSTRAINT payout_addresses_label_check
    CHECK (label IS NULL OR label ~ '^[A-Za-z0-9][A-Za-z0-9 ._''&()-]{0,19}$');
