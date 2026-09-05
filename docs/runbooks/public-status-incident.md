# Public status incident: deposit progression delay

Use this procedure whenever `payday-indexer-cursor-lagging`,
`payday-indexer-sweep-paused`, or `payday-indexer-sweep-backlog-stale` enters
`ALARM`.
The status page automatically reports **Deposit indexing and settlement** as
degraded when finalized-head lag exceeds 1,000 blocks, either worker heartbeat
is stale, sweeping is paused, or collectable funds have waited over 15 minutes.
It does not disclose block heights, providers, or topology.

1. Acknowledge the operator alert and open `https://status.payday.sh` and
   `https://status.payday.sh/v1/status`. Confirm the component is degraded.
2. Within 15 minutes, publish a customer incident in the team's approved public
   incident channel: “Deposit detection and settlement are delayed. Funds remain
   safe; we are investigating.” Link to the status page and timestamp the update.
3. Triage with the indexer logs and the [stuck deposit request](stuck-deposit-request.md),
   [cursor reset](indexer-cursor-reset.md), and
   [RPC limits](quicknode-rpc-limits.md) runbooks. Never publish RPC URLs,
   block numbers, account data, or AWS details.
4. Post an update at least every 30 minutes while impact continues, even if
   there is no material change.
5. After CloudWatch returns to `OK`, verify the JSON endpoint reports
   `deposit_indexing_and_settlement.status` as `operational` for two checks.
   Publish a resolved update with duration and customer impact. Record a brief
   internal timeline and follow-up actions.

If the alarm and public status disagree, treat the alarm as customer-impacting
until finalized-head lag, both worker heartbeats, and sweep progression have
been verified.
