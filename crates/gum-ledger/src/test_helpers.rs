//! Fixtures for tests of this crate and of the services built on it
//! (`gum-server` enables the `test-helpers` feature as a dev-dependency).
//! Everything here goes through the real write paths; nothing inserts a
//! row the repositories could not have produced.

use alloy_primitives::{Address, B256, U256, address};
use gum_core::{
    Amount, BeneficiaryAddress, CanonicalIssuanceSnapshot, ChainId, Currency, FactoryAddress,
    Invoice, NetworkTerms, Party, PayerAttestation, PayerPolicy, PayerWalletAttestation,
    TokenAddress, sign_payer_attestation, wallet_of,
};
use sqlx::PgPool;
use sqlx::types::chrono::Utc;
use uuid::Uuid;

use crate::{
    AccountId, BindPayerWallet, CreateInvoiceInput, DbAttachment, DbInvoice, InvoiceRepository,
};

/// The wallet every test payer signs with unless it says otherwise.
pub const TEST_PAYER_KEY: [u8; 32] = [7u8; 32];

/// The token every test request on the fixture chain is denominated in.
pub const TEST_TOKEN: Address = address!("0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512");

/// The networks a test request offers: the local fixture chain first
/// (where `bind_for_test` binds) and a second chain nobody pays on.
pub fn test_networks(chain_id: u64) -> Vec<NetworkTerms> {
    vec![
        NetworkTerms {
            chain_id: ChainId(chain_id),
            token: TokenAddress(address!("0xe7f1725E7734CE288F8367e1Bb143E90bb3F0512")),
            factory: FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3")),
        },
        // Above every chain id a test uses, so canonical order keeps the
        // test's own chain first.
        NetworkTerms {
            chain_id: ChainId(84_532),
            token: TokenAddress(address!("0x036CbD53842c5426634e7929541eC2318f3dCF7e")),
            factory: FactoryAddress(address!("0x5FbDB2315678afecb367f032d93F642f64180aa3")),
        },
    ]
}

pub fn test_payer_wallet() -> Address {
    wallet_of(&TEST_PAYER_KEY)
}

/// The attestation `key`'s wallet would sign for `invoice` on its first
/// network, in a session that was issued `nonce`.
pub fn test_attestation(invoice: &Invoice, key: &[u8; 32], nonce: B256) -> PayerWalletAttestation {
    let message = PayerAttestation::new(
        invoice.attribution_hash,
        wallet_of(key),
        nonce,
        invoice.expiration_timestamp,
    );
    let network = invoice.networks[0];
    sign_payer_attestation(key, &message, network.chain_id.0, network.factory.0)
}

/// Bind `key`'s wallet to the invoice behind `id` through the real write
/// path: a payer session, a challenge, and the attestation. Returns the
/// bound row.
pub async fn bind_for_test(pool: &PgPool, id: Uuid, key: &[u8; 32]) -> DbInvoice {
    let repo = InvoiceRepository::new(pool.clone());
    let sessions = crate::PayerSessionRepository::new(pool.clone());
    let row = repo.find_by_id(id).await.unwrap().unwrap();
    let invoice = Invoice::try_from(&row).unwrap();
    let session = sessions.create(id, crate::PAYER_SESSION_TTL).await.unwrap();
    let chain = invoice.networks[0].chain_id;
    let challenge = sessions
        .issue_wallet_challenge(session.id, chain.0, std::time::Duration::from_secs(600))
        .await
        .unwrap();
    let attestation = test_attestation(&invoice, key, challenge.nonce);
    let binding = invoice
        .bind_payer_wallet(chain, attestation, Utc::now().to_rfc3339())
        .unwrap();
    match repo
        .bind_payer_wallet(id, session.id, &binding, Utc::now())
        .await
        .unwrap()
    {
        BindPayerWallet::Bound(row) => row,
        other => panic!("test invoice could not be bound: {other:?}"),
    }
}

/// Issue and bind in one go, for tests that need a payable address.
pub async fn insert_bound(
    pool: &PgPool,
    input: &CreateInvoiceInput,
    attachment_id: Option<Uuid>,
) -> DbInvoice {
    let issued = InvoiceRepository::new(pool.clone())
        .insert_issued(input, attachment_id)
        .await
        .unwrap()
        .row;
    bind_for_test(pool, issued.id, &TEST_PAYER_KEY).await
}

pub fn party(name: &str) -> Party {
    Party {
        name: name.into(),
        email: None,
        details: None,
    }
}

pub async fn account(pool: &PgPool, id: u128) -> AccountId {
    let id = Uuid::from_u128(id);
    sqlx::query(
            "INSERT INTO accounts (id, api_key_hash, api_key_hint) VALUES ($1, $2, 'hint') ON CONFLICT (id) DO NOTHING",
        )
        .bind(id)
        .bind(id.as_bytes().repeat(2))
        .execute(pool)
        .await
        .unwrap();
    AccountId(id)
}

/// Issue a fresh domain invoice (new id every call, as a retried request
/// would produce) and project it for the DB. Unbound: see `insert_bound`.
pub fn issuance_input(
    owner: AccountId,
    key: &str,
    attachment: Option<&DbAttachment>,
) -> CreateInvoiceInput {
    let networks = test_networks(1);
    let beneficiary = BeneficiaryAddress(address!("0x70997970C51812dc3A010C7d01b50e0d17dc79C8"));
    let amount = Amount(U256::from(1_000_000));
    let mut snapshot = CanonicalIssuanceSnapshot::new(
        party("Acme"),
        party("Globex"),
        PayerPolicy::Permissionless,
        Currency::Usdc,
        &networks,
        beneficiary,
        amount,
        4_000_000_000,
    );
    snapshot.attachment = attachment.and_then(DbAttachment::commitment);
    let invoice = Invoice::issue(
        Currency::Usdc,
        &networks,
        beneficiary,
        amount,
        4_000_000_000,
        snapshot,
    )
    .unwrap();
    CreateInvoiceInput::from_invoice(&invoice, owner, key.into(), 6, 3_600, "in:3600".into())
}
