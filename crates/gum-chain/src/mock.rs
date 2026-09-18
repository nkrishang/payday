//! In-memory chain for tests: one final history, deterministic hashes and
//! timestamps, scriptable receipts, failures and reorgs. Enabled with the
//! `mock` feature so gum-indexer and gum-signers share one fixture.
//!
//! Every field of [`MockState`] is public; tests script the world with
//! [`MockChain::with`] / [`MockChain::set`] and inspect it afterwards.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use alloy_primitives::{Address, B256, Bytes, U256, address, keccak256};
use async_trait::async_trait;
use gum_core::{
    Amount, BeneficiaryAddress, ChainId, FactoryAddress, RecoveryAddress, TokenAddress,
};

use crate::{
    BlockHeader, ChainError, ChainExecutor, ChainReader, FailureProbe, FeeEstimate,
    PreparedSweepTransaction, SettlementEvent, SweepOutcome, SweepReceipt, SweepRequest,
    TokenPaymentReceipt, TransactionOutcome, WatchedTransfer,
};

/// Chain ID the mock reports.
pub const CHAIN_ID: u64 = 31337;
/// Block timestamps in the mock advance ten seconds per block from here.
pub const GENESIS_TIMESTAMP: u64 = 1_800_000_000;
/// Comfortably after every block the tests mine.
pub const FAR_EXPIRY: u64 = GENESIS_TIMESTAMP + 1_000_000;

/// The token address the fixtures use for USDC.
pub fn usdc() -> Address {
    address!("0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512")
}

/// The factory the mock's BatchSweepers report and payment addresses derive from.
pub fn factory() -> FactoryAddress {
    FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3"))
}

pub fn batch_sweeper() -> Address {
    address!("0x9fE46736679d2D9a65F0992F2272dE9f3c7fa6e0")
}

/// The k-th signer of the mock's pool; the default pool is `[mock_signer(0)]`.
pub fn mock_signer(index: u8) -> Address {
    Address::repeat_byte(0xA0 + index)
}

pub fn block_hash(block: u64) -> B256 {
    B256::with_last_byte(block as u8)
}

fn block_timestamp(block: u64) -> u64 {
    GENESIS_TIMESTAMP + block * 10
}

/// The transaction hash the mock reports for a consumed authorization:
/// derived from the nonce, so tests can predict it.
pub fn mock_authorization_tx_hash(nonce: B256) -> B256 {
    let mut hash = nonce;
    hash[31] = hash[31].wrapping_add(1);
    hash
}

/// The address a sweep item deploys to, derived the way the API does.
fn payment_address_of(sweep: &SweepRequest) -> Address {
    gum_core::predict_payment_address(
        factory(),
        TokenAddress(sweep.token),
        Amount(sweep.amount),
        BeneficiaryAddress(sweep.receiver),
        sweep.expiration_timestamp,
        RecoveryAddress(sweep.recovery),
        gum_core::Salt(sweep.salt),
        ChainId(sweep.chain_id),
    )
    .0
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    /// The pool signer the transaction was signed from.
    pub signer: Address,
    pub nonce: u64,
    pub gas_limit: u64,
    pub fees: FeeEstimate,
    pub sweeps: Vec<SweepRequest>,
    pub tx_hash: B256,
    /// Set for generic calls (withdrawal steps); sweeps leave it empty.
    pub to: Option<Address>,
    pub calldata: Bytes,
}

