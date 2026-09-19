//! Shared validation for a configured contract deployment.

use alloy_primitives::{Address, B256};
use thiserror::Error;

/// The deployment this build was configured for.
pub struct ExpectedDeployment {
    pub chain_id: u64,
    pub factory: Address,
    pub factory_code_hash: B256,
    pub batch_sweeper: Address,
    pub batch_sweeper_code_hash: B256,
    /// The `WithdrawalForwarder` and its expected runtime code hash, on a
    /// chain that has CCTP.
    pub forwarder: Option<(Address, B256)>,
}

/// What an RPC endpoint reports for the configured addresses.
pub struct ObservedDeployment {
    pub chain_id: u64,
    pub factory_code_hash: B256,
    pub batch_sweeper_code_hash: B256,
    pub bound_factory: Address,
    /// Read only when `ExpectedDeployment::forwarder` is set.
    pub forwarder_code_hash: Option<B256>,
}

#[derive(Debug, Error)]
pub enum DeploymentError {
    #[error("{operation} failed against the chain's RPC URL: {message}")]
    Rpc {
        operation: &'static str,
        message: String,
    },
    #[error("the RPC URL serves chain {actual}, but its GUM_CHAINS entry is chain {expected}")]
    ChainId { expected: u64, actual: u64 },
    #[error(
        "PaymentFactory at {address} has runtime code hash {actual}, but the chain's factory_code_hash is {expected}; the configured factory is not the reviewed generation"
    )]
    FactoryCodeHash {
        address: Address,
        expected: B256,
        actual: B256,
    },
    #[error(
        "BatchSweeper at {address} has runtime code hash {actual}, but the chain's batch_sweeper_code_hash is {expected}; the configured sweeper is not the reviewed generation"
    )]
    BatchSweeperCodeHash {
        address: Address,
        expected: B256,
        actual: B256,
    },
    #[error(
        "BatchSweeper at {batch_sweeper} is bound to PaymentFactory {actual}, not the chain's configured factory {expected}"
    )]
    BoundFactory {
        batch_sweeper: Address,
        expected: Address,
        actual: Address,
    },
    #[error(
        "WithdrawalForwarder at {address} has runtime code hash {actual}, but the chain's cctp.forwarder_code_hash is {expected}; the configured forwarder is not the reviewed generation"
    )]
    ForwarderCodeHash {
        address: Address,
        expected: B256,
        actual: B256,
    },
}

pub fn check_deployment(
    expected: &ExpectedDeployment,
    observed: &ObservedDeployment,
) -> Result<(), DeploymentError> {
    if observed.chain_id != expected.chain_id {
        return Err(DeploymentError::ChainId {
            expected: expected.chain_id,
            actual: observed.chain_id,
        });
    }
    if observed.factory_code_hash != expected.factory_code_hash {
        return Err(DeploymentError::FactoryCodeHash {
            address: expected.factory,
            expected: expected.factory_code_hash,
            actual: observed.factory_code_hash,
        });
    }
    if observed.batch_sweeper_code_hash != expected.batch_sweeper_code_hash {
        return Err(DeploymentError::BatchSweeperCodeHash {
            address: expected.batch_sweeper,
            expected: expected.batch_sweeper_code_hash,
            actual: observed.batch_sweeper_code_hash,
        });
    }
    if observed.bound_factory != expected.factory {
        return Err(DeploymentError::BoundFactory {
            batch_sweeper: expected.batch_sweeper,
            expected: expected.factory,
            actual: observed.bound_factory,
        });
    }
    if let Some((address, expected_hash)) = expected.forwarder {
        // An unread hash never matches: the caller must have looked.
        let actual = observed.forwarder_code_hash.unwrap_or_default();
        if actual != expected_hash {
            return Err(DeploymentError::ForwarderCodeHash {
                address,
                expected: expected_hash,
                actual,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::address;

    #[test]
    fn chain_id_is_part_of_deployment_validation() {
        let factory = address!("0000000000000000000000000000000000000001");
        let expected = ExpectedDeployment {
            chain_id: 143,
            factory,
            factory_code_hash: B256::ZERO,
            batch_sweeper: Address::ZERO,
            batch_sweeper_code_hash: B256::ZERO,
            forwarder: None,
        };
        let observed = ObservedDeployment {
            chain_id: 1,
            factory_code_hash: B256::ZERO,
            batch_sweeper_code_hash: B256::ZERO,
            bound_factory: factory,
            forwarder_code_hash: None,
        };
        assert!(matches!(
            check_deployment(&expected, &observed),
            Err(DeploymentError::ChainId {
                expected: 143,
                actual: 1
            })
        ));
    }

    #[test]
    fn forwarder_code_hash_is_checked_only_when_configured() {
        let factory = address!("0000000000000000000000000000000000000001");
        let forwarder = address!("0000000000000000000000000000000000000002");
        let mut expected = ExpectedDeployment {
            chain_id: 143,
            factory,
            factory_code_hash: B256::ZERO,
            batch_sweeper: Address::ZERO,
            batch_sweeper_code_hash: B256::ZERO,
            forwarder: None,
        };
        let mut observed = ObservedDeployment {
            chain_id: 143,
            factory_code_hash: B256::ZERO,
            batch_sweeper_code_hash: B256::ZERO,
            bound_factory: factory,
            forwarder_code_hash: None,
        };
        check_deployment(&expected, &observed).unwrap();

        expected.forwarder = Some((forwarder, B256::repeat_byte(0xF0)));
        assert!(matches!(
            check_deployment(&expected, &observed),
            Err(DeploymentError::ForwarderCodeHash { address, .. }) if address == forwarder
        ));
        observed.forwarder_code_hash = Some(B256::repeat_byte(0xF0));
        check_deployment(&expected, &observed).unwrap();
    }
}
