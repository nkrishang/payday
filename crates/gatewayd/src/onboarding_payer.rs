//! The onboarding walkthrough's one real on-chain transfer: Payday paying
//! its own first, self-issued deposit request so a brand new merchant sees
//! the whole product — a real invoice, a real transfer, real settlement —
//! before they have a real customer of their own.
//!
//! Mirrors `attestation.rs`'s local-key-or-KMS signer selection, but where
//! that signer only produces a raw off-chain signature, this one signs and
//! broadcasts a real transaction, so it also needs a wallet-filled provider
//! (the same construction `gateway-indexer` uses for the sweep signer).

use std::sync::Arc;

use alloy_network::{EthereumWallet, TransactionBuilder};
use alloy_primitives::{Address, B256, Signature, U256};
use alloy_provider::{DynProvider, Provider, ProviderBuilder};
use alloy_rpc_types_eth::TransactionRequest;
use alloy_signer::Signer;
use alloy_signer_aws::AwsSigner;
use alloy_signer_local::PrivateKeySigner;
use alloy_sol_types::{SolCall, sol};

use crate::config::OnboardingPayerSignerConfig;

sol! {
    function transfer(address to, uint256 amount) returns (bool);
}

/// The same key, kept for raw signatures: the demo payer attests its wallet
/// (an EIP-712 digest) before it pays.
enum Backend {
    Local(PrivateKeySigner),
    Kms(AwsSigner),
}

#[derive(Clone)]
pub struct OnboardingPayerSigner {
    provider: DynProvider,
    backend: Arc<Backend>,
    address: Address,
    /// The chain the demo pays on and that chain's USDC.
    chain_id: u64,
    usdc: Address,
}

impl OnboardingPayerSigner {
    pub async fn from_config(
        config: &OnboardingPayerSignerConfig,
        sdk_config: &aws_config::SdkConfig,
        rpc_url: &str,
        chain_id: u64,
        usdc: Address,
    ) -> Result<Self, String> {
        let (address, wallet, backend) = match config {
            OnboardingPayerSignerConfig::Local(key) => {
                let signer: PrivateKeySigner = key
                    .parse()
                    .map_err(|error| format!("invalid PAYDAY_ONBOARDING_PAYER_KEY: {error}"))?;
                (
                    signer.address(),
                    EthereumWallet::from(signer.clone()),
                    Backend::Local(signer),
                )
            }
            OnboardingPayerSignerConfig::AwsKms(key_id) => {
                let kms = aws_sdk_kms::Client::new(sdk_config);
                let signer = AwsSigner::new(kms, key_id.clone(), Some(chain_id))
                    .await
                    .map_err(|error| {
                        format!("failed to initialize PAYDAY_ONBOARDING_PAYER_KMS_KEY_ID: {error}")
                    })?;
                (
                    signer.address(),
                    EthereumWallet::from(signer.clone()),
                    Backend::Kms(signer),
                )
            }
        };
        let provider = ProviderBuilder::new()
            .wallet(wallet)
            .connect(rpc_url)
            .await
            .map_err(|error| format!("failed to connect onboarding payer provider: {error}"))?
            .erased();
        Ok(Self {
            provider,
            backend: Arc::new(backend),
            address,
            chain_id,
            usdc,
        })
    }

    pub fn address(&self) -> Address {
        self.address
    }

    /// The chain the demo pays on.
    pub fn chain_id(&self) -> u64 {
        self.chain_id
    }

    /// That chain's USDC.
    pub fn usdc(&self) -> Address {
        self.usdc
    }

    /// Sign a raw 32-byte digest (the payer attestation's EIP-712 hash).
    pub async fn sign_hash(&self, digest: &B256) -> Result<Signature, alloy_signer::Error> {
        match &*self.backend {
            Backend::Local(signer) => signer.sign_hash(digest).await,
            Backend::Kms(signer) => signer.sign_hash(digest).await,
        }
    }

    /// Broadcast `USDC.transfer(to, amount)` and return the transaction hash.
    /// Does not wait for a receipt: the indexer picks the transfer up and
    /// carries the invoice through funding and settlement on its own, exactly
    /// as it would for a payer's own browser-submitted transfer.
    pub async fn send_usdc(&self, to: Address, amount: U256) -> Result<B256, String> {
        let usdc_address = self.usdc;
        let calldata = transferCall { to, amount }.abi_encode();
        let tx = TransactionRequest::default()
            .with_to(usdc_address)
            .with_input(calldata);
        let pending = self
            .provider
            .send_transaction(tx)
            .await
            .map_err(|error| error.to_string())?;
        Ok(*pending.tx_hash())
    }
}
