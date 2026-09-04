# payday.sh

The landing page, the hosted checkout at `/pay/{id}` (where every
`payment_url` points), and the merchant dashboard at `/dashboard`.

The checkout is built on Payday's own public payer API through
[`@payday/sdk`](../sdk/typescript). Those routes take no API key and expose no
merchant data, because a payment link is open by design: anyone holding it is
allowed to fulfil the payment. For a gated invoice the API withholds the
amount, parties, attachment, and address until the payer verifies, and the
page renders only what it was sent — nothing withheld enters the React tree.

The landing page's "Start Building" opens that same sign-in as a dialog, which
is all a sign-up is here: the API provisions an account on first sight of a
verified identity.

The dashboard uses the merchant API with a short-lived identity token obtained
from an emailed code, held in memory and this tab's `sessionStorage` only; no
API key exists in the browser. Its pages render per request as empty shells
and fetch everything client-side. See [docs/dashboard.md](../docs/dashboard.md).

## Running it locally

The gateway must be running (`just dev` from the repository root, which also
starts Anvil, the indexer, and the local identity provider), and
`PAYDAY_PUBLIC_BASE_URL` must point here so created payments link somewhere that
can render them.

```bash
npm ci                          # from the repository root
cp web/.env.example web/.env.local
just web                        # http://127.0.0.1:3002
```

Port 3002, because 3001 belongs to the local development identity provider.

```bash
npm run typecheck --workspace @payday/web
npm run lint      --workspace @payday/web
npm test          --workspace @payday/web

npx playwright install chromium          # once
npm run test:e2e  --workspace @payday/web
```

The browser suite runs against `e2e/stub-api.mjs`, a stand-in for the Payday
API. For the checkout the scenario is chosen by the payment id —
`/pay/pay_settled`, `/pay/pay_gated-email`, and so on. Because the checkout
renders on the server, intercepting in the browser would miss the first paint
entirely, so the app itself is pointed at the stub. For the dashboard the same
stub plays the merchant API behind a fake bearer check, the presigned upload
target, and the OTP issuer (code `123456`). It covers what unit tests cannot:
that the page hydrates, that polling moves the DOM on its own, that states
which must not offer an address really do not, that a gated invoice's withheld
fields are absent from both the HTML and the DOM, that a merchant can sign in —
from the landing page as well as the login page — upload a PDF, issue an
invoice, and download its proof, and that the CSP each route is served can
actually be satisfied. The sign-up dialog's five-minute resend window is driven
by Playwright's clock rather than waited out.

## Configuration

Every variable is `NEXT_PUBLIC_` and therefore inlined into the browser bundle.
Nothing secret belongs here — in particular `NEXT_PUBLIC_RPC_URL` must be a
public endpoint, never the operator RPC the gateway reads from Secrets Manager.
See [`.env.example`](.env.example). Missing values fail loudly at startup rather
than degrading silently, matching the gateway's own configuration convention.

The dashboard adds the issuer it signs in against (`NEXT_PUBLIC_AUTH0_DOMAIN`,
`NEXT_PUBLIC_AUTH0_CLIENT_ID`, `NEXT_PUBLIC_AUTH0_AUDIENCE`) and, because the
browser PUTs attachment bytes straight to object storage, the origin of the
presigned upload URL (`NEXT_PUBLIC_ATTACHMENT_UPLOAD_ORIGIN`) so the page's
Content-Security-Policy admits it — the local MinIO in development, the
attachment bucket's virtual-hosted URL in production.

## How it holds together

**One render on the server, then polling in the browser.** `app/pay/[id]/page.tsx`
fetches the payment server-side, so the page arrives with the amount, address,
QR and status already in it — no spinner, no layout shift, and it still reads
correctly with JavaScript disabled. `usePayment` then keeps it live by reading
the API directly from the browser; the payer routes are public and CORS-enabled,
so proxying through this app would add a hop and a second copy of the contract
without buying anything. The dashboard calls the merchant routes the same way,
which the gateway allows from one origin only — `PAYDAY_PUBLIC_BASE_URL`, so
this app must be served from exactly that origin.

**Polling is adaptive, and deliberately not long polling.** The person who just
paid is the case that has to feel instant, and it is the one case we can detect
precisely: their transaction receipt opens a 90-second window of 1.5s reads.
Everyone else is served by a few seconds, terminal states stop entirely, and a
background tab pauses. Holding open connections on an unauthenticated endpoint
would improve a case that is already handled. See `lib/poll.ts`.

**Two safety rules live in `lib/checkout-state.ts`, with tests.** The payer's
device clock never decides that a payment expired — chain time does, so a
countdown reaching zero moves the page to "the deadline has been reached" and
waits for the server. And payment instructions disappear the moment the payment
stops being payable, because funds sent afterwards route to the Payday recovery
wallet rather than back to the payer; the page tells payers to contact the
merchant and Payday support for return handling.

**A locked invoice is absent, not hidden.** `unlockedPayment` in
`lib/checkout-state.ts` narrows the payer response to the shape whose
mechanics are present; every component that renders an amount, address, QR, or
attachment takes that narrowed type, so a gated invoice's withheld content
cannot be rendered by accident and the page shows only the issuer, heading, and
masked mailbox until the API unlocks it. The attachment's signed URL is fetched
on click and never server-rendered.

**Amounts are never floats.** The API sends every amount twice, as a display
string and as integer `_base_units`. Arithmetic uses base units as `BigInt`;
`lib/format.ts` does the rest.

**The wallet transfer is verified before it is signed.** `WalletPay` checks the
payment's chain and token contract against this deployment's configured values,
then sends a plain ERC-20 `transfer` of `remaining_base_units` re-read at the
moment of signing. No approval, no contract call, nothing that can redirect
funds.

Wallets come from wagmi's EIP-6963 discovery, so the connect sheet lists what
the payer actually has installed. WalletConnect is added when
`NEXT_PUBLIC_WALLETCONNECT_PROJECT_ID` is set and covers phone wallets.

## Deployment

A Vercel project rooted at `web/`, serving `payday.sh`. Point the gateway at it
with the `checkout_base_url` Terraform variable; the gateway answers its own
`GET /pay/{id}` with a `301` here, so links shared before the checkout moved keep
working.
