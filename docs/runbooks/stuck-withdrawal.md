# Stuck withdrawal

A merchant's withdrawal (`wd_…`) has a leg that is not progressing, or has
failed, and the merchant asks where their funds are. Read the leg's state
first; each state says exactly which party is next and where the funds are.
The withdrawal's `currency` decides which legs exist: a USDC withdrawal may
bridge through CCTP, a USDT withdrawal moves the destination network's
balance only and has no bridge stage, so its leg goes `authorized →
relaying → completed` and never reaches `burned`, `attested`, or `minting`.
Every same-chain leg, USDC or USDT0, is an EIP-3009
`TransferWithAuthorization` the relayer submits to the leg's `token`.

## Where the funds are, by leg state

| Leg state | Funds are | Who acts next |
|---|---|---|
| `awaiting_signature` | In the Payday wallet, untouched | The merchant (sign, or cancel). Expires 24 h after creation. |
| `authorized` | In the Payday wallet, untouched | gum-server orchestrates the next `withdrawal_step`. |
| `relaying` | Moving: a `transferWithAuthorization` or (USDC only) `WithdrawalForwarder.bridge` is in flight | gum-signers: receipt, fee replacement, or reconciliation. |
| `burned` | Burned on the source chain; Circle owes the mint | Circle's attestation service (Iris). Monad: seconds. Base/Arbitrum: ~15–19 minutes. |
| `attested` | Burned; attestation stored on the leg | gum-server orchestrates `receiveMessage` on the destination chain. |
| `minting` | The mint is in flight on the destination chain | gum-signers: receipt or fee replacement. |
| `completed` | At the destination address | Nobody. |
| `failed` | Wherever the last successful step left them; `failure_reason` says which | An operator, if the reason is not the merchant's to fix. |
| `expired` | In the Payday wallet, untouched | The merchant: create a new withdrawal. |

Nothing Payday runs can send a leg's funds anywhere but the destination the
merchant signed: a transfer leg's authorization names the destination, a
bridge leg's (USDC only) names the forwarder and commits to the destination
through its nonce, and CCTP mints to the recipient inside Circle's message.

## Look

```bash
psql "$DATABASE_URL" -c "
  SELECT l.id, l.kind, l.state, l.source_chain_id, l.destination_chain_id, l.amount, w.currency,
         l.valid_before, l.step_chain_id, '0x' || encode(l.step_signer, 'hex') AS step_signer,
         l.step_nonce, cardinality(l.step_tx_hashes) AS submissions,
         l.step_submitted_at, l.attestation_next_check_at, l.failure_reason,
         encode(l.burn_tx_hash, 'hex') AS burn_tx, encode(l.mint_tx_hash, 'hex') AS mint_tx
  FROM withdrawal_legs l JOIN withdrawals w ON w.id = l.withdrawal_id
  WHERE w.id = '<withdrawal uuid>' ORDER BY l.position"
```

Each executable step has an `execution.jobs` row whose `command` kind is
`withdrawal_step`. Obtain the step job id from the leg, then inspect it and its
lane:

```sql
SELECT * FROM execution.jobs WHERE id = '<STEP_JOB_ID>';
SELECT * FROM execution.transactions WHERE job_id = '<STEP_JOB_ID>';
```

Use `execution.executor_status`, `execution.signer_status`, and
`GET /v1/status` for health. A stalled step follows the same replacement and
continuing-reconciliation behavior as a sweep lane; see
[stuck-deposit-request.md](stuck-deposit-request.md#stalled-execution-lane).

## `burned` for longer than expected

Ask Circle directly, with the source chain's CCTP domain (Monad 15, Base 6,
Arbitrum 3):

```bash
curl -fsS "https://iris-api.circle.com/v2/messages/15?transactionHash=0x<burn tx>" | jq
```

`404` means Iris has not indexed the burn yet; `pending_confirmations`
means the source chain is not final in Circle's eyes yet (an L2 waits for
~65 Ethereum blocks); `complete` means gum-server will pick it up on its
next poll (`attestation_next_check_at`). A burn that Iris never completes
after an hour is a Circle incident; the leg stays `burned` and no funds are
at risk, since only the attested message can mint them.

## `attested` or `minting` for longer than expected

The destination chain's execution job is responsible. Check its transaction
lane and signer balance. If the mint must be done by hand, anyone may submit
it from any funded key, and reconciliation will observe the result:

```bash
psql "$DATABASE_URL" -Atc "SELECT encode(attestation_message,'hex'), encode(attestation,'hex') FROM withdrawal_legs WHERE id = '<leg uuid>'"
cast send 0x81D40F21F12A8F0E3252Bccb954D722d4c464B64 \
  'receiveMessage(bytes,bytes)' 0x<message> 0x<attestation> \
  --rpc-url "$DESTINATION_RPC_URL" --account payday-ops
```

## `failed`

`failure_reason` names the reverted step. A leg is not failed the first time
a step reverts: while its obligation survives — an authorization nobody has
consumed, an attestation Circle has already signed — the relayer re-queues
it on a backoff that doubles with every consecutive revert, starting at
30 s (`step_reverts` counts, `step_retry_at` says when it is next eligible),
and only the fifth revert in a row is terminal. The count resets when a step
finally succeeds. So a `failed` leg has reverted five times or hit a
condition no retry fixes.

A `relaying` step whose signer nonce was spent without a visible receipt keeps
its durable transaction and attempt history while gum-signers reconciles every
pass. Past the replacement limit fees are no longer raised, an
`ExecutionStalled` event is published, and reconciliation continues.

A reverted transfer or burn left
the funds in the Payday wallet; the merchant creates a new withdrawal. A
reverted mint whose message nonce is already used means somebody else
minted it (the relayer completes the leg by itself when it sees that);
otherwise re-submit `receiveMessage` by hand as above and set the leg
straight:

```bash
psql "$DATABASE_URL" -c "UPDATE withdrawal_legs SET state = 'completed', failure_reason = NULL, mint_tx_hash = decode('<tx hash without 0x>', 'hex') WHERE id = '<leg uuid>' AND state = 'failed'"
```

then let the withdrawal settle on the next terminal transition, or set
`completed_at` yourself if every leg is now `completed` and `failed_at` was
set.
