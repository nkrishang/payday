//! Observability shared by every Gum service.
//!
//! * [`init`] installs the `tracing` subscriber: JSON lines in production
//!   (`GUM_LOG_FORMAT=json`), human-readable locally. Every line carries
//!   `service`, and span fields such as `correlation_id`, `deposit_request_id`
//!   and `chain_id` are flattened into each event, so one deposit can be
//!   followed across all three services by grepping one id.
//! * [`correlation`] extracts and injects the correlation headers on
//!   internal RPC calls.
//! * [`health`] serves `/health/live` and `/health/ready` from a set of
//!   named [`health::ReadinessCheck`]s; liveness is "the process runs",
//!   readiness is "every dependency this service needs answers".
//!
//! Field names are a contract with whoever reads the logs. Use these and
//! only these for the concepts they name:
//!
//! | field                | meaning                                             |
//! |----------------------|-----------------------------------------------------|
//! | `service`            | `gum-server`, `gum-indexer`, `gum-signers`          |
//! | `correlation_id`     | the deposit request / withdrawal flow being served  |
//! | `request_id`         | one HTTP request                                    |
//! | `deposit_request_id` | `invoices.id`                                       |
//! | `payment_address`    | the CREATE3 address being watched or swept          |
//! | `chain_id`           | EIP-155 chain id                                    |
//! | `tx_hash`            | a transaction hash                                  |
//! | `signer`             | a pool signer address                               |
//! | `job_id`             | an execution job (sweep batch / step / payment)     |
//! | `message_type`       | bus message kind                                    |
//! | `message_id`         | bus message id                                      |
//! | `attempt`            | attempt number of the current try                   |
//! | `error`              | the error being reported, `Display`ed               |
//! | `latency_ms`         | wall time of the operation the event closes         |

pub mod correlation;
pub mod health;

use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Which encoder the subscriber writes with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Json,
    Pretty,
}

impl LogFormat {
    /// `GUM_LOG_FORMAT`: `json` or `pretty`; unset means pretty when
    /// stdout is a terminal and JSON otherwise.
    pub fn from_env() -> Self {
        match std::env::var("GUM_LOG_FORMAT").as_deref() {
            Ok("json") => Self::Json,
            Ok("pretty") | Ok("text") => Self::Pretty,
            _ => {
                if std::io::IsTerminal::is_terminal(&std::io::stdout()) {
                    Self::Pretty
                } else {
                    Self::Json
                }
            }
        }
    }
}

/// Install the global subscriber. `service` is stamped on every event.
/// `RUST_LOG` filters as usual (default `info`).
pub fn init(service: &'static str) {
    init_with(service, LogFormat::from_env());
}

pub fn init_with(service: &'static str, format: LogFormat) {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let service_span = tracing::info_span!("service", service);
    match format {
        LogFormat::Json => {
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .json()
                        .flatten_event(true)
                        .with_current_span(true)
                        .with_span_list(false)
                        .with_span_events(FmtSpan::NONE)
                        .with_target(true),
                )
                .init();
        }
        LogFormat::Pretty => {
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_target(false)
                        .with_span_events(FmtSpan::NONE),
                )
                .init();
        }
    }
    // The service span is entered for the lifetime of the process so every
    // event, including those outside request spans, carries `service`.
    // `Box::leak` keeps the guard alive; there is exactly one per process.
    Box::leak(Box::new(service_span.entered()));
}
