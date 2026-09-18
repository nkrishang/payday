//! Operator subcommands. They share the binary so a deployment has one
//! image and one set of credentials for the ledger.
//!
//! * `migrate` applies pending schema migrations. Run before starting the
//!   services; every service's readiness check refuses to pass until the
//!   schema is at the version its binary expects.
//! * `bus dead [limit]` lists dead-lettered deliveries of both consumers.
//! * `bus retry <message-id>` returns one dead delivery to its queue.
//! * `chain resume <chain-id>` clears a chain's open finality fault and
//!   tells the signers to resume. Only after an operator has confirmed the
//!   node is healthy: the fault means a finalized block changed hash.

use gum_bus::Consumer;
use gum_bus::ConsumerOptions;
use gum_bus::Publisher;
use gum_contracts::CorrelationId;
use gum_ledger::ChainFaultRepository;
use sqlx::PgPool;
use uuid::Uuid;

const CONSUMERS: [&str; 2] = ["gum-server", "gum-signers"];

const USAGE: &str = "usage: gum-server [migrate | bus dead [limit] | bus retry <message-id> | chain resume <chain-id>]";

pub async fn run(pool: &PgPool, args: &[String]) -> Result<(), String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["migrate"] => {
            gum_schema::MIGRATOR
                .run(pool)
                .await
                .map_err(|error| format!("migration failed: {error}"))?;
            println!("migrations applied");
            Ok(())
        }
        ["bus", "dead"] | ["bus", "dead", _] => {
            let limit: i64 = match args.get(2) {
                Some(value) => value
                    .parse()
                    .map_err(|_| format!("invalid limit '{value}'"))?,
                None => 50,
            };
            for name in CONSUMERS {
                let consumer = Consumer::new(name, pool.clone(), ConsumerOptions::default());
                let dead = consumer
                    .dead(limit)
                    .await
                    .map_err(|error| error.to_string())?;
                println!("{name}: {} dead deliveries", dead.len());
                for delivery in dead {
                    println!(
                        "  {}  topic={} kind={} correlation_id={} attempts={} last_attempt_at={} error={}",
                        delivery.message_id,
                        delivery.topic,
                        delivery.kind,
                        delivery.correlation_id,
                        delivery.attempt_count,
                        delivery
                            .last_attempt_at
                            .map(gum_core::rfc3339)
                            .unwrap_or_default(),
                        delivery.last_error.unwrap_or_default()
                    );
                }
            }
            Ok(())
        }
        ["bus", "retry", message_id] => {
            let message_id: Uuid = message_id
                .parse()
                .map_err(|_| format!("invalid message id '{message_id}'"))?;
            let mut retried = false;
            for name in CONSUMERS {
                let consumer = Consumer::new(name, pool.clone(), ConsumerOptions::default());
                if consumer
                    .retry_dead(message_id)
                    .await
                    .map_err(|error| error.to_string())?
                {
                    println!("{name}: delivery {message_id} returned to the queue");
                    retried = true;
                }
            }
            if retried {
                Ok(())
            } else {
                Err(format!("no dead delivery of message {message_id}"))
            }
        }
        ["chain", "resume", chain_id] => {
            let chain_id: u64 = chain_id
                .parse()
                .map_err(|_| format!("invalid chain id '{chain_id}'"))?;
            let faults = ChainFaultRepository::new(pool.clone(), Publisher::new("gum-server"));
            match faults
                .clear(chain_id, CorrelationId::new())
                .await
                .map_err(|error| error.to_string())?
            {
                Some(fault_id) => {
                    println!("chain {chain_id}: fault {fault_id} cleared; signers told to resume");
                    Ok(())
                }
                None => Err(format!("chain {chain_id} has no open fault")),
            }
        }
        _ => Err(USAGE.to_owned()),
    }
}