#[derive(Default)]
pub struct MockState {
    pub chain_id: u64,
    pub latest: u64,
    pub finalized: u64,
    pub transfers: Vec<WatchedTransfer>,
    pub max_log_range: Option<u64>,
    /// Every `eth_getLogs` the worker made: the range and the recipient
    /// filter it carried.
    pub log_requests: Vec<(u64, u64, Vec<Address>)>,
    /// Hash overrides to simulate a reorg at a height.
    pub hashes: HashMap<u64, B256>,
    pub receipts: HashMap<B256, SweepReceipt>,
    /// Outcomes attached to the receipt of the next submission, keyed by
    /// payment address. Items without an entry get `Settled`.
    pub next_outcomes: HashMap<Address, SweepOutcome>,
    /// Block the next submission's receipt lands in; `None` leaves it unmined.
    pub mine_at: Option<u64>,
    pub next_receipt_succeeds: bool,
    pub prepared: HashMap<B256, Submission>,
    pub next_transaction_id: u8,
    pub submissions: Vec<Submission>,
    /// The pool, in rotation order. `MockChain::new` seeds one signer.
    pub signers: Vec<Address>,
    /// Keys registered for dedicated work but excluded from the ordinary pool.
    pub dedicated_signers: HashSet<Address>,
    /// Mined transaction count per signer; a missing entry is 0.
    pub mined_nonces: HashMap<Address, u64>,
    /// Pending transaction count overrides; otherwise it equals the mined count.
    pub pending_nonces: HashMap<Address, u64>,
    pub submit_error: Option<fn() -> ChainError>,
    pub fees: FeeEstimate,
    /// `estimate_fees` calls, for the once-per-pass assertion.
    pub fee_requests: usize,
    /// Balance overrides per signer; a missing entry is one native token.
    pub balances: HashMap<Address, U256>,
    /// Signers whose `eth_getBalance` fails, for health isolation tests.
    pub balance_errors: HashSet<Address>,
    /// Signers whose signing fails, for rotation isolation tests.
    pub prepare_errors: HashSet<Address>,
    pub settled: HashMap<Address, bool>,
    pub settlement_events: HashMap<Address, SettlementEvent>,
    /// Blocks from which each payment's settlement event exists: ranges
    /// ending before the block return no settlement, so classification
    /// has to page through chunks to find it.
    pub settlement_appears_at: HashMap<Address, u64>,
    pub probes: HashMap<Address, FailureProbe>,
    pub header_requests: usize,
    pub reorg_on_header_request: Option<usize>,
    /// Number of `block_header` reads that still fail with a retryable
    /// error before succeeding; see the range-retry test.
    pub failing_header_reads: u32,
    /// Runtime code hashes by address; see [`mock_code_hash`] for the default.
    pub code_hashes: HashMap<Address, B256>,
    /// Factory each BatchSweeper reports; defaults to the test factory.
    pub sweeper_factories: HashMap<Address, Address>,
    /// `eth_call` answers by (contract, calldata); anything else is 32 zero bytes.
    pub view_results: HashMap<(Address, Bytes), Bytes>,
    /// EIP-3009 authorizations the token has consumed, by (authorizer, nonce).
    pub consumed_authorizations: HashSet<(Address, B256)>,
    /// The block each consumed authorization's `AuthorizationUsed` event
    /// appears in, so ranges ending before it find nothing.
    pub authorization_events: HashMap<B256, u64>,
    /// Token payment receipts the node answers `token_payment_receipt`
    /// with, by hash: execution plus the `Transfer` logs one token left.
    pub token_receipts: HashMap<B256, TokenPaymentReceipt>,
}

/// The code hash the mock reports for an address it has no override for:
/// a stand-in for "some deployed runtime bytecode" that differs per address.
pub fn mock_code_hash(address: Address) -> B256 {
    keccak256(address.as_slice())
}

pub struct MockChain {
    pub state: Mutex<MockState>,
}

impl MockChain {
    pub fn new(latest: u64) -> Self {
        Self {
            state: Mutex::new(MockState {
                chain_id: 31337,
                latest,
                finalized: latest,
                mine_at: Some(latest),
                next_receipt_succeeds: true,
                fees: FeeEstimate {
                    max_fee_per_gas: 100,
                    max_priority_fee_per_gas: 2,
                },
                signers: vec![mock_signer(0)],
                ..MockState::default()
            }),
        }
    }

    /// A pool of `count` signers, `mock_signer(0)..mock_signer(count)`.
    pub fn with_signers(self, count: u8) -> Self {
        self.with(|state| state.signers = (0..count).map(mock_signer).collect())
    }

    pub fn with(self, apply: impl FnOnce(&mut MockState)) -> Self {
        apply(&mut self.state.lock().unwrap());
        self
    }

