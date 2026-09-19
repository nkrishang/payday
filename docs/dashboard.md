# Merchant dashboard

The dashboard at `gum.money/dashboard` is the browser face of the same API the
SDK uses. Gum is API-first: deposit requests, customers, attachments, and
webhooks are created and managed through the API (see `docs/api-reference.md`
and `docs/quickstart.md`). The dashboard creates none of them. It shows the
account, its money, and its key: the API key the merchant's own server calls
the API with, the balance of settled deposits and the flow that withdraws it to
a chain of the merchant's choice, the sign-in email, and every deposit,
read-only. It adds no rules of its own: every limit, policy check, and status
comes from the API.

## Signing in

The dashboard signs in through [Privy](https://privy.io) with an emailed
one-time code, from the landing page's "Start Building", which opens the
exchange in a dialog of Gum's own. There is no dashboard login page and no
separate registration: the API provisions an account the first time it sees
a Privy identity, so a first code creates the account and every later one
signs into it, and the same dialog does both. Every account gets an embedded
EVM wallet from Privy at that first sign-in; it is the merchant's own, and it
is where a deposit settles when the request's `payout_address` names it.

Anyone reaching a dashboard route without a live session — signed out, or
holding tokens Privy will no longer refresh — is sent to the landing page,
which is where the way back in is. Signing out does the same.

The session is Privy's identity token
([Authentication § 7](authentication.md#7-dashboard-sessions)). Privy's SDK
keeps it, and its refresh token, in the browser and refreshes it while the
merchant stays signed in; the dashboard reads the current token from the SDK
and sends it on every call. Nothing of Gum's stores a credential: **no API
key exists in the browser** — the API accepts the identity token directly on
the merchant routes, and maps it to the merchant account. Signing out asks
Privy to end the session.

Another code is offered a minute after the last one (`RESEND_COOLDOWN_MS`), so
a merchant is not sent into Privy's rate limit or left holding two live codes
wondering which one the page wants.

The web app is configured with the app it signs in to as
`NEXT_PUBLIC_PRIVY_APP_ID` (see `web/.env.example`), the same id `gum-server`
verifies sessions against. Locally that is the real development Privy app —
there is no stand-in — so the code arrives in a real mailbox, and
`http://127.0.0.1:3002` must be among the app's allowed domains.

Dashboard pages are rendered per request as empty shells: no deposit request,
customer, or token is in server-rendered HTML or any build artifact, and every
page is marked `noindex`.

The dashboard calls the API from the browser with `GET`, `POST`, `PATCH`,
`PUT`, and `DELETE`, so `gum-server` answers cross-origin requests on the
merchant routes from exactly one origin: the
web origin it is configured with as `GUM_PUBLIC_BASE_URL`. The dashboard
must be served from that origin — `http://127.0.0.1:3002` locally,
`https://gum.money` in production; from any other origin every request fails
its preflight. The payer checkout has no such constraint, because the payer
routes allow any origin.

## The dashboard page

Signing in lands on `/dashboard`, which is the whole dashboard, top to bottom:
the API key, the account with its balances and the withdraw flow, and the
deposit list. There are no section tabs to click through first — the only
other dashboard page is the detail of one deposit.

## API key

The top of the page is the **API key** section: the key the merchant's own
server calls the API with. It generates, rolls, and revokes the key, with one
confirmation and no second sign-in: the session is the credential, and the API
refuses these routes to an API key on its own. Generate shows the key exactly
once; roll gives a replacement while the previous key keeps working for 24
hours (`previous_key_expires_at` reports the window); revoke invalidates the
current key and any key in its grace window at once.

## Account, balances, and withdrawals

The middle of the page is the account itself. It shows the mailbox the
merchant signed in with and their Gum wallet — the embedded EVM wallet
Privy created for the account — in full, ready to copy or open in each
network's explorer, with its balance of every stablecoin the network serves
and its gas balance on every supported network read straight from the public
RPCs (`NEXT_PUBLIC_CHAINS`), and a Sign out. The wallet is the same address on
every chain. It shows what a deposit pays it: a request settles to the
`payout_address`
named when it is created, so balances and withdrawals here cover exactly the
requests whose payout address is this wallet's address — an integration that
pays some other address collects there, and its deposits still appear in the
table below. A wallet that Privy is still creating shows as such with a *Check
again*; the API records it as soon as a session carries it.

**Change**, beside the sign-in email, moves the account to a new mailbox: a
code is sent to the new address through Privy's own flow, and the email
changes only once the merchant enters it. Until then the old address is still
the signed-in one, and the session, the account, and the API key are
untouched — the email is only how the merchant gets in.

**Withdraw**, beneath the balances, moves one currency to one address the
merchant names, on a chain of their choice. For USDC it moves everything the
wallet holds on every network: the API snapshots the balances into one leg per
network, the merchant signs each leg's EIP-712 authorization with the wallet
Privy holds (no gas, no delegation), and the relayer carries the legs to the
destination while the panel tracks them; funds on another network cross
through Circle's CCTP, which takes seconds from Monad and about twenty minutes
from Base or Arbitrum. USDT has no 1:1 bridge, so a USDT withdrawal is a
single leg moving the destination network's balance; USDT held on another
network is withdrawn separately to an address there. **Export wallet key**
shows the wallet's key once, through Privy's own dialog, for a merchant who
wants to withdraw from their own server (`docs/api-reference.md`,
Withdrawals).

## Deposits

**List** (on `/dashboard`). Its own section: heading, one line of what it
holds, then the filters, then the table. An account with nothing issued sees
the same table with its columns and an empty row, not a different layout. Five
rows at a time, with Previous/Next shown only when there is another page.
Filters for status, verification, and the customer sit above the table, and
each is the API's own parameter, so a filter narrows the query rather than the
page. A row carries the `issuer_id` the request was created with, the amount
with the token's mark, and opens its detail when clicked. Every deposit shows
its heading (or reference), the payer, the amount, the deposit request status,
the payer-policy mode, the verification state, and a paperclip when a PDF is
attached. The status filter and the Previous/Next controls are the API's own
`status` and `starting_after` parameters.

The list is read-only: there is no action on it, and no composer. Issuing a
deposit request is the API's job — see `docs/quickstart.md` — and requests
issued that way appear here like any other.

**Detail** (in the row itself). A row opens in place rather than navigating:
the list is where a merchant looks, and leaving it to read one request meant
re-filtering and re-paging to come back. One row is open at a time — the
detail is tall enough that two would make the table hard to read — and a
request is only fetched once its row has been opened. `/dashboard/deposits/{id}`
renders the same read-only detail as its own page, so links already sent still
land somewhere useful.

The open row leads with the two questions a list cannot answer, drawn rather
than written: a bar for how much of the amount has arrived, and a three-point
rail — issued, funded, then settled or however it ended — for how far through
its life the request is, each point carrying how long ago it happened. Nothing
the summary row already shows is repeated. Under that, grouped by what a
merchant came for:

- *Document*: only what the row omits — the payer's address and details,
  the notes, a link to the saved customer, and the `issuer_id` the request
  carries;
- *Verification* (gated requests only): the policy mode with the merchant's own
  assertion (the expected email), and the verification verdict — separate from
  the deposit request status, because a gated request can be funded before its
  payer has verified. The activity behind it follows once there is any: every
  attempt the payer made, with its status (code sent, approved, or abandoned)
  and time. The payer's session and the code itself are never shown, because
  the API never sends them;
- *Deposit*: the one-time address, the payout address, the network and token
  (the networks offered until the payer chooses, then the chosen one), the
  funded time, the settlement transaction, and any operator attention message.
  Addresses and the settlement hash are shown in full and link to the chosen
  chain's explorer (`explorerUrl` in `NEXT_PUBLIC_CHAINS`) — the whole address
  is what a merchant compares against a wallet, so a column too narrow for it
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
  the deterministic deposit request PDF (`GET /v1/deposit-requests/{id}/request.pdf`),
  and the Proof of Payment as JSON (`GET /v1/deposit-requests/{id}/proof`),
  which becomes available once the request settles and verifies offline
  (`gum_core::verify_proof`).

Issued deposit requests are immutable; a different amount, party, policy, or
attachment means a new deposit request.

The dashboard wears the landing page's palette and type in both colour
schemes. `.dash` in `web/app/globals.css` redefines the theme tokens
(`--canvas`, `--surface`, `--ink`, the `--color-*` Tailwind reads, and
`--logo-accent`) rather than restyling the pages, so every page below it
follows; a page that reads a token needs no change to sit on the dark ground.
The header is the landing page's, unchanged: the same wordmark at the same
size, the same Docs and Pricing links on the same 76px rule, plus Sign out
(which the Account section repeats).

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
