//! Startup check that the configured contracts are the reviewed generation.
//!
//! The API derives every payment address from `PAYDAY_FACTORY_ADDRESS` with
//! this build's `predict_payment_address`, which assumes the factory embeds
//! the reviewed `Payment.creationCode`. A factory of another generation would
//! hand payers addresses nothing will ever sweep, and a `BatchSweeper` bound
//! to another factory would deploy through it. Both hashes are `keccak256` of
//! the runtime bytecode `eth_getCode` returns; operators compute them with
//! `cast keccak "$(cast code <ADDRESS> --rpc-url <RPC_URL>)"`.

use alloy_primitives::{Address, B256, keccak256};
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::{TransactionInput, TransactionRequest};
use alloy_sol_types::{SolCall, sol};
pub use gateway_core::ExpectedDeployment;
use gateway_core::{DeploymentError, ObservedDeployment, check_deployment};

sol! {
    function factory() view returns (address);
}

/// Transport errors print the full request URL, and `PAYDAY_RPC_URL` carries
/// the provider token, so the message is redacted before it can reach a log
/// line or the startup panic.
fn rpc_error(operation: &'static str, error: impl std::fmt::Display) -> DeploymentError {
    DeploymentError::Rpc {
        operation,
        message: redact_urls(&error.to_string()),
    }
}

/// Replace every `http://` or `https://` URL in `message` with `<rpc-url>`.
pub(crate) fn redact_urls(message: &str) -> String {
    let mut redacted = String::with_capacity(message.len());
    let mut rest = message;
    while let Some(start) = url_start(rest) {
        redacted.push_str(&rest[..start]);
        let url = &rest[start..];
        let end = url
            .find(|c: char| c.is_whitespace() || c == ')' || c == '"')
            .unwrap_or(url.len());
        redacted.push_str("<rpc-url>");
        rest = &url[end..];
    }
    redacted.push_str(rest);
    redacted
}

fn url_start(message: &str) -> Option<usize> {
    match (message.find("http://"), message.find("https://")) {
        (Some(http), Some(https)) => Some(http.min(https)),
        (http, https) => http.or(https),
    }
}

/// Read the live deployment from `rpc_url` and refuse any mismatch.
pub async fn verify_deployment(
    rpc_url: &str,
    expected: &ExpectedDeployment,
) -> Result<(), DeploymentError> {
    let provider = ProviderBuilder::new()
        .connect(rpc_url)
        .await
        .map_err(|error| rpc_error("connect", error))?;
    let chain_id = provider
        .get_chain_id()
        .await
        .map_err(|error| rpc_error("eth_chainId", error))?;
    let factory_code_hash = code_hash(&provider, expected.factory).await?;
    let batch_sweeper_code_hash = code_hash(&provider, expected.batch_sweeper).await?;
    let call = TransactionRequest::default()
        .to(expected.batch_sweeper)
        .input(TransactionInput::new(factoryCall {}.abi_encode().into()));
    let output = provider
        .call(call)
        .await
        .map_err(|error| rpc_error("eth_call BatchSweeper.factory", error))?;
    let bound_factory = factoryCall::abi_decode_returns(&output).map_err(|error| {
        rpc_error(
            "eth_call BatchSweeper.factory",
            format!("could not decode response: {error}"),
        )
    })?;
    check_deployment(
        expected,
        &ObservedDeployment {
            chain_id,
            factory_code_hash,
            batch_sweeper_code_hash,
            bound_factory,
        },
    )?;
    tracing::info!(
        chain_id,
        factory = %expected.factory,
        batch_sweeper = %expected.batch_sweeper,
        "contract deployment verified"
    );
    Ok(())
}

/// `keccak256` of the runtime bytecode at `address`; an address without code
/// hashes as empty bytes and so never matches a reviewed generation.
async fn code_hash(provider: &impl Provider, address: Address) -> Result<B256, DeploymentError> {
    provider
        .get_code_at(address)
        .await
        .map(|code| keccak256(&code))
        .map_err(|error| rpc_error("eth_getCode", error))
}

#[cfg(test)]
mod tests {
    use alloy_primitives::address;

