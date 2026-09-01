"use client";

import * as Tabs from "@radix-ui/react-tabs";
import { useState } from "react";
import { CopyButton } from "@/components/ui/copy-button";
import { cn } from "@/lib/cn";

const SNIPPETS = [
  {
    id: "cli",
    label: "CLI",
    code: `payday create \\
  --amount 25.00 \\
  --to 0x1111111111111111111111111111111111111111 \\
  --expires-in 1h \\
  --memo 'Order 1042'

payday get <PAYMENT-ID> --watch`,
  },
  {
    id: "sdk",
    label: "TypeScript",
    code: `import { PaydayClient } from "@payday/sdk";

const payday = new PaydayClient({ apiKey: process.env.PAYDAY_API_KEY! });

const payment = await payday.payments.create(
  {
    amount: "25.00",
    payout_address: "0x1111111111111111111111111111111111111111",
    expires_in: 3600,
  },
  crypto.randomUUID(), // idempotency key is mandatory
);

// Hand this to the payer. Nothing else is needed.
console.log(payment.payment_url);`,
  },
  {
    id: "curl",
    label: "curl",
    code: `curl -fsS "$API/v1/payments" \\
  -H "Authorization: Bearer $PAYDAY_API_KEY" \\
  -H 'Content-Type: application/json' \\
  -H "Idempotency-Key: order-1042" \\
  -d '{
    "amount": "25.00",
    "payout_address": "0x1111111111111111111111111111111111111111",
    "expires_in": 3600
  }'`,
  },
] as const;

export function Integrate() {
  const [active, setActive] = useState<string>(SNIPPETS[0].id);
  const current = SNIPPETS.find((snippet) => snippet.id === active) ?? SNIPPETS[0];

  return (
    <Tabs.Root value={active} onValueChange={setActive}>
      <div className="flex items-center justify-between gap-3">
        <Tabs.List className="inline-flex rounded-[10px] border border-line bg-surface p-0.5">
          {SNIPPETS.map((snippet) => (
            <Tabs.Trigger
              key={snippet.id}
              value={snippet.id}
              className={cn(
                "rounded-[8px] px-3 py-1.5 text-[13px] font-medium transition-colors",
                active === snippet.id
                  ? "bg-inverse text-inverse-ink"
                  : "text-muted hover:text-ink",
              )}
            >
              {snippet.label}
            </Tabs.Trigger>
          ))}
        </Tabs.List>
        <CopyButton value={current.code} label="snippet" />
      </div>

      {SNIPPETS.map((snippet) => (
        <Tabs.Content key={snippet.id} value={snippet.id} className="mt-4">
          <pre className="overflow-x-auto rounded-[16px] border border-line bg-surface px-5 py-5 font-mono text-[12.5px] leading-[1.75] text-muted">
            <code>{snippet.code}</code>
          </pre>
        </Tabs.Content>
      ))}
    </Tabs.Root>
  );
}
