# Stuck deposit request triage

A deposit request is not progressing through its lifecycle. The normal flow is:

```
created → funded → deploying → fulfilled
   │                    │
   └──► expired ◄───────┘──► recovered
```

- `created`: awaiting finalized USDC.
- `funded`: finalized credit reached the amount; queued for a helper transaction.
- `deploying`: claimed by the sweep worker; a helper transaction is in flight
  or being retried.
- `fulfilled`: the `Payment` contract paid the beneficiary exactly the deposit request
  amount; any overpayment remainder went back to the payer's attested wallet.
- `recovered`: the `Payment` contract paid the whole balance back to the
  payer's attested wallet (the deposit request expired before it could be settled).
- `expired`: chain time passed the deadline while the deposit request was open. Any
  balance at the address is recovered automatically; the status becomes
  `recovered` when that finalizes.
- `blocked`: the worker gave up on the deposit request; `blocked_reason` says why.

Funds that arrive after the contract exists are forwarded to the payer's
attested wallet automatically and never change the status. Every nonzero
return — overpayment remainder, expired balance, or late transfer — is a row
in `recovered_funds` (see "Reconciling returned funds" below). A deposit request
that never had its wallet bound has no address and can only wait or expire. The API reports
`received_base_units`, `execute_tx_hash`, `resolved_at_block`, and
`blocked_reason` for every deposit request.

## Step 1: Check the deposit request status

```bash
export PAYDAY_API_URL="https://api.payday.sh"
export PAYDAY_API_KEY="<API-key-for-the-account-that-created-the-deposit request>"
```

Then:

```bash
curl -s -H "Authorization: Bearer $PAYDAY_API_KEY" \
  "$PAYDAY_API_URL/v1/deposit-requests/<DEPOSIT_REQUEST_ID>" | python3 -m json.tool
```

Note the customer-facing `status`, `received_base_units`, `attention`, and
`address` from the response. Database queries below use the UUID portion after
the `dr_` prefix and expose internal lifecycle names intentionally.

## Step 2: Verify the USDC transfer landed on-chain

```bash
cast call 0x754704Bc059F8C67012fEd69BC8A327a5aafb603 \
  'balanceOf(address)(uint256)' <DEPOSIT_ADDRESS> \
  --rpc-url "$MONAD_RPC_URL"
```

If the balance is 0 and `received_base_units` is 0, the payer has not sent
USDC yet (or sent to the wrong address). The deposit request will remain in `created`
until a transfer is detected by the indexer.

## Step 3: Stuck in `created` (USDC was sent)

The indexer hasn't processed the block containing the transfer yet.

1. Check if the indexer is running — see [daily-monitoring.md](daily-monitoring.md).
2. Check indexer logs for errors:

   ```bash
   aws logs tail /ecs/payday/indexer --since 30m --region "$AWS_REGION" \
     | grep -E "WARN|ERROR|fatal|lagging"
   ```

3. If the `payday-indexer-cursor-lagging` alarm is raised the indexer is
   catching up; each pass drains up to `PAYDAY_INDEXER_MAX_RANGES_PER_TICK`
   ranges, so a backlog clears on its own. See
   [indexer-cursor-reset.md](indexer-cursor-reset.md) only if the cursor
   itself is wrong.

4. If the indexer is running, caught up, and still not detecting the transfer,
   verify the transfer actually exists by searching for USDC Transfer logs to
   the deposit address:

   ```bash
   RECIPIENT_TOPIC=$(python3 -c "print('0x' + '<DEPOSIT_ADDRESS>'.lower()[2:].zfill(64))")
   FROM=$((CURRENT_BLOCK - 100))
   curl -s -X POST "$MONAD_RPC_URL" \
     -H "Content-Type: application/json" \
     -d "{\"jsonrpc\":\"2.0\",\"method\":\"eth_getLogs\",\"params\":[{\"fromBlock\":\"$(python3 -c "print(hex($FROM))")\",\"toBlock\":\"latest\",\"address\":\"0x754704Bc059F8C67012fEd69BC8A327a5aafb603\",\"topics\":[\"0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef\",null,null,\"$RECIPIENT_TOPIC\"]}],\"id\":1}"
   ```

## Step 4: Stuck in `funded`, `expired`, or `deploying`

The deposit request is in the sweep queue. The worker submits one helper transaction
at a time, so check whether that pipeline is moving:

```bash
aws logs tail /ecs/payday/indexer --since 30m --region "$AWS_REGION" \
  | grep -E "helper transaction|sweep|batch|paused|blocked"
```

Possible causes:

- **KMS signer out of MON**: the `payday-indexer-signer-low-balance` alarm
  fires and submissions fail with an insufficient-funds error until the signer
  is funded. See [daily-monitoring.md](daily-monitoring.md) step 7.
- **Helper transaction unconfirmed**: the worker replaces it on the same nonce
  with fees bumped by 12.5% every `PAYDAY_SWEEP_PENDING_TIMEOUT_SECS`, up to
  `PAYDAY_SWEEP_MAX_SUBMISSIONS` times, then pauses and raises
  `payday-indexer-sweep-paused`. See "Sweep worker paused" below.