    use super::*;

    const FACTORY: Address = address!("0x5FbDB2315678afecb367f032d93F642f64180aa3");
    const SWEEPER: Address = address!("0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0");

    fn expected() -> ExpectedDeployment {
        ExpectedDeployment {
            chain_id: 31337,
            factory: FACTORY,
            factory_code_hash: B256::repeat_byte(0xF1),
            batch_sweeper: SWEEPER,
            batch_sweeper_code_hash: B256::repeat_byte(0xB1),
        }
    }

    fn observed() -> ObservedDeployment {
        ObservedDeployment {
            chain_id: 31337,
            factory_code_hash: B256::repeat_byte(0xF1),
            batch_sweeper_code_hash: B256::repeat_byte(0xB1),
            bound_factory: FACTORY,
        }
    }

    #[test]
    fn matching_deployment_passes() {
        check_deployment(&expected(), &observed()).unwrap();
    }

    #[test]
    fn rpc_error_messages_never_carry_the_rpc_url() {
        let leaked = "error sending request for url (https://x.quiknode.pro/SECRET/)";
        assert_eq!(
            redact_urls(leaked),
            "error sending request for url (<rpc-url>)"
        );
        assert_eq!(
            redact_urls("tried \"https://a.example/T1\" then http://b.example/T2 next"),
            "tried \"<rpc-url>\" then <rpc-url> next"
        );
        assert_eq!(
            redact_urls("connection reset by peer"),
            "connection reset by peer"
        );

        let message = rpc_error("eth_getCode", leaked).to_string();
        assert!(!message.contains("quiknode"), "{message}");
        assert!(!message.contains("SECRET"), "{message}");
        assert!(
            message.contains("eth_getCode failed against PAYDAY_RPC_URL: "),
            "{message}"
        );
        assert!(message.contains("<rpc-url>"), "{message}");

        let payload = rpc_error(
            "eth_call BatchSweeper.factory",
            "server returned an error response: error code -32000: execution reverted",
        );
        assert!(payload.to_string().contains("execution reverted"));
    }

    #[test]
    fn wrong_chain_id_fails() {
        let mut observed = observed();
        observed.chain_id = 1;
        let error = check_deployment(&expected(), &observed).unwrap_err();
        assert!(matches!(
            error,
            DeploymentError::ChainId {
                expected: 31337,
                actual: 1
            }
        ));
        assert!(error.to_string().contains("PAYDAY_CHAIN_ID"));
    }

    #[test]
    fn wrong_factory_code_hash_fails() {
        let mut observed = observed();
        observed.factory_code_hash = B256::repeat_byte(0x01);
        let error = check_deployment(&expected(), &observed).unwrap_err();
        assert!(matches!(
            error,
            DeploymentError::FactoryCodeHash { address, .. } if address == FACTORY
        ));
        let message = error.to_string();
        assert!(message.contains("PAYDAY_FACTORY_CODE_HASH"));
        assert!(message.contains(&B256::repeat_byte(0xF1).to_string()));
        assert!(message.contains(&B256::repeat_byte(0x01).to_string()));
    }

    #[test]
    fn wrong_batch_sweeper_code_hash_fails() {
        let mut observed = observed();
        observed.batch_sweeper_code_hash = B256::repeat_byte(0x02);
        let error = check_deployment(&expected(), &observed).unwrap_err();
        assert!(matches!(
            error,
            DeploymentError::BatchSweeperCodeHash { address, .. } if address == SWEEPER
        ));
        assert!(error.to_string().contains("PAYDAY_BATCH_SWEEPER_CODE_HASH"));
    }

    #[test]
    fn sweeper_bound_to_another_factory_fails() {
        let other = address!("0x000000000000000000000000000000000000dEaD");
        let mut observed = observed();
        observed.bound_factory = other;
        let error = check_deployment(&expected(), &observed).unwrap_err();
        assert!(matches!(
            error,
            DeploymentError::BoundFactory { actual, .. } if actual == other
        ));
        assert!(error.to_string().contains(&FACTORY.to_string()));
    }
}
