# Stuck deposit request triage

A deposit request is not progressing through its lifecycle. Internal states
are `created`, `funded`, `fulfilled`, `recovered`, and `expired`. Membership in
an open execution is represented separately by `sweep_job_id`; manual review
by `attention_reason`. The public API reports `needs_attention` when the latter
is set. See [`docs/architecture.md`](../architecture.md) for the full flow.

## Step 1: Check the deposit request status

```bash
curl -s -H "Authorization: Bearer $GUM_API_KEY" \
  "$GUM_API_URL/v1/deposit-requests/<DEPOSIT_REQUEST_ID>" | python3 -m json.tool
```

Note `status`, `received_base_units`, `attention`, and `address`. Database
queries use the UUID after the `dr_` prefix.

```sql
SELECT id, status, sweep_job_id, sweep_attempts, attention_reason
FROM invoices WHERE id = '<DEPOSIT_REQUEST_UUID>';
```

## Step 2: Verify the transfer landed on-chain

```bash
cast call <TOKEN_ADDRESS> 'balanceOf(address)(uint256)' <DEPOSIT_ADDRESS> \
  --rpc-url "$MONAD_RPC_URL"
```

If the balance and `received_base_units` are zero, the payer has not sent the
request's stablecoin, or used the wrong network or asset. See
[wrong-network-deposit.md](wrong-network-deposit.md).

## Step 3: Stuck in `created` (the stablecoin was sent)

Check the `indexer` service and `/ecs/payday/indexer` logs. A
`finality violation; chain halted` requires the chain-halt flow in
[indexer-fatal-halt.md](indexer-fatal-halt.md). Provider failures and a growing
cursor lag point to [quicknode-rpc-limits.md](quicknode-rpc-limits.md). The
cursor is owned by gum-server in `indexer_cursor`; do not restart the indexer
merely to refresh it.

## Step 4: Stuck in the sweep pipeline

Gum-server schedules a `sweep_jobs` row and command; gum-signers owns signing,
replacement, and reconciliation. Follow one request by correlation id:

```sql
SELECT sj.id, sj.chain_id, sj.correlation_id, sj.created_at,
       sj.submitted_tx_hash, sj.resolution, sj.resolved_at,
       sj.resolution_detail
FROM sweep_jobs sj JOIN invoices i ON i.sweep_job_id = sj.id
WHERE i.id = '<DEPOSIT_REQUEST_UUID>';

SELECT * FROM bus.messages
WHERE correlation_id = '<CORRELATION_ID>' ORDER BY published_at;

SELECT j.id, j.state, j.attempts, j.next_attempt_at, j.last_error,
       j.command, j.resolution
FROM execution.jobs j WHERE j.correlation_id = '<CORRELATION_ID>';

SELECT t.* FROM execution.transactions t
JOIN invoices i ON i.sweep_job_id = t.job_id
WHERE i.id = '<DEPOSIT_REQUEST_UUID>';
```

Search `/ecs/payday/api`, `/ecs/payday/indexer`, and `/ecs/payday/signers` for
the same `correlation_id`. Common causes:

- `signer balance is low`: fund the signer named in
  `execution.signer_status`; the `payday-signers-low-balance` alarm identifies
  the condition.
- `broadcast failed; the signed transaction stays durable for the next pass`:
  the signed attempt remains in `execution.transaction_attempts` and retries.
- `job deferred after a transient failure` or `reconciling transaction failed`:
  inspect `last_error`; retry and reconciliation continue.
- `transaction unconfirmed after the replacement limit; fees are no longer
  raised`: `payday-signers-stalled` fires and an `ExecutionStalled` event is
  published. The lane is not paused; reconciliation continues every pass.
- A dead bus delivery: list it with `gum-server bus dead [limit]`, correct the
  cause, then run `gum-server bus retry <message-id>` in a one-off `api` task.

## Step 5: Status is `needs_attention`

`attention_reason` includes:

| Reason | Meaning / action |
|--------|------------------|
| `beneficiary_blacklisted` | Agree a valid destination with the merchant. |
| `recovery_blacklisted` | The payer must resolve the issuer restriction. |
| `payment_address_blacklisted` | Compliance escalation; funds cannot move. |
| `balance_below_amount` | Investigate disagreement between chain balance and finalized history. |
| `retries_exhausted` | Inspect the execution job, transactions, attempts, and receipts. |
| `settlement_unknown` | Establish the transaction outcome before release. |

Once the cause is resolved, use the audited release endpoint; do not clear the
column directly:

```bash
export GUM_ADMIN_SECRET="$(aws secretsmanager get-secret-value \
  --secret-id payday/admin-bearer --query SecretString --output text)"
curl -fsS -X POST "$GUM_API_URL/v1/admin/deposit-requests/<DEPOSIT_REQUEST_ID>/release" \
  -H "Authorization: Bearer $GUM_ADMIN_SECRET" | jq
unset GUM_ADMIN_SECRET
```

## Reconciling returned funds

Overpayment remainders, expired balances, and late transfers return on-chain
to the payer's attested wallet. The ledger records them:

```sql
SELECT r.invoice_id, i.account_id, r.reason, r.amount,
       encode(r.transaction_hash, 'hex') AS tx, r.block_number, r.recovered_at
FROM recovered_funds r
JOIN invoices i ON i.id = r.invoice_id
ORDER BY r.recovered_at DESC;
```

## Stalled execution lane

Inspect every signed replacement and the durable lane before intervening:

```sql
SELECT t.*, a.tx_hash, a.replacement_number, a.broadcast_at
FROM execution.transactions t
LEFT JOIN execution.transaction_attempts a ON a.transaction_id = t.id
WHERE t.job_id = '<SWEEP_JOB_ID>'
ORDER BY a.replacement_number;
```

The open lane is unique per `(chain_id, signer)`. After
`GUM_SWEEP_PENDING_TIMEOUT_SECS`, signers raise fees up to
`GUM_SWEEP_MAX_SUBMISSIONS`; after that they stop raising fees but continue
reconciliation. Do not alter the nonce lane or send from its KMS key until the
existing attempts and canonical nonce are understood.
