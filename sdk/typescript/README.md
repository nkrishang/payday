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
  refund_address: "0x2222222222222222222222222222222222222222",
  expires_in: 3600,
}, crypto.randomUUID()); // caller-supplied idempotency key is mandatory

console.log(await payday.payments.get(payment.id));
console.log(await payday.payments.list({ status: "awaiting_payment", limit: 20 }));
```

The client also provides long polling through `payments.get(id, { waitForChange: true })`, `payments.cancel`, `payments.transfers`, `status`, and `webhooks.add/list/remove/test/deliveries`. API failures throw `PaydayError`, exposing `code`, `status`, and `requestId`.

Set `baseUrl` in the constructor to target the sandbox or a local gateway. Never expose an API key in browser-delivered code.
