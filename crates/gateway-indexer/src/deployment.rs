//! Startup validation of the configured contract deployment.

pub use gateway_core::ExpectedDeployment;
use gateway_core::{DeploymentError, ObservedDeployment, check_deployment};

use crate::chain::ChainClient;

pub async fn verify_deployment(
    chain: &dyn ChainClient,
    expected: &ExpectedDeployment,
) -> Result<(), DeploymentError> {
    let observed = ObservedDeployment {
        chain_id: chain.get_chain_id().await.map_err(chain_error)?,
        factory_code_hash: chain
            .code_hash(expected.factory)
            .await
            .map_err(chain_error)?,
        batch_sweeper_code_hash: chain
            .code_hash(expected.batch_sweeper)
            .await
            .map_err(chain_error)?,
        bound_factory: chain
            .batch_sweeper_factory(expected.batch_sweeper)
            .await
            .map_err(chain_error)?,
    };
    check_deployment(expected, &observed)?;
    tracing::info!(chain_id = observed.chain_id, factory = %expected.factory, batch_sweeper = %expected.batch_sweeper, "contract deployment verified");
    Ok(())
}

fn chain_error(error: crate::chain::ChainError) -> DeploymentError {
    DeploymentError::Rpc {
        operation: "deployment read",
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indexer::tests::{MockChain, mock_code_hash};
    use alloy_primitives::{Address, B256, address};

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
    async fn wrong_chain_id_fails() {
        let chain = MockChain::new(1).with(|state| state.chain_id = 1);
        let error = verify_deployment(&chain, &expected()).await.unwrap_err();
        assert!(matches!(
            error,
            DeploymentError::ChainId {
                expected: 31337,
                actual: 1
            }
        ));
    }

    #[tokio::test]
    async fn wrong_code_hash_fails() {
        let chain = MockChain::new(1).with(|state| {
            state.code_hashes.insert(FACTORY, B256::ZERO);
        });
        assert!(matches!(
            verify_deployment(&chain, &expected()).await,
            Err(DeploymentError::FactoryCodeHash { .. })
        ));
    }
}