    pub fn set(&self, apply: impl FnOnce(&mut MockState)) {
        apply(&mut self.state.lock().unwrap());
    }

    pub fn submissions(&self) -> Vec<Submission> {
        self.state.lock().unwrap().submissions.clone()
    }
}

#[async_trait]
impl ChainReader for MockChain {
    async fn get_chain_id(&self) -> Result<u64, ChainError> {
        Ok(self.state.lock().unwrap().chain_id)
    }

    async fn latest_block_number(&self) -> Result<u64, ChainError> {
        Ok(self.state.lock().unwrap().latest)
    }

    async fn finalized_header(&self) -> Result<BlockHeader, ChainError> {
        // A tag read is a header read: it counts toward `header_requests`
        // like every other `eth_getBlockByNumber`.
        let finalized = self.state.lock().unwrap().finalized;
        self.block_header(finalized).await
    }

    async fn block_header(&self, number: u64) -> Result<BlockHeader, ChainError> {
        let mut state = self.state.lock().unwrap();
        state.header_requests += 1;
        if state.failing_header_reads > 0 {
            state.failing_header_reads -= 1;
            return Err(ChainError::Transient(format!(
                "block {number} read throttled"
            )));
        }
        if number > state.latest {
            return Err(ChainError::Transient(format!(
                "block {number} is unavailable"
            )));
        }
        Ok(BlockHeader {
            number,
            hash: if state
                .reorg_on_header_request
                .is_some_and(|request| state.header_requests >= request)
            {
                B256::repeat_byte(0xEE)
            } else {
                state
                    .hashes
                    .get(&number)
                    .copied()
                    .unwrap_or_else(|| block_hash(number))
            },
            timestamp: block_timestamp(number),
        })
    }

    async fn token_transfers(
        &self,
        _tokens: &[Address],
        from_block: u64,
        to_block: u64,
        recipients: &[Address],
    ) -> Result<Vec<WatchedTransfer>, ChainError> {
        let mut state = self.state.lock().unwrap();
        let requested = to_block - from_block + 1;
        if state.max_log_range.is_some_and(|max| requested > max) {
            return Err(ChainError::LogRangeTooLarge(format!(
                "mock limit is {} blocks",
                state.max_log_range.unwrap()
            )));
        }
        state
            .log_requests
            .push((from_block, to_block, recipients.to_vec()));
        Ok(state
            .transfers
            .iter()
            .filter(|transfer| (from_block..=to_block).contains(&transfer.block_number))
            .filter(|transfer| recipients.contains(&transfer.recipient))
            .cloned()
            .collect())
    }

    async fn sweep_receipt(
        &self,
        tx_hash: B256,
        _batch_sweeper: Address,
    ) -> Result<Option<SweepReceipt>, ChainError> {
        Ok(self.state.lock().unwrap().receipts.get(&tx_hash).cloned())
    }

