# Merchant dashboard

The dashboard at `payday.sh/dashboard` is the browser face of the same API the
SDK and CLI use. It issues invoices, keeps customers, uploads the one PDF an
invoice may carry, and shows what happened to each payment. It adds no rules
of its own: every limit, policy check, and status comes from the API, and the
form only gives immediate feedback (a missing name, a file that is not a PDF)
before the API has the final word.

## Signing in

The dashboard signs in with the same emailed one-time code as the CLI. The
login page asks the identity issuer for a code, exchanges it for a short-lived
access token for the Payday API audience, and that token is the session
([Authentication § 7](authentication.md#7-dashboard-sessions)). It is kept in
memory and mirrored to the tab's `sessionStorage` so a reload does not demand a
new code; it is never written to `localStorage`, never placed in a URL, and
ends with the tab. **No API key exists in the browser** — the API accepts the
identity token directly on the payment, customer, and attachment routes, and
maps it to the merchant account. Signing out clears the token.

Locally the issuer is `payday-dev-identity` (the code is printed in its log)
and the client ID is `payday-dashboard-local`; in production it is the
`Payday Dashboard` Auth0 application. The web app is configured through
`NEXT_PUBLIC_AUTH0_DOMAIN`, `NEXT_PUBLIC_AUTH0_CLIENT_ID`, and
`NEXT_PUBLIC_AUTH0_AUDIENCE` (see `web/.env.example`).

Dashboard pages are rendered per request as empty shells: no invoice, customer,
or token is in server-rendered HTML or any build artifact, and every page is
marked `noindex`.

The dashboard calls the API from the browser, so `gatewayd` answers
cross-origin requests on the merchant routes from exactly one origin: the
web origin it is configured with as `PAYDAY_PUBLIC_BASE_URL`. The dashboard
must be served from that origin — `http://127.0.0.1:3002` locally,
`https://payday.sh` in production; from any other origin every request fails
its preflight. The payer checkout has no such constraint, because the payer
routes allow any origin.

## Invoices

**List.** Every invoice with its heading (or reference), the billed party, the
amount, the payment status, the payer-policy mode, the verification state, and a
paperclip when a PDF is attached. The status filter and the Previous/Next
controls are the API's own `status` and `starting_after` parameters.

**New invoice.** The issued document, exactly as `POST /v1/payments` takes it:

- issuer and bill-to parties (name, optional email, optional free-text
  details, rendered verbatim on the invoice);
- a customer picker that pre-fills bill-to from a saved customer and links the
  invoice to it — the invoice still stores its own snapshot;
- the amount in USDC, used directly (there are no line items), the payout
  address, and an optional deadline in hours;
- heading, reference, and notes;
- one PDF attachment, up to 5 MiB. The file goes straight to object storage
  with the API's presigned headers, then the dashboard polls finalization and
  shows the attachment's own stages: *uploading*, *scanning* (the malware scan
  has not reported), then *ready* with its size and SHA-256 — or *rejected*
  with the API's reason;
- the payer policy: one of the four modes, with the expected email for every
  verified mode and the expected first and last name for `verified_identity`
  only.

Issued invoices are immutable; a different amount, party, policy, or
attachment means a new invoice.

**Detail.** Everything the merchant `Payment` object carries, in four groups:

- *Document*: parties, customer link, heading, reference, notes, created and
  deadline times, and the attribution hash the payment address commits to;
- *Payment*: the one-time address and payout address, network and token,
  received versus requested, funded and settled times, the settlement
  transaction, and any operator attention message;
- *Verification*: the policy mode with the merchant's own assertions (expected
  email and, for `verified_identity`, the expected name), the verification
  verdict — separate from the payment status, because a gated invoice can be
  funded before its payer has verified — and, for gated invoices, the
  activity behind it: each fact (email, document, liveness and face match,
  name match) on its own, every attempt with the provider's reference and
  allowlisted risk categories, the reviewer's outcome and time, whether the
  payer may retry, and a *Request review* action for a declined identity
  check. The provider's extracted identity is never shown, because the API
  never has it;
- *Attachment* and *Downloads*: the attached PDF through a short-lived signed
  URL fetched on demand, the deterministic invoice PDF
  (`GET /v1/payments/{id}/invoice.pdf`), and the Proof of Payment as JSON
  (`GET /v1/payments/{id}/proof`), which becomes available once the invoice
  settles and verifies offline with `payday proof verify`.

## Verification and recovery indicators

Two indicators sit beside the payment status and mean different things.

**Verification** — *Not required* for permissionless invoices; *Pending* until
the gateway records that the expected payer completed the policy's checks;
*Verified*, with the completion time, afterwards. For
`verified_identity_unattributed` the detail page notes that the check confirms
a real person, not who they are.

**Likely unsolicited** — shown, with the time, when finalized funds arrived at a
gated invoice before its verification completed. The address is not
quarantined: settlement waits for verification, and unverified funds are
recovered at expiry.

**Recovered funds** — a section on the detail page, present only when something
went to the Payday recovery wallet rather than the payout address: the
overpayment remainder on a settled invoice, the full balance of a returned one,
and every transfer the indexer classified as late, each with its transaction
hash and whether it has been collected. Recovered balances are returned after
review; quote the hash to support.

## Customers

A customer is a reusable counterparty record: name, optional email, optional
details. The list pages through `GET /v1/customers`; a customer's page edits it
(`PATCH`, a full replacement of the editable fields) and links to a new invoice
pre-filled from it. Editing a customer never changes an invoice already issued.
