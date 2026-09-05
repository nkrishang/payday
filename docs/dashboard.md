# Merchant dashboard

The dashboard at `payday.sh/dashboard` is the browser face of the same API the
SDK uses. It issues deposit requests, keeps customers, uploads the one PDF a
deposit request may carry, and shows what happened to each deposit request. It adds no rules
of its own: every limit, policy check, and status comes from the API, and the
form only gives immediate feedback (a missing name, a file that is not a PDF)
before the API has the final word.

## Signing in

The dashboard signs in through [Privy](https://privy.io) with an emailed
one-time code, from the landing page's "Start Building", which opens the
exchange in a dialog of Payday's own. There is no dashboard login page and no
separate registration: the API provisions an account the first time it sees
a Privy identity, so a first code creates the account and every later one
signs into it, and the same dialog does both. Every account gets an embedded
EVM wallet from Privy at that first sign-in; it is the merchant's own, and it
is where their deposit requests settle unless they choose otherwise.

Anyone reaching a dashboard route without a live session — signed out, or
holding tokens Privy will no longer refresh — is sent to the landing page,
which is where the way back in is. Signing out does the same.

The session is Privy's identity token
([Authentication § 7](authentication.md#7-dashboard-sessions)). Privy's SDK
keeps it, and its refresh token, in the browser and refreshes it while the
merchant stays signed in; the dashboard reads the current token from the SDK
and sends it on every call. Nothing of Payday's stores a credential: **no API
key exists in the browser** — the API accepts the identity token directly on
the deposit request, customer, attachment, and account routes, and maps it to the
merchant account. Signing out asks Privy to end the session.

Another code is offered a minute after the last one (`RESEND_COOLDOWN_MS`),
so a merchant is not sent into Privy's rate limit or left holding two live
codes wondering which one the page wants.

The web app is configured with the app it signs in to as
`NEXT_PUBLIC_PRIVY_APP_ID` (see `web/.env.example`), the same id `gatewayd`
verifies sessions against. Locally that is the real development Privy app —
there is no stand-in — so the code arrives in a real mailbox, and
`http://127.0.0.1:3002` must be among the app's allowed domains.

Dashboard pages are rendered per request as empty shells: no deposit request, customer,
or token is in server-rendered HTML or any build artifact, and every page is
marked `noindex`.

The dashboard calls the API from the browser with `GET`, `POST`, `PATCH`,
`PUT`, and `DELETE`, so `gatewayd` answers cross-origin requests on the
merchant routes from exactly one origin: the
web origin it is configured with as `PAYDAY_PUBLIC_BASE_URL`. The dashboard
must be served from that origin — `http://127.0.0.1:3002` locally,
`https://payday.sh` in production; from any other origin every request fails
its preflight. The payer checkout has no such constraint, because the payer
routes allow any origin.

## The dashboard page

Signing in lands on `/dashboard`, which is the whole dashboard: the deposit request
requests, the customers they are addressed to, and the flow that issues a new one.
There are no section tabs to click through first — the only other dashboard
pages are the detail of one record and the two long forms, and the header's
wordmark and a back link return from them.

A merchant who has just signed up is not shown a description of the product;
they are put to work on the first thing it needs from them. Nothing can be
issued until an **issuer identity** exists, so with neither an identity nor a
request the page *is* that setup, and issuing follows straight out of it. Once
anything has been issued the same page carries the deposit request list, the identities,
and the customer list in full, filters and cursors included.
`/dashboard/deposits` and `/dashboard/customers` redirect here.

Setting up gates issuing, never looking: an account that issued through the
API still sees what it has, and only the "New deposit request" action diverts
into setup.

## Account and wallet

The foot of `/dashboard` is the account itself. The **Account** section shows
the mailbox the merchant signed in with and their Payday wallet — the
embedded EVM wallet Privy created for the account — in full, ready to copy or
open in the explorer, with its USDC and gas balance on the deployment's chain
read straight from the public RPC (`NEXT_PUBLIC_RPC_URL`), and a Sign out. A
wallet that Privy is still creating shows as such with a *Check again*; the
API records it as soon as a session carries it. The **API key** section
below it generates, rolls, and revokes the key the merchant's own server uses,
with one confirmation and no second sign-in: the session is the credential,
and the API refuses these routes to an API key on its own.

## Issuer identities

An identity is the merchant's own side of a deposit request — the party it is issued
under and the address payers write to — saved once instead of retyped. It
answers what the composer used to ask as free text. Where a request settles
is not part of it: that is the account's Payday wallet by default, and an
identity may keep saved wallets as alternatives.

**Setting one up** takes two steps, in place on `/dashboard`: the name and
contact address, then the code. The whole form is on the page, with one
action in a footer that belongs to the form rather than to any section card —
*Send code*, *Confirm code*. The footer sticks to the bottom of the viewport
while the form runs past the fold and settles at its end, so the action is
always to hand without ever appearing to belong to the card beside it; it
names the section it is acting on, and finishing one scrolls the next into
view. The contact address is **proven with an emailed code** (through Auth0,
the same exchange payers use) before any deposit request can carry it: payers are
told to write there, so Payday does not take a merchant's word for the mailbox
any more than it takes a payer's. The flow resumes from whatever the account
already holds, so an abandoned tab reopens at the step that is unfinished
rather than the beginning. An identity counts as usable once its mailbox is
proven.

**Managing them** is a section of the same page: a line per identity — the
name, whether it can be issued under, its contact address, and whether it
settles to the Payday wallet or has saved wallets of its own — that opens onto
the rest. An open row *is* its form, with no Edit step, and one Save commits
the whole row: rename it, move its contact address (which drops the proof,
because a different mailbox is a different claim), and attach or drop saved
wallets. Save lights up only once something differs from what is stored, takes
the API's own answer as the new baseline when it lands, and Cancel puts it all
back. Dropping the last saved wallet simply leaves the identity on the Payday
wallet. A wallet is saved once per account and may serve several identities;
an identity may keep several.

An identity's name is unique within the account, case and surrounding space
included, because two identities called the same thing are the same row to
whoever reads a list or a picker. Both forms refuse a name already taken before
the request is made, and the column refuses it regardless. Contact addresses may
repeat — two identities can share a support mailbox.

Issuance is unchanged by any of this. `POST /v1/deposit-requests` still takes the party
and the payout address inline and snapshots them, so editing an identity never
reaches a deposit request already issued — the same rule customers follow. What the
identity does is stop the retyping and establish the mailbox.

It also leaves a durable handle: a request records the identity's `issuer_id`
immutably, so "these requests were issued under that identity" stays answerable
after the identity is renamed, moved to another mailbox, or pointed at
different wallets. An identity that requests were issued under cannot be
deleted.

The header is the landing page's, unchanged: the same wordmark at the same
size, the same Docs and Pricing links on the same 76px rule, plus Sign out
(which the Account section repeats).

**Creating one, in the page.** The action does not navigate and does not open a
modal: the empty state gives way to a four-step composer — the amount and
deadline, the payer, the payer policy, then a review — and the issued link
takes its place when the API answers. A preview sits beside the steps
throughout, filling in as they are answered; it is the review, and each filled
row leads back to the step that set it. The body is built by
`buildCreateDepositRequest` in `web/components/dashboard/create-deposit-request.ts`, and the
last screen states plainly that an issued request is immutable.

There is no second, longer form. The composer asks everything
`POST /v1/deposit-requests` takes, so nothing is a link away.

The dashboard wears the landing page's palette and type in both colour schemes.
`.dash` in `web/app/globals.css` redefines the theme tokens (`--canvas`,
`--surface`, `--ink`, the `--color-*` Tailwind reads, and `--logo-accent`)
rather than restyling the pages, so every page below it follows; a page that
reads a token needs no change to sit on the dark ground.

## Deposit requests

**List** (on `/dashboard`). Its own section, shaped like the identities and
customers sections beside it: heading, one line of what it holds, and the *New*
control on the same row, then the filters, then the table. An account with
nothing issued sees the same table with its columns and an empty row, not a
different layout. Five rows at a time, with Previous/Next shown only when there
is another page. Filters for status, verification, and the customer sit above the table beside the *New* control, and each is the API's own
parameter, so a filter narrows the query rather than the page. A row carries the
issuer identity it was issued under as a badge, the amount with the token's
mark, and opens its detail when clicked. Every deposit request shows its heading (or reference), the payer, the
amount, the deposit request status, the payer-policy mode, the verification state, and a
paperclip when a PDF is attached. The status filter and the Previous/Next
controls are the API's own `status` and `starting_after` parameters.

**New deposit request** (in place on `/dashboard`). The only way to issue one,
in four steps with a running preview beside them that doubles as the review.
The identity is chosen from what the merchant set up and preselected when
there is one; the request settles to the account's Payday wallet unless the
identity has saved wallets, in which case they are offered beside it with the
Payday wallet still the default — so the common case is no clicks at all. A
customer's own page links here with `?customer=`, which opens the composer on
that customer.

1. *Amount*: the amount in USDC, used directly (there are no line items), the
   identity when there is more than one and the destination when the identity
   offers more than one, and the deadline — 24 hours, 7 days, 30 days, or a moment picked from a date
   and time control. A preset is sent as `expires_in`; a picked moment is sent
   as `expires_at`, because converting it to a duration would re-anchor it to
   whenever the request arrived. The window the API accepts — at least ten
   minutes out, at most 366 days — is checked here too, to save a round trip.
2. *Payer*: a customer picker that pre-fills the payer from a saved
   customer and links the request to it (the request still stores its own
   snapshot), the payer typed fresh otherwise — saved as a customer on
   issue — what the request is for, a reference, notes, and the attachment.
3. *Verification*: the payer policy.
4. *Review*: every value as it will be sent.

The whole document, exactly as `POST /v1/deposit-requests` takes it:

- issuer and payer parties (name, optional email, optional free-text
  details, rendered verbatim on the deposit request);
- heading, reference, and notes;
- one PDF attachment, up to 5 MiB. The file goes straight to object storage
  with the API's presigned headers, then the dashboard polls finalization and
  shows the attachment's own stages: *uploading*, *scanning* (the malware scan
  has not reported), then *ready* with its size and SHA-256 — or *rejected*
  with the API's reason;
- the payer policy: `permissionless` or `verified_email`, with the expected
  email for `verified_email`. The expected email is required for that mode,
  says so on its label, and arrives pre-filled from the payer's address
  — following it until the merchant types their own, after which it is theirs.
  The third mode, `merchant_session`, is not offered here: it needs an
  application that has signed the payer in and can hand them the client
  secret, so it is created through the API. Requests issued that way still
  appear in the dashboard, with the app's payer reference and an "Opened by
  your app" entry in the verification activity.

Issued deposit requests are immutable; a different amount, party, policy, or
attachment means a new deposit request.

**Detail** (in the row itself). A row opens in place rather than navigating:
the list is where a merchant works, and leaving it to read one request meant
re-filtering and re-paging to come back. One row is open at a time — the detail
is tall enough that two would make the table hard to read — and a request is
only fetched once its row has been opened. `/dashboard/deposits/{id}` redirects
here, so links already sent still land somewhere useful.

The open row leads with the two questions a list cannot answer, drawn rather
than written: a bar for how much of the amount has arrived, and a three-point
rail — issued, funded, then settled or however it ended — for how far through
its life the request is, each point carrying how long ago it happened. Nothing
the summary row already shows is repeated. Under that, grouped by what a
merchant came for:

- *Document*: only what the row omits — the payer's address and details,
  the notes, and a link to the saved customer;
- *Verification* (gated requests only): the policy mode with the merchant's own
  assertion (the expected email), and the verification verdict — separate from
  the deposit request status, because a gated request can be funded before its payer
  has verified. The activity behind it follows once there is any: every
  attempt the payer made, with its status (code sent, approved, or abandoned)
  and time. The payer's session and the code itself are never shown, because
  the API never sends them;
- *Deposit*: the one-time address, the payout address, network and token, the
  funded time, the settlement transaction, and any operator attention message.
  Addresses and the settlement hash are shown in full and link to the
  configured explorer (`NEXT_PUBLIC_EXPLORER_BASE_URL`) — the whole address is
  what a merchant compares against a wallet, so a column too narrow for it
  breaks the line rather than hiding characters. A deployment without an
  explorer, such as a local chain, renders them as plain text rather than as
  links to a page that does not exist;
- *Recovered funds*, when there are any;
- *Payer's view*: what the payer sees, read through the public payer route with
  no payer session — so a gated request shows exactly what an unverified
  visitor would, and a merchant can check that before sending the link. The
  shareable link sits with it, ready to copy. It is a preview, not the
  checkout: no pay button, because the merchant is not the one paying;
- *Files*: the attached PDF through a short-lived signed URL fetched on demand,
  the deterministic deposit request PDF (`GET /v1/deposit-requests/{id}/request.pdf`), and the
  Proof of Payment as JSON (`GET /v1/deposit-requests/{id}/proof`), which becomes
  available once the request settles and verifies offline
  (`gateway_core::verify_proof`).


## Verification and recovery indicators

Two indicators sit beside the deposit request status and mean different things.

**Verification** — *Not required* for permissionless deposit requests; *Pending* until
the gateway records that the expected payer completed the policy's checks;
*Verified*, with the completion time, afterwards.

**Payer wallet** — the wallet the payer signed the request's attestation
with, shown on the detail page with the address once it exists (the address
is created from that signature; before it, the row says so). Only that
wallet's transfers are the payer's.

**Likely unsolicited** — shown, with the time, when finalized funds arrived
from a wallet other than the attested one. The funds still count and settle;
the flag says the attested wallet did not pay them, and no Proof of Payment
will claim it did.

**Returned to the payer** — a section on the detail page, present only when
something went back to the payer's attested wallet rather than the payout
address: the overpayment remainder on a settled deposit request, the full balance of
a returned one, and every transfer the indexer classified as late, each with
its transaction hash and whether it has been collected. Returns are on-chain
and automatic.

## Customers

A customer is a reusable counterparty record: name, optional email, optional
details. The list, below the deposit requests on `/dashboard`, pages through
`GET /v1/customers`; a customer's page edits it
(`PATCH`, a full replacement of the editable fields) and links to a new deposit request
pre-filled from it. Editing a customer never changes a deposit request already issued.
