# Payday TypeScript SDK

Zero-runtime-dependency, typed client for the Payday payments API. Requires Node.js 18+ (or another runtime with native `fetch`). Wire DTO fields intentionally use the API's canonical snake_case names.

```bash
npm install @payday/sdk
```

```ts
import { PaydayClient } from "@payday/sdk";

const payday = new PaydayClient({ apiKey: process.env.PAYDAY_API_KEY! });
const payment = await payday.payments.create({
  amount: "10.00",
  payout_address: "0x1111111111111111111111111111111111111111",
  expires_in: 3600,
}, crypto.randomUUID()); // caller-supplied idempotency key is mandatory

console.log(await payday.payments.get(payment.id));
console.log(await payday.payments.list({ status: "awaiting_payment", limit: 20 }));
```

Exactly `amount` settles to `payout_address`. The response's `recovery_address`
is the Payday recovery wallet the payment is committed to: overpayment
remainders, expired balances, and late transfers land there and are returned by
the operator after manual review. It is platform-configured, so a create request
carrying `refund_address` is rejected.

The client also provides long polling through `payments.get(id, { waitForChange: true })`, `payments.cancel`, `payments.transfers`, `status`, and `webhooks.add/list/remove/test/deliveries`. API failures throw `PaydayError`, exposing `code`, `status`, and `requestId`.

Set `baseUrl` in the constructor to target the sandbox or a local gateway. Never expose an API key in browser-delivered code.

## Building your own checkout

`PaydayPayerClient` reads the public routes behind a `payment_url`. It takes no API key and is safe to run in a browser: a payment link is open by design, because anyone holding it is allowed to fulfil the payment.

```ts
import { PaydayPayerClient } from "@payday/sdk";

const payer = new PaydayPayerClient();

const payment = await payer.payments.get("pay_0198f80c-8d2f-7dc1-a369-90556a64f700");
payment.remaining_base_units; // exact integer string — the only value to do arithmetic on
payment.payment_uri;          // EIP-681 request for the amount still due, or null
payment.payable;              // false once the address must stop being shown
payment.server_timestamp;     // render the deadline without trusting the payer's clock

payer.payments.qrUrl(payment.id); // <img src> for the QR; answers 410 once not payable
```

The response deliberately carries no merchant data — no payout or recovery address, no memo, reference, or metadata. Pass an `AbortSignal` to cancel a poll. If you build your own checkout, reproduce the guidance in [Payment safety](../../docs/payment-safety.md): payers must send the exact amount of the exact token on the exact chain, and must not pay at the deadline boundary.