    async fn token_payment_receipt(
        &self,
        tx_hash: B256,
        _token: Address,
    ) -> Result<Option<TokenPaymentReceipt>, ChainError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .token_receipts
            .get(&tx_hash)
            .cloned())
    }

    async fn transaction_receipt(
        &self,
        tx_hash: B256,
    ) -> Result<Option<TransactionOutcome>, ChainError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .receipts
            .get(&tx_hash)
            .map(|receipt| TransactionOutcome {
                succeeded: receipt.succeeded,
                block: receipt.block,
                block_hash: receipt.block_hash,
            }))
    }

    /// The mock's world has one final history, so reading through
    /// finality changes nothing.
    async fn finalized_view_call(&self, to: Address, calldata: Bytes) -> Result<Bytes, ChainError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .view_results
            .get(&(to, calldata))
            .cloned()
            .unwrap_or_else(|| Bytes::from(vec![0u8; 32])))
    }

    async fn authorization_state(
        &self,
        token: Address,
        authorizer: Address,
        nonce: B256,
        at_block: u64,
    ) -> Result<bool, ChainError> {
        let _ = token;
        let _ = at_block;
        Ok(self
            .state
            .lock()
            .unwrap()
            .consumed_authorizations
            .contains(&(authorizer, nonce)))
    }

    async fn authorization_used_tx(
        &self,
        token: Address,
        authorizer: Address,
        nonce: B256,
        from_block: u64,
        to_block: u64,
    ) -> Result<Option<(B256, TransactionOutcome)>, ChainError> {
        let _ = token;
        let _ = authorizer;
        let state = self.state.lock().unwrap();
        let Some(at_block) = state.authorization_events.get(&nonce).copied() else {
            return Ok(None);
        };
        if at_block < from_block || at_block > to_block {
            return Ok(None);
        }
        Ok(Some((
            mock_authorization_tx_hash(nonce),
            TransactionOutcome {
                succeeded: true,
                block: at_block,
                block_hash: B256::with_last_byte(at_block as u8),
            },
        )))
    }

    async fn payment_settled(&self, payment: Address, _block: u64) -> Result<bool, ChainError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .settled
            .get(&payment)
            .copied()
            .unwrap_or(true))
    }

    async fn payment_settlement_tx(
        &self,
        payment: Address,
        _token: Address,
        from_block: u64,
        to_block: u64,
    ) -> Result<Option<SettlementEvent>, ChainError> {
        let state = self.state.lock().unwrap();
        // A settlement the mock says exists only from some block onward is
        // invisible to ranges that end below it, exactly like a real
        // provider answering for a range without the deployment.
        if state
            .settlement_appears_at
            .get(&payment)
            .is_some_and(|appears_at| to_block < *appears_at)
        {
            return Ok(None);
        }
        if let Some(event) = state.settlement_events.get(&payment) {
            return Ok(Some(*event));
        }
        // Without an override the deployment was this worker's own exact
        // settlement, whose amount the mock finds among its submissions.
        let settled = state
            .submissions
            .iter()
            .flat_map(|submission| &submission.sweeps)
            .find(|sweep| payment_address_of(sweep) == payment)
            .map_or(U256::ZERO, |sweep| sweep.amount);
        Ok(Some(SettlementEvent {
            transaction_hash: B256::repeat_byte(0xCC),
            block_number: from_block,
            block_hash: block_hash(from_block),
            transaction_index: 0,
            settled: Some(settled),
            recovered: U256::ZERO,
        }))
    }

    async fn probe_failure(
        &self,
        _currency: gum_core::Currency,
        _token: Address,
        payment: Address,
        _receiver: Address,
        _recovery: Address,
        _block: u64,
    ) -> Result<FailureProbe, ChainError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .probes
            .get(&payment)
            .copied()
            .unwrap_or_default())
    }

    async fn code_hash(&self, address: Address) -> Result<B256, ChainError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .code_hashes
            .get(&address)
            .copied()
            .unwrap_or_else(|| mock_code_hash(address)))
    }

    async fn batch_sweeper_factory(&self, batch_sweeper: Address) -> Result<Address, ChainError> {
        Ok(self
            .state
            .lock()
            .unwrap()
            .sweeper_factories
            .get(&batch_sweeper)
            .copied()
            .unwrap_or(factory().0))
    }
}

#[async_trait]
impl ChainExecutor for MockChain {
    fn signers(&self) -> Vec<Address> {
        self.state.lock().unwrap().signers.clone()
    }

    async fn signer_nonce(&self, signer: Address, pending: bool) -> Result<u64, ChainError> {
        let state = self.state.lock().unwrap();
        if pending && let Some(nonce) = state.pending_nonces.get(&signer) {
            return Ok(*nonce);
        }
        Ok(state.mined_nonces.get(&signer).copied().unwrap_or(0))
    }

    async fn signer_balance(&self, signer: Address) -> Result<U256, ChainError> {
        if self.state.lock().unwrap().balance_errors.contains(&signer) {
            return Err(ChainError::Transient(format!(
                "balance read throttled for {signer}"
            )));
        }
        Ok(self
            .state
            .lock()
            .unwrap()
            .balances
            .get(&signer)
            .copied()
            .unwrap_or(U256::from(10u64).pow(U256::from(18u64))))
    }

