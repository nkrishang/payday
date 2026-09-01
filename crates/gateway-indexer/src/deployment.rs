//! Startup check that the configured contracts are the reviewed generation.
//!
//! `PaymentFactory` embeds `Payment.creationCode`, so a factory of the wrong
//! generation would derive different payment addresses than this build's
//! `predict_payment_address`, and a `BatchSweeper` bound to another factory
//! would deploy through it. Both hashes are `keccak256` of the runtime
//! bytecode `eth_getCode` returns; operators compute them with
//! `cast keccak "$(cast code <ADDRESS> --rpc-url <RPC_URL>)"`.

use alloy_primitives::{Address, B256};
use thiserror::Error;

use crate::chain::{ChainClient, ChainError};

/// The deployment this build was configured for.
pub struct ExpectedDeployment {
    pub chain_id: u64,
    pub factory: Address,
    pub factory_code_hash: B256,
    pub batch_sweeper: Address,
    pub batch_sweeper_code_hash: B256,
}

#[derive(Debug, Error)]
pub enum DeploymentError {
    #[error("could not read the deployment from the RPC endpoint: {0}")]
    Chain(#[from] ChainError),
    #[error(
        "PaymentFactory at {address} has runtime code hash {actual}, but PAYDAY_FACTORY_CODE_HASH is {expected}; the configured factory is not the reviewed generation"
    )]
    FactoryCodeHash {
        address: Address,
        expected: B256,
        actual: B256,
    },
    #[error(
        "BatchSweeper at {address} has runtime code hash {actual}, but PAYDAY_BATCH_SWEEPER_CODE_HASH is {expected}; the configured sweeper is not the reviewed generation"
    )]
    BatchSweeperCodeHash {
        address: Address,
        expected: B256,
        actual: B256,
    },
    #[error(
        "BatchSweeper at {batch_sweeper} is bound to PaymentFactory {actual}, not the configured PAYDAY_FACTORY_ADDRESS {expected}"
    )]
    BoundFactory {
        batch_sweeper: Address,
        expected: Address,
        actual: Address,
    },
}

/// Refuse to run against any contract that is not the expected generation.
/// The chain id is asserted by the caller before the RPC is trusted for reads.
pub async fn verify_deployment(
    chain: &dyn ChainClient,
    expected: &ExpectedDeployment,
) -> Result<(), DeploymentError> {
    let factory_code_hash = chain.code_hash(expected.factory).await?;
    if factory_code_hash != expected.factory_code_hash {
        return Err(DeploymentError::FactoryCodeHash {
            address: expected.factory,
            expected: expected.factory_code_hash,
            actual: factory_code_hash,
        });
    }
    let batch_sweeper_code_hash = chain.code_hash(expected.batch_sweeper).await?;
    if batch_sweeper_code_hash != expected.batch_sweeper_code_hash {
        return Err(DeploymentError::BatchSweeperCodeHash {
            address: expected.batch_sweeper,
            expected: expected.batch_sweeper_code_hash,
            actual: batch_sweeper_code_hash,
        });
    }
    let bound_factory = chain.batch_sweeper_factory(expected.batch_sweeper).await?;
    if bound_factory != expected.factory {
        return Err(DeploymentError::BoundFactory {
            batch_sweeper: expected.batch_sweeper,
            expected: expected.factory,
            actual: bound_factory,
        });
    }
    tracing::info!(
        chain_id = expected.chain_id,
        factory = %expected.factory,
        batch_sweeper = %expected.batch_sweeper,
        "contract deployment verified"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use alloy_primitives::address;

    use super::*;
    use crate::indexer::tests::{MockChain, mock_code_hash};

    const FACTORY: Address = address!("0x5FbDB2315678afecb367f032d93F642f64180aa3");
    const SWEEPER: Address = address!("0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0");

    fn expected() -> ExpectedDeployment {
        ExpectedDeployment {
            chain_id: 31337,
            factory: FACTORY,
            factory_code_hash: mock_code_hash(FACTORY),
            batch_sweeper: SWEEPER,
            batch_sweeper_code_hash: mock_code_hash(SWEEPER),
        }
    }

    #[tokio::test]
    async fn matching_deployment_passes() {
        verify_deployment(&MockChain::new(1), &expected())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn wrong_factory_code_hash_fails() {
        let chain = MockChain::new(1).with(|state| {
            state.code_hashes.insert(FACTORY, B256::repeat_byte(0x01));
        });
        let error = verify_deployment(&chain, &expected()).await.unwrap_err();
        assert!(matches!(
            error,
            DeploymentError::FactoryCodeHash { address, .. } if address == FACTORY
        ));
        assert!(error.to_string().contains("PAYDAY_FACTORY_CODE_HASH"));
    }

    #[tokio::test]
    async fn wrong_batch_sweeper_code_hash_fails() {
        let chain = MockChain::new(1).with(|state| {
            state.code_hashes.insert(SWEEPER, B256::repeat_byte(0x02));
        });
        let error = verify_deployment(&chain, &expected()).await.unwrap_err();
        assert!(matches!(
            error,
            DeploymentError::BatchSweeperCodeHash { address, .. } if address == SWEEPER
        ));
        assert!(error.to_string().contains("PAYDAY_BATCH_SWEEPER_CODE_HASH"));
    }

    #[tokio::test]
    async fn sweeper_bound_to_another_factory_fails() {
        let other = address!("0x000000000000000000000000000000000000dEaD");
        let chain = MockChain::new(1).with(|state| {
            state.sweeper_factories.insert(SWEEPER, other);
        });
        let error = verify_deployment(&chain, &expected()).await.unwrap_err();
        assert!(matches!(
            error,
            DeploymentError::BoundFactory { actual, .. } if actual == other
        ));
        assert!(error.to_string().contains(&FACTORY.to_string()));
    }
}