- **Transient item failure** (USDC paused, unknown revert): the deposit request stays
  `deploying` with `sweep_attempts` incrementing behind exponential backoff
  (2 s doubling, capped at 5 minutes). After `PAYDAY_SWEEP_MAX_ATTEMPTS` it
  becomes `blocked` with reason `retries_exhausted`.
- **Indexer crashed after funding**: restart it — see
  [service-restart.md](service-restart.md). The queue is durable; an in-flight
  batch is reconciled from its receipt on restart.

## Step 5: Status is `blocked`

The worker stopped trying. `blocked_reason` is one of:

| Reason | Meaning | Action |
|--------|---------|--------|
| `beneficiary_blacklisted` | Circle blacklisted the beneficiary | Agree a new destination with the merchant; after expiry the balance returns to the payer's wallet instead |
| `recovery_blacklisted` | Circle blacklisted the payer's attested wallet, the address's recovery term | Only the payer can resolve this with Circle. The wallet is committed into the address, so nothing can redirect the return; contact the merchant so they can reach the payer. An exact, on-time balance never touches the recovery term, so an exact deposit still settles |
| `payment_address_blacklisted` | Circle blacklisted the deposit address itself | Compliance escalation; nothing can move the funds |
| `balance_below_amount` | The chain balance is below the credited amount | Finalized history disagreed with the ledger; investigate the RPC provider before anything else |
| `retries_exhausted` | Repeated unclassified failures | Read the receipts of the batches in `sweep_batches` for this deposit request |
| `parameters_mismatch` | The row no longer derives its own deposit address | Database corruption or tampering; do not touch the funds until understood |
| `corrupt_row` | The row failed to decode | As above |

Once the cause is resolved, release the deposit request through the audited operator
API. The operator credential is distinct from merchant API keys; retrieve it
from Secrets Manager into an environment variable without printing it:

```bash
export PAYDAY_ADMIN_SECRET="$(aws secretsmanager get-secret-value \
  --secret-id payday/admin-bearer --query SecretString --output text)"
curl -fsS -X POST "$PAYDAY_API_URL/v1/admin/deposit-requests/<DEPOSIT_REQUEST_ID>/release" \
  -H "Authorization: Bearer $PAYDAY_ADMIN_SECRET" | jq
unset PAYDAY_ADMIN_SECRET
```

A terminal (`fulfilled`/`recovered`) deposit request can also carry a `blocked_reason`
when a *late* transfer could not be forwarded to the payer's wallet; clear
only the reason in that case.

The API atomically rejects unknown or already-released deposit requests, uses database
time to choose `expired` or `deploying`, and only clears the reason on terminal
late-transfer blocks. Do not use direct SQL for normal recovery.

The deposit page tells payers that payout is paused and their funds remain
safe, and asks them not to send a second deposit.

Configure merchant webhooks with `POST /v1/webhooks`; see
[`webhooks.md`](../webhooks.md) for signing and retry semantics. Gatewayd
snapshots verified email into a separate outbox and sends through SES. Set
`PAYDAY_NOTIFICATION_FROM_ADDRESS`; AWS credentials require `ses:SendEmail`.

## Reconciling returned funds

Overpayment remainders, expired balances, and late transfers go back on-chain
to the payer's attested wallet, the recovery term committed into every
deposit address. Payday holds nothing and returns nothing by hand. The ledger
is the record of what went back, where, and why:

```sql
-- See db-access.md
SELECT r.invoice_id, i.account_id, r.reason, r.amount,
       encode(r.transaction_hash, 'hex') AS tx, r.block_number, r.recovered_at
FROM recovered_funds r
JOIN invoices i ON i.id = r.invoice_id
ORDER BY r.recovered_at DESC;
```

One row per nonzero return, unique on `(invoice_id, transaction_hash,
reason)`, written in the same transaction that finalized the sweep; each row
also raised a `deposit_request.recovered_funds` webhook for the merchant. The
transaction named by the row is the return itself, into
`invoices.payer_wallet`. The one case needing a human is a payer who paid
from a wallet other than the one they attested (`likely_unsolicited_at` set):
the return went to the attested wallet, and the merchant may need to tell
them so.

## Sweep worker paused

`payday-indexer-sweep-paused` means the worker logged `sweep worker paused`
on every pass. Block indexing continues; only helper transactions stop. The
log line carries the reason:

- **Unconfirmed after N submissions**: every replacement of the batch's
  nonce failed to mine. Check the signer balance and the fee market. To
  resolve manually, send any transaction from the KMS key with that nonce and
  a higher fee (for example a zero-value self-transfer) — the worker then sees
  the nonce consumed, abandons the batch, and re-queues its deposit requests:

  ```bash
  export AWS_KMS_KEY_ID="$(terraform -chdir=infra output -raw kms_key_arn)"
  cast send "$(cast wallet address --aws)" --value 0 --nonce <NONCE> \
    --gas-price <HIGHER_FEE> --aws --rpc-url "$MONAD_RPC_URL"
  ```

- **No outcome for deposit request**: the finalized receipt of the helper transaction
  carries no event for a deposit request it should contain. This is an invariant
  violation; capture the transaction hash from the log and escalate.

The worker retries on the next pass, so once the condition clears the alarm
returns to OK on its own.
