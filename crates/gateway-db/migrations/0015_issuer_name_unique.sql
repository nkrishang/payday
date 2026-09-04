-- One name per identity, per account.
--
-- An identity is picked by its id everywhere it matters, so a duplicate name
-- breaks nothing on the wire — it breaks the merchant's ability to tell two
-- rows apart. Two identities called "Payday" are indistinguishable in a
-- picker, in the issuer badge on a request, and in the list itself, so the
-- name is required to be unique the way a human reads it: case and surrounding
-- space are not a difference.

-- Pre-release data may already hold duplicates. Keep the oldest of each set
-- and suffix the rest rather than refusing to start; the name stays inside its
-- 255-byte bound.
WITH ranked AS (
    SELECT id,
           row_number() OVER (
               PARTITION BY account_id, lower(btrim(name))
               ORDER BY created_at, id
           ) AS position,
           name
    FROM issuers
)
UPDATE issuers
SET name = left(ranked.name, 250) || ' (' || ranked.position || ')'
FROM ranked
WHERE issuers.id = ranked.id AND ranked.position > 1;

CREATE UNIQUE INDEX issuers_account_name_unique
    ON issuers(account_id, lower(btrim(name)));