    async fn estimate_fees(&self) -> Result<FeeEstimate, ChainError> {
        let mut state = self.state.lock().unwrap();
        state.fee_requests += 1;
        Ok(state.fees)
    }

    async fn prepare_sweep_batch(
        &self,
        signer: Address,
        _batch_sweeper: Address,
        sweeps: &[SweepRequest],
        nonce: u64,
        gas_limit: u64,
        fees: FeeEstimate,
    ) -> Result<PreparedSweepTransaction, ChainError> {
        let mut state = self.state.lock().unwrap();
        if !state.signers.contains(&signer) && !state.dedicated_signers.contains(&signer) {
            return Err(ChainError::Transient(format!("no key for signer {signer}")));
        }
        if state.prepare_errors.contains(&signer) {
            return Err(ChainError::Transient(format!(
                "signing failed for {signer}"
            )));
        }
        state.next_transaction_id += 1;
        let tx_hash = B256::with_last_byte(state.next_transaction_id);
        let raw = Bytes::copy_from_slice(tx_hash.as_slice());
        state.prepared.insert(
            tx_hash,
            Submission {
                signer,
                nonce,
                gas_limit,
                fees,
                sweeps: sweeps.to_vec(),
                tx_hash,
                to: None,
                calldata: Bytes::new(),
            },
        );
        Ok(PreparedSweepTransaction { hash: tx_hash, raw })
    }

    async fn prepare_call(
        &self,
        signer: Address,
        to: Address,
        calldata: Bytes,
        nonce: u64,
        gas_limit: u64,
        fees: FeeEstimate,
    ) -> Result<PreparedSweepTransaction, ChainError> {
        let mut state = self.state.lock().unwrap();
        if !state.signers.contains(&signer) && !state.dedicated_signers.contains(&signer) {
            return Err(ChainError::Transient(format!("no key for signer {signer}")));
        }
        if state.prepare_errors.contains(&signer) {
            return Err(ChainError::Transient(format!(
                "signing failed for {signer}"
            )));
        }
        state.next_transaction_id += 1;
        let tx_hash = B256::with_last_byte(state.next_transaction_id);
        let raw = Bytes::copy_from_slice(tx_hash.as_slice());
        state.prepared.insert(
            tx_hash,
            Submission {
                signer,
                nonce,
                gas_limit,
                fees,
                sweeps: Vec::new(),
                tx_hash,
                to: Some(to),
                calldata,
            },
        );
        Ok(PreparedSweepTransaction { hash: tx_hash, raw })
    }

    async fn broadcast_sweep_transaction(
        &self,
        transaction: &PreparedSweepTransaction,
    ) -> Result<(), ChainError> {
        let mut state = self.state.lock().unwrap();
        if let Some(error) = state.submit_error.take() {
            return Err(error());
        }
        if state
            .submissions
            .iter()
            .any(|submission| submission.tx_hash == transaction.hash)
        {
            return Ok(());
        }
        let submission = state.prepared[&transaction.hash].clone();
        let tx_hash = submission.tx_hash;
        let signer = submission.signer;
        let nonce = submission.nonce;
        let sweeps = submission.sweeps.clone();
        state.submissions.push(submission);
        if let Some(block) = state.mine_at {
            let outcomes = sweeps
                .iter()
                .map(|sweep| {
                    let payment = payment_address_of(sweep);
                    let outcome =
                        state
                            .next_outcomes
                            .remove(&payment)
                            .unwrap_or(SweepOutcome::Settled {
                                amount: sweep.amount,
                                recovered_amount: U256::ZERO,
                            });
                    (payment, outcome)
                })
                .collect();
            let succeeded = state.next_receipt_succeeds;
            state.mined_nonces.insert(signer, nonce + 1);
            state.receipts.insert(
                tx_hash,
                SweepReceipt {
                    succeeded,
                    block,
                    block_hash: block_hash(block),
                    transaction_index: 1,
                    outcomes,
                },
            );
        }
        Ok(())
    }
}
