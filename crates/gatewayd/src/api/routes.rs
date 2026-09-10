use std::time::Duration;

use axum::http::{HeaderName, Method, header};
use axum::routing::{delete, get, post, put};
use axum::{Router, middleware};
use tower_http::cors::{AllowOrigin, Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;
use tracing::field;

use crate::api::accounts;
use crate::api::admin;
use crate::api::attachments;
use crate::api::auth;
use crate::api::customers;
use crate::api::deposit_requests;
use crate::api::health;
use crate::api::issuers;
use crate::api::merchant_session;
use crate::api::payer;
use crate::api::payer_verification;
use crate::api::payer_wallet;
use crate::api::proof;
use crate::api::status;
use crate::api::verification;
use crate::api::wallet_pregeneration;
use crate::api::webhooks;
use crate::api::{middleware as api_middleware, openapi};
use crate::state::AppState;

pub fn router(state: AppState) -> Router {
    // The dashboard is a browser app served from the public origin, and it is
    // the only origin the merchant API answers to: a one-element list, so any
    // other origin gets no allow header at all rather than a mismatching one.
    // The layer sits outside the authentication route layer so preflights,
    // which carry no credential, are answered before it.
    tracing::info!(
        origin = state.payer.origin(),
        "merchant routes answer browser requests from the dashboard origin"
    );
    let merchant_cors = CorsLayer::new()
        .allow_origin(AllowOrigin::list([state.payer.origin_header()]))
        // PUT and DELETE joined the list with issuer identities: the dashboard
        // sets an identity's wallets with PUT and drops a saved wallet with
        // DELETE, and a method missing here fails the preflight, not the call.
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::PUT,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ACCEPT,
            HeaderName::from_static("idempotency-key"),
        ])
        .max_age(Duration::from_secs(86_400));

    // Merchant routes: an API key or a dashboard session, one account either
    // way. The key routes sit here too, under the same authentication; they
    // refuse the API-key form of it themselves (`auth::Session`), so a key
    // can never mint or revoke another.
    let authenticated = Router::new()
        .route("/v1/account", get(accounts::get_account))
        .route(
            "/v1/account/api-key",
            post(accounts::issue).delete(accounts::revoke),
        )
        .route(
            "/v1/deposit-requests",
            post(deposit_requests::create_deposit_request)
                .get(deposit_requests::list_deposit_requests),
        )
        .route(
            "/v1/deposit-requests/{id}",
            get(deposit_requests::get_deposit_request),
        )
        .route(
            "/v1/deposit-requests/{id}/cancel",
            post(deposit_requests::cancel_deposit_request),
        )
        .route(
            "/v1/deposit-requests/{id}/onboarding-deposit",
            post(deposit_requests::onboarding_deposit),
        )
        .route(
            "/v1/deposit-requests/{id}/transfers",
            get(deposit_requests::transfers),
        )
        .route(
            "/v1/deposit-requests/{id}/attachment",
            get(attachments::deposit_request_attachment),
        )
        .route(
            "/v1/deposit-requests/{id}/request.pdf",
            get(deposit_requests::request_pdf),
        )
        .route("/v1/deposit-requests/{id}/proof", get(proof::get_proof))
        .route(
            "/v1/deposit-requests/{id}/verification",
            get(verification::merchant_detail),
        )
        .route(
            "/v1/deposit-requests/{id}/client-secret",
            post(merchant_session::mint),
        )
        .route(
            "/v1/deposit-requests/{id}/preview-session",
            post(deposit_requests::preview_session),
        )
        .route(
            "/v1/customers",
            post(customers::create).get(customers::list),
        )
        .route("/v1/issuers", post(issuers::create).get(issuers::list))
        .route(
            "/v1/issuers/{id}",
            get(issuers::get)
                .patch(issuers::update)
                .delete(issuers::delete),
        )
        .route(
            "/v1/issuers/{id}/verify/email/start",
            post(issuers::start_email_verification),
        )
        .route(
            "/v1/issuers/{id}/verify/email/confirm",
            post(issuers::confirm_email_verification),
        )
        .route(
            "/v1/issuers/{id}/payout-addresses",
            put(issuers::set_payout_addresses),
        )
        .route(
            "/v1/payout-addresses",
            post(issuers::create_payout_address).get(issuers::list_payout_addresses),
        )
        .route(
            "/v1/payout-addresses/{id}",
            delete(issuers::delete_payout_address),
        )
        .route(
            "/v1/customers/{id}",
            get(customers::get).patch(customers::update),
        )
        .route("/v1/attachments", post(attachments::create))
        .route("/v1/attachments/{id}/finalize", post(attachments::finalize))
        .route("/v1/status", get(status::get))
        .route("/v1/webhooks", post(webhooks::add).get(webhooks::list))
        .route(
            "/v1/webhooks/{id}",
            get(webhooks::get).delete(webhooks::remove),
        )
        .route("/v1/webhooks/{id}/test", post(webhooks::test))
        .route("/v1/webhook-deliveries", get(webhooks::deliveries))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_account,
        ))
        .layer(RequestBodyLimitLayer::new(64 * 1024))
        .layer(merchant_cors.clone());

    // Pregenerating a wallet happens before the merchant has a session — the
    // sign-up dialog calls this the moment they submit their email, in
    // parallel with Privy sending the code — so it cannot sit behind
    // `require_account` like the routes above. Still only the dashboard
    // origin, and its own tiny body limit: one email, nothing else.
    let merchant_onboarding = Router::new()
        .route(
            "/v1/wallets/pregenerate",
            post(wallet_pregeneration::pregenerate),
        )
        .layer(RequestBodyLimitLayer::new(1024))
        .layer(merchant_cors);

    let administration = Router::new()
        .route(
            "/v1/admin/deposit-requests/{id}/release",
            post(admin::release),
        )
        .route_layer(middleware::from_fn(auth::require_admin))
        .layer(RequestBodyLimitLayer::new(16 * 1024));

    // A payment link is public by design: anyone holding it may read the payment
    // and fulfil it. These routes accept no API key and return no merchant data,
    // so allowing any browser origin grants exactly what curl already has — and
    // it is what lets a merchant render their own checkout, as the docs invite.
    // This must never be extended to the API-key routes.
    let payer_session_header = HeaderName::from_static(payer_verification::PAYER_SESSION_HEADER);
    let payer = Router::new()
        .route("/v1/payer/deposit-requests/{id}", get(payer::get))
        .route("/v1/payer/deposit-requests/{id}/qr", get(payer::qr))
        .route(
            "/v1/payer/deposit-requests/{id}/attachment",
            get(payer::attachment),
        )
        .route(
            "/v1/payer/deposit-requests/{id}/verify",
            get(payer_verification::status),
        )
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods([Method::GET, Method::HEAD])
                .allow_headers([payer_session_header.clone()])
                .max_age(Duration::from_secs(86_400)),
        );

    // Verification writes send a code to the merchant's customer, mint
    // sessions, and bind wallets, so unlike the reads they answer one
    // browser origin only: the hosted checkout (product plan §7.1). No
    // `Any` here, ever.
    let payer_verification = Router::new()
        .route(
            "/v1/payer/deposit-requests/{id}/verify/email/start",
            post(payer_verification::start_email),
        )
        .route(
            "/v1/payer/deposit-requests/{id}/verify/email/confirm",
            post(payer_verification::confirm_email),
        )
        .route(
            "/v1/payer/deposit-requests/{id}/wallet/challenge",
            post(payer_wallet::challenge),
        )
        .route(
            "/v1/payer/deposit-requests/{id}/wallet/attest",
            post(payer_wallet::attest),
        )
        .route(
            "/v1/payer/deposit-requests/{id}/session",
            post(merchant_session::exchange),
        )
        .layer(RequestBodyLimitLayer::new(8 * 1024))
        .layer(
            CorsLayer::new()
                .allow_origin(AllowOrigin::list([state.payer.checkout_origin_header()]))
                .allow_methods([Method::POST, Method::OPTIONS])
                .allow_headers([header::CONTENT_TYPE, payer_session_header])
                .max_age(Duration::from_secs(86_400)),
        );

    Router::new()
        .route("/health", get(health::health))
        .merge(payer)
        .merge(payer_verification)
        .merge(merchant_onboarding)
        .route("/openapi.json", get(openapi::spec))
        .route("/docs", get(openapi::reference))
        .route("/api", get(openapi::reference))
        .route("/api/openapi.json", get(openapi::spec))
        .merge(authenticated)
        .merge(administration)
        .with_state(state)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::extract::Request| {
                    let request_id = request
                        .extensions()
                        .get::<api_middleware::RequestId>()
                        .map(|value| value.0.as_str())
                        .unwrap_or("unknown");
                    tracing::info_span!(
                        "http_request", method = %request.method(), path = %request.uri().path(),
                        request_id, status = field::Empty, latency_ms = field::Empty,
                        account_id = field::Empty
                    )
                })
                .on_response(
                    |response: &axum::response::Response,
                     latency: std::time::Duration,
                     span: &tracing::Span| {
                        span.record("status", response.status().as_u16());
                        span.record("latency_ms", latency.as_millis());
                        tracing::info!(parent: span, "request completed");
                    },
                ),
        )
        .layer(middleware::from_fn(api_middleware::request_id))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};

    use alloy_primitives::Address;
    use alloy_signer_local::PrivateKeySigner;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode, header};
    use gateway_core::{
        AttachmentId, ChainId, CustomerId, Invoice, IssuerId, PayoutAddressId, ProofError,
        ProofOfPayment, WebhookId, verify_proof,
    };
    use gateway_db::{AccountId, AccountRepository, InvoiceRepository};
    use jsonwebtoken::{DecodingKey, EncodingKey};
    use rsa::pkcs8::{EncodePrivateKey, EncodePublicKey, LineEnding};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use sqlx::PgPool;
    use tower::ServiceExt;
    use uuid::Uuid;

    use super::*;
    use crate::api::auth::testing::PrivyApp;
    use crate::attachments::memory::MemoryObjectStorage;
    use crate::attachments::{AttachmentStore, CLEAN_SCAN, SCAN_STATUS_TAG};
    use crate::attestation::VerificationAttestor;
    use crate::payer_identity::PayerVerification;
    use crate::payer_identity::testing::{FakeTenant, OTP};
    use crate::pregenerated_wallet::WalletPregenerator;
    use crate::pregenerated_wallet::testing::FakePregenerator;

    const KEY: &str = "payday_live_0123456789abcdef0123456789abcdef";
    /// The wallet the test payer signs attestations with.
    const PAYER_KEY: [u8; 32] = [7u8; 32];
    /// A minimal PDF, byte for byte what the e2e suite uploads.
    const PDF: &[u8] = b"%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF";

    fn attestor() -> VerificationAttestor {
        VerificationAttestor::local(PrivateKeySigner::from_slice(&[7u8; 32]).unwrap())
    }

    /// The deployment under test offers two networks through one factory:
    /// chain 1 with the zero token, chain 2 with another.
    fn test_networks(factory: Address) -> Arc<gateway_core::ChainRegistry> {
        let chain = |chain_id: u64, usdc: Address| gateway_core::ChainConfig {
            chain_id,
            usdc,
            factory,
            batch_sweeper: Address::repeat_byte(0x55),
            factory_code_hash: alloy_primitives::B256::ZERO,
            batch_sweeper_code_hash: alloy_primitives::B256::ZERO,
            usdc_start_block: 0,
            finality_source: gateway_core::FinalitySource::Finalized,
            finality_confirmations: 0,
            block_time_ms: 1000,
            log_range_size: 100,
            scan: gateway_core::ScanMode::Full,
            explorer_base_url: None,
        };
        Arc::new(
            gateway_core::ChainRegistry::new(vec![
                chain(1, Address::ZERO),
                chain(2, Address::repeat_byte(0x02)),
            ])
            .unwrap(),
        )
    }

    /// The router plus the memory bucket behind it, so a test can play the
    /// uploading client and the malware scanner.
    struct TestApp {
        router: Router,
        storage: Arc<MemoryObjectStorage>,
    }

    fn test_state(
        pool: PgPool,
        accounts: AccountRepository,
        merchant_verifier: Option<auth::PrivyVerifier>,
        factory: Address,
        payer_verification: Option<PayerVerification>,
        pregenerated_wallets: Option<Arc<dyn WalletPregenerator>>,
    ) -> (AppState, Arc<MemoryObjectStorage>) {
        let storage = Arc::new(MemoryObjectStorage::default());
        let store = AttachmentStore::new(storage.clone(), Duration::from_secs(300));
        let state = AppState::new(
            InvoiceRepository::new(pool),
            accounts,
            merchant_verifier,
            test_networks(factory),
            payer_access(),
            "payday_live_".into(),
            Some(WEBHOOK_KEY),
            Some(store),
            Some(attestor()),
            payer_verification,
            None,
            pregenerated_wallets,
        );
        (state, storage)
    }

    /// The webhook secret-encryption key the test deployment is configured
    /// with, so endpoints can be registered.
    const WEBHOOK_KEY: [u8; 32] = [9u8; 32];

    /// A payer audience with its own signing key, and the stand-in tenant
    /// that mints tokens for it.
    fn payer_verification() -> (PayerVerification, Arc<FakeTenant>) {
        let private = rsa::RsaPrivateKey::new(&mut rand_08::thread_rng(), 2048).unwrap();
        let private_pem = private.to_pkcs8_pem(LineEnding::LF).unwrap();
        let public_pem = private
            .to_public_key()
            .to_public_key_pem(LineEnding::LF)
            .unwrap();
        let tenant = Arc::new(FakeTenant {
            issuer: "https://payer.issuer/".into(),
            audience: "https://api.payday.sh/payer".into(),
            client_id: "payday-payer".into(),
            kid: "payer-key".into(),
            key: EncodingKey::from_rsa_pem(private_pem.as_bytes()).unwrap(),
            started: Default::default(),
            outage: Default::default(),
        });
        let verifier = auth::Auth0Verifier::for_test(
            "https://payer.issuer/",
            "https://api.payday.sh/payer",
            "payday-payer",
            "payer-key",
            DecodingKey::from_rsa_pem(public_pem.as_bytes()).unwrap(),
        );
        (
            PayerVerification::new(tenant.clone(), verifier, [0x42; 32]),
            tenant,
        )
    }

    async fn app_with_payer_verification(pool: PgPool) -> (Router, Arc<FakeTenant>) {
        let (verification, tenant) = payer_verification();
        let app = build_with(pool, None, Address::ZERO, Some(verification), None)
            .await
            .router;
        (app, tenant)
    }

    async fn app_with_pregeneration(pool: PgPool) -> (Router, Arc<FakePregenerator>) {
        let pregenerator = Arc::new(FakePregenerator::default());
        let app = build_with(
            pool,
            None,
            Address::ZERO,
            None,
            Some(pregenerator.clone() as Arc<dyn WalletPregenerator>),
        )
        .await
        .router;
        (app, pregenerator)
    }

    /// The Privy DID and the embedded wallet of the merchant the session
    /// tests sign in as.
    const MERCHANT_DID: &str = "did:privy:clmerchant000000000000001";
    const MERCHANT_WALLET: &str = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";

    async fn app(pool: PgPool) -> Router {
        build(pool, None, Address::ZERO).await.router
    }

    /// The same deployment reconfigured with another factory.
    async fn app_with_factory(pool: PgPool, factory: Address) -> Router {
        build(pool, None, factory).await.router
    }

    async fn test_app(pool: PgPool) -> TestApp {
        build(pool, None, Address::ZERO).await
    }

    async fn build(
        pool: PgPool,
        merchant_verifier: Option<auth::PrivyVerifier>,
        factory: Address,
    ) -> TestApp {
        build_with(pool, merchant_verifier, factory, None, None).await
    }

    async fn build_with(
        pool: PgPool,
        merchant_verifier: Option<auth::PrivyVerifier>,
        factory: Address,
        payer_verification: Option<PayerVerification>,
        pregenerated_wallets: Option<Arc<dyn WalletPregenerator>>,
    ) -> TestApp {
        let accounts = AccountRepository::new(pool.clone());
        if accounts
            .find_by_identity("https://test.issuer/", "email|test-user")
            .await
            .unwrap()
            .is_none()
        {
            let _ = accounts
                .issue_api_key_with_email(
                    "https://test.issuer/",
                    "email|test-user",
                    None,
                    &Uuid::now_v7().to_string(),
                    KEY,
                    "merchant@example.com",
                )
                .await
                .unwrap();
        }
        let (state, storage) = test_state(
            pool,
            accounts,
            merchant_verifier,
            factory,
            payer_verification,
            pregenerated_wallets,
        );
        TestApp {
            router: router(state),
            storage,
        }
    }

    const CHECKOUT_ORIGIN: &str = "http://127.0.0.1:3000";
    const SESSION_HEADER: &str = "payday-payer-session";

    /// A payer read, with the session header when one is given.
    fn payer_get(path: &str, session: Option<&str>) -> Request<Body> {
        let mut request = Request::get(path);
        if let Some(session) = session {
            request = request.header(SESSION_HEADER, session);
        }
        request.body(Body::empty()).unwrap()
    }

    /// A verification write from the hosted checkout.
    fn payer_post(path: &str, session: Option<&str>, body: Option<&Value>) -> Request<Body> {
        let mut request = Request::post(path)
            .header(header::ORIGIN, CHECKOUT_ORIGIN)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(session) = session {
            request = request.header(SESSION_HEADER, session);
        }
        request
            .body(Body::from(body.map(Value::to_string).unwrap_or_default()))
            .unwrap()
    }

    /// The payer's wallet step over HTTP: ask for the challenge, sign it
    /// with `key`, and attest. Returns the unlocked payer response (which
    /// now carries the address) and the session the binding was made in.
    async fn bind_wallet(
        app: &Router,
        id: &str,
        session: Option<&str>,
        key: &[u8; 32],
    ) -> (Value, String) {
        let wallet = gateway_core::wallet_of(key);
        let challenge = app
            .clone()
            .oneshot(payer_post(
                &format!("/v1/payer/deposit-requests/{id}/wallet/challenge"),
                session,
                Some(&json!({"wallet": wallet.to_checksum(None), "chain_id": "1"})),
            ))
            .await
            .unwrap();
        assert_eq!(challenge.status(), StatusCode::OK, "wallet challenge");
        let challenge = json_body(challenge).await;
        let session = challenge["payer_session"].as_str().unwrap().to_owned();
        let signature = sign_challenge(&challenge, key);
        let attested = app
            .clone()
            .oneshot(payer_post(
                &format!("/v1/payer/deposit-requests/{id}/wallet/attest"),
                Some(&session),
                Some(&json!({"wallet": wallet.to_checksum(None), "signature": signature})),
            ))
            .await
            .unwrap();
        assert_eq!(attested.status(), StatusCode::OK, "wallet attest");
        (json_body(attested).await, session)
    }

    /// Sign the typed data a challenge response carries, as a wallet would.
    fn sign_challenge(challenge: &Value, key: &[u8; 32]) -> String {
        let typed: gateway_core::PayerAttestationTypedData =
            serde_json::from_value(challenge["typed_data"].clone()).unwrap();
        let message = gateway_core::PayerAttestation::new(
            typed.message.attribution_hash.parse().unwrap(),
            typed.message.wallet.parse().unwrap(),
            typed.message.nonce.parse().unwrap(),
            typed.message.expires_at,
        );
        gateway_core::sign_payer_attestation(
            key,
            &message,
            typed.domain.chain_id,
            typed.domain.verifying_contract.parse().unwrap(),
        )
        .signature
    }

    async fn create_gated(
        app: &Router,
        key: &str,
        policy: Value,
        heading: &str,
    ) -> (String, Value) {
        let mut body = valid_body();
        body["heading"] = json!(heading);
        body["reference"] = json!("INV-9");
        body["payer_policy"] = policy;
        let created = app
            .clone()
            .oneshot(create_request(KEY, key, &body))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = json_body(created).await;
        (created["id"].as_str().unwrap().to_owned(), created)
    }

    /// Start email verification and return the session token.
    /// Prove the mailbox for `session` with the stand-in tenant's code.
    async fn confirm_email(app: &Router, id: &str, session: &str) {
        let confirmed = app
            .clone()
            .oneshot(payer_post(
                &format!("/v1/payer/deposit-requests/{id}/verify/email/confirm"),
                Some(session),
                Some(&json!({"otp": OTP})),
            ))
            .await
            .unwrap();
        assert_eq!(confirmed.status(), StatusCode::OK);
    }

    async fn payer_read(app: &Router, id: &str, session: Option<&str>) -> Value {
        json_body(
            app.clone()
                .oneshot(payer_get(
                    &format!("/v1/payer/deposit-requests/{id}"),
                    session,
                ))
                .await
                .unwrap(),
        )
        .await
    }

    async fn start_session(app: &Router, id: &str) -> String {
        let started = app
            .clone()
            .oneshot(payer_post(
                &format!("/v1/payer/deposit-requests/{id}/verify/email/start"),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(started.status(), StatusCode::OK);
        assert_eq!(started.headers()[header::CACHE_CONTROL], "no-store");
        let started = json_body(started).await;
        assert!(started["expires_at"].is_string());
        started["payer_session"].as_str().unwrap().to_owned()
    }

    /// A second account with its own key.
    async fn other_account(pool: &PgPool, key: &str) -> AccountId {
        AccountRepository::new(pool.clone())
            .issue_api_key_with_email(
                "https://issuer.example/",
                &format!("email|{key}"),
                None,
                &format!("event-{key}"),
                key,
                "other@example.com",
            )
            .await
            .unwrap()
            .account_id
    }

    fn get_request(key: &str, path: &str) -> Request<Body> {
        Request::get(path)
            .header(header::AUTHORIZATION, format!("Bearer {key}"))
            .body(Body::empty())
            .unwrap()
    }

    fn json_request(method: &str, key: &str, path: &str, body: &Value) -> Request<Body> {
        Request::builder()
            .method(method)
            .uri(path)
            .header(header::AUTHORIZATION, format!("Bearer {key}"))
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    fn sha256_hex(bytes: &[u8]) -> String {
        format!("0x{}", hex::encode(Sha256::digest(bytes)))
    }

    fn payer_access() -> payer::PayerAccess {
        payer::PayerAccess::new("http://127.0.0.1:3000", vec![], None).unwrap()
    }

    fn unix_now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    /// A request body that passes every validation rule; tests override one field.
    fn valid_body() -> Value {
        json!({
            "payout_address": "0x0000000000000000000000000000000000000002",
            "amount": "1",
            "expires_in": 3600,
            "issuer": {"name": "Acme"},
            "payer": {"name": "Globex"},
            "payer_policy": {"mode": "permissionless"}
        })
    }

    /// The account `app()` provisions for `KEY`.
    async fn test_account(pool: &PgPool) -> AccountId {
        AccountRepository::new(pool.clone())
            .find_by_identity("https://test.issuer/", "email|test-user")
            .await
            .unwrap()
            .expect("app() provisions the test account")
    }

    /// A finalized upload, exactly as the attachment routes leave it.
    async fn ready_attachment(pool: &PgPool, account: AccountId, filename: &str) -> AttachmentId {
        let id = Uuid::now_v7();
        sqlx::query(
            r#"INSERT INTO invoice_attachments
                 (id, account_id, object_key, original_filename, mime_type, byte_length, sha256,
                  status, scan_result, finalized_at)
               VALUES ($1, $2, $3, $4, 'application/pdf', 1234, $5, 'ready', 'NO_THREATS_FOUND', now())"#,
        )
        .bind(id)
        .bind(account.0)
        .bind(format!("{}/{id}.pdf", account.0))
        .bind(filename)
        .bind([0xAB_u8; 32].as_slice())
        .execute(pool)
        .await
        .unwrap();
        AttachmentId(id)
    }

    /// An upload that was staged but never finalized.
    async fn pending_attachment(pool: &PgPool, account: AccountId) -> AttachmentId {
        let id = Uuid::now_v7();
        sqlx::query(
            "INSERT INTO invoice_attachments (id, account_id, object_key, original_filename, mime_type, status) VALUES ($1, $2, $3, 'pending.pdf', 'application/pdf', 'pending_upload')",
        )
        .bind(id)
        .bind(account.0)
        .bind(format!("{}/{id}.pdf", account.0))
        .execute(pool)
        .await
        .unwrap();
        AttachmentId(id)
    }

    async fn customer(pool: &PgPool, account: AccountId, name: &str) -> CustomerId {
        let id = Uuid::now_v7();
        sqlx::query("INSERT INTO customers (id, account_id, name) VALUES ($1, $2, $3)")
            .bind(id)
            .bind(account.0)
            .bind(name)
            .execute(pool)
            .await
            .unwrap();
        CustomerId(id)
    }

    fn create_request(key: &str, idempotency_key: &str, body: &Value) -> Request<Body> {
        Request::post("/v1/deposit-requests")
            .header(header::AUTHORIZATION, format!("Bearer {key}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header("idempotency-key", idempotency_key)
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn json_body(response: axum::response::Response) -> Value {
        let body = to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    /// A Privy app and a dashboard session for the test merchant, embedded
    /// wallet included.
    fn merchant_session() -> (PrivyApp, String) {
        let app = PrivyApp::new();
        let token = app.token(MERCHANT_DID, "merchant@example.com", Some(MERCHANT_WALLET));
        (app, token)
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn health_reflects_database_reachability(pool: PgPool) {
        let app = app(pool.clone()).await;
        let response = app
            .clone()
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        pool.close().await;
        let response = app
            .oneshot(Request::get("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    async fn assert_unauthorized(app: Router, request: Request<Body>) {
        let response = app.oneshot(request).await.unwrap();

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(response.headers()[header::WWW_AUTHENTICATE], "Bearer");
        let body = to_bytes(response.into_body(), 1024).await.unwrap();
        let mut body = serde_json::from_slice::<serde_json::Value>(&body).unwrap();
        assert!(body["request_id"].as_str().is_some_and(|id| !id.is_empty()));
        body.as_object_mut().unwrap().remove("request_id");
        assert_eq!(
            body,
            serde_json::json!({
                "error": {
                    "code": "unauthorized",
                    "message": "A valid bearer API key or dashboard session token is required"
                }
            })
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn get_invoice_rejects_missing_and_wrong_credentials(pool: PgPool) {
        assert_unauthorized(
            app(pool.clone()).await,
            Request::get("/v1/deposit-requests/not-an-id")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
        assert_unauthorized(
            app(pool).await,
            Request::get("/v1/deposit-requests/not-an-id")
                .header(
                    header::AUTHORIZATION,
                    "Bearer payday_live_fedcba9876543210fedcba9876543210",
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn create_invoice_rejects_missing_and_wrong_credentials(pool: PgPool) {
        assert_unauthorized(
            app(pool.clone()).await,
            Request::post("/v1/deposit-requests")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
        assert_unauthorized(
            app(pool).await,
            Request::post("/v1/deposit-requests")
                .header(
                    header::AUTHORIZATION,
                    "Bearer payday_live_fedcba9876543210fedcba9876543210",
                )
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await;
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn invoice_routes_accept_the_configured_credential(pool: PgPool) {
        let response = app(pool)
            .await
            .oneshot(
                Request::get("/v1/deposit-requests/not-an-id")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        // The request reached the handler, which rejects the deliberately
        // malformed invoice ID before making a database query.
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn invoice_routes_reject_oversized_requests(pool: PgPool) {
        let response = app(pool)
            .await
            .oneshot(
                Request::post("/v1/deposit-requests")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(vec![b'x'; 64 * 1024 + 1]))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn create_invoice_returns_settlement_fields_and_a_single_use_address(pool: PgPool) {
        let app = app(pool).await;
        let response = app
            .clone()
            .oneshot(create_request(KEY, "fields", &valid_body()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = json_body(response).await;
        assert_eq!(body["status"], "awaiting_deposit");
        assert_eq!(body["amount"], "1.000000");
        assert_eq!(body["amount_base_units"], "1000000");
        assert_eq!(body["received"], "0.000000");
        assert_eq!(body["received_base_units"], "0");
        assert!(body["settlement_tx_hash"].is_null());
        assert!(body["settled_block"].is_null());
        assert!(body["attention"].is_null());
        // No address until the payer attests a wallet.
        assert!(body["address"].is_null());
        assert!(body["payer_wallet"].is_null());
        assert!(body["recovery_address"].is_null());
        assert!(body["self_settlement"].is_null());
        let id = body["id"].as_str().unwrap();
        assert!(id.starts_with("dr_"));

        let replay = app
            .clone()
            .oneshot(create_request(KEY, "fields", &valid_body()))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        let replay = json_body(replay).await;
        assert_eq!(replay["id"], id);
        assert_eq!(replay["expires_at"], body["expires_at"]);
        assert_eq!(replay["expires_in"], 3_600);

        let rejected_prefix = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/deposit-requests/{}", &id[..16]))
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(rejected_prefix.status(), StatusCode::BAD_REQUEST);
        assert_eq!(
            json_body(rejected_prefix).await["error"]["code"],
            "invalid_request"
        );

        let list_response = app
            .clone()
            .oneshot(
                Request::get("/v1/deposit-requests")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(list_response.status(), StatusCode::OK);
        assert_eq!(
            json_body(list_response).await["deposit_requests"][0]["id"],
            id
        );

        let second = app
            .clone()
            .oneshot(create_request(KEY, "second-fields", &valid_body()))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::CREATED);
        let second_id = json_body(second).await["id"].as_str().unwrap().to_owned();

        let first_page = app
            .clone()
            .oneshot(
                Request::get("/v1/deposit-requests?limit=1")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let first_page = json_body(first_page).await;
        assert_eq!(first_page["deposit_requests"].as_array().unwrap().len(), 1);
        assert_eq!(first_page["deposit_requests"][0]["id"], second_id);
        assert_eq!(first_page["next_cursor"], second_id);
        let second_page = app
            .clone()
            .oneshot(
                Request::get(format!(
                    "/v1/deposit-requests?limit=1&starting_after={second_id}"
                ))
                .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        let second_page = json_body(second_page).await;
        assert_eq!(second_page["deposit_requests"][0]["id"], id);
        assert!(second_page["next_cursor"].is_null());

        let legacy_route = app
            .oneshot(
                Request::get("/v1/invoices")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(legacy_route.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn relative_expiry_replays_after_wall_clock_moves(pool: PgPool) {
        let app = app(pool).await;
        let body = valid_body();
        let first = app
            .clone()
            .oneshot(create_request(KEY, "relative-replay", &body))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::CREATED);
        let first = json_body(first).await;
        tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;

        let replay = app
            .oneshot(create_request(KEY, "relative-replay", &body))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        let replay = json_body(replay).await;
        assert_eq!(replay["id"], first["id"]);
        assert_eq!(replay["expires_at"], first["expires_at"]);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn exact_expiry_replays_after_the_creation_window(pool: PgPool) {
        let app = app(pool).await;
        let mut body = valid_body();
        body.as_object_mut().unwrap().remove("expires_in");
        body["expires_at"] = json!(
            sqlx::types::chrono::DateTime::from_timestamp((unix_now() + 602) as i64, 0)
                .unwrap()
                .to_rfc3339()
        );
        let first = app
            .clone()
            .oneshot(create_request(KEY, "exact-replay", &body))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::CREATED);
        let first = json_body(first).await;
        tokio::time::sleep(std::time::Duration::from_secs(3)).await;

        let replay = app
            .oneshot(create_request(KEY, "exact-replay", &body))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(json_body(replay).await["id"], first["id"]);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn payment_freshness_distinguishes_indexed_cursor_from_finalized_head(pool: PgPool) {
        let app = app(pool.clone()).await;
        let created = app
            .clone()
            .oneshot(create_request(KEY, "freshness", &valid_body()))
            .await
            .unwrap();
        let id = json_body(created).await["id"].as_str().unwrap().to_owned();
        // Freshness is per chain, so an unbound request reports none.
        let unbound = json_body(
            app.clone()
                .oneshot(
                    Request::get(format!("/v1/deposit-requests/{id}"))
                        .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(unbound["as_of"].is_null());
        assert!(unbound["indexer_freshness"]["last_indexed_block"].is_null());
        bind_wallet(&app, &id, None, &PAYER_KEY).await;
        sqlx::query("INSERT INTO indexer_cursor(chain_id,token_address,last_block,last_block_hash,last_block_timestamp) VALUES(1,$1,117,$2,1700000000)")
            .bind(Address::ZERO.as_slice()).bind([1_u8; 32].as_slice()).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO indexer_status(chain_id,token_address,finalized_block,finalized_block_hash,finalized_block_timestamp) VALUES(1,$1,120,$2,1700000012)")
            .bind(Address::ZERO.as_slice()).bind([2_u8; 32].as_slice()).execute(&pool).await.unwrap();

        let response = app
            .oneshot(
                Request::get(format!("/v1/deposit-requests/{id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let payment = json_body(response).await;
        assert_eq!(payment["as_of"]["block"], "117");
        assert_eq!(payment["indexer_freshness"]["last_indexed_block"], "117");
        assert_eq!(payment["indexer_freshness"]["last_finalized_block"], "120");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn address_lookup_cancel_and_bounded_list_shape_work(pool: PgPool) {
        let app = app(pool.clone()).await;
        let body = json!({
            "payout_address": "0x0000000000000000000000000000000000000002",
            "amount": "1",
            "issuer": {"name": "Acme"},
            "payer": {"name": "Globex"},
            "payer_policy": {"mode": "permissionless"},
            "reference": "Order 1234"
        });
        let created = app
            .clone()
            .oneshot(create_request(KEY, "lookup-and-cancel", &body))
            .await
            .unwrap();
        let created = json_body(created).await;
        let id = created["id"].as_str().unwrap();
        // No network until the payer chooses one; the offer lists them all.
        assert!(created["chain"].is_null());
        assert!(created["token"].is_null());
        assert_eq!(created["networks"][0]["chain"]["id"], "1");
        assert_eq!(created["networks"][0]["chain"]["name"], "Ethereum");
        assert_eq!(
            created["networks"][0]["token"]["address"],
            Address::ZERO.to_checksum(None)
        );
        assert_eq!(created["networks"][1]["chain"]["id"], "2");
        assert!(created["recovery_address"].is_null());
        assert_eq!(created["reference"], "Order 1234");
        assert!(created.get("memo").is_none());
        assert_eq!(created["issuer"]["name"], "Acme");
        assert_eq!(created["payer"]["name"], "Globex");
        assert_eq!(created["payer_policy"]["mode"], "permissionless");
        assert_eq!(created["attribution"]["version"], 3);
        assert!(created["attachment"].is_null());
        assert_eq!(created["expires_in"], 86_400);
        // The address exists once the payer binds a wallet, and it is the
        // merchant's lookup key from then on.
        let (bound, _) = bind_wallet(&app, id, None, &PAYER_KEY).await;
        let address = bound["address"].as_str().unwrap();

        let found = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/deposit-requests/{address}"))
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(found.status(), StatusCode::OK);
        assert_eq!(json_body(found).await["id"], id);

        let listed = app
            .clone()
            .oneshot(
                Request::get("/v1/deposit-requests")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let listed = json_body(listed).await;
        assert_eq!(listed["deposit_requests"][0]["id"], id);
        assert!(listed["deposit_requests"][0].get("transfers").is_none());
        assert_eq!(listed["deposit_requests"][0]["payer_name"], "Globex");
        // A summary carries what a list needs to render and link without a
        // second read per row.
        assert_eq!(
            listed["deposit_requests"][0]["deposit_url"],
            created["deposit_url"]
        );
        assert_eq!(
            listed["deposit_requests"][0]["expires_at"],
            created["expires_at"]
        );
        assert!(listed["deposit_requests"][0]["updated_at"].is_string());
        // Every timestamp is RFC 3339 in UTC to the second, `Z` suffixed.
        for field in ["created_at", "updated_at", "expires_at"] {
            let value = created[field].as_str().unwrap();
            assert!(
                value.len() == 20 && value.ends_with('Z'),
                "{field}: {value}"
            );
        }

        // Transfers are enveloped like every other list.
        let transfers = json_body(
            app.clone()
                .oneshot(get_request(
                    KEY,
                    &format!("/v1/deposit-requests/{id}/transfers"),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(transfers["transfers"], json!([]));
        assert_eq!(
            listed["deposit_requests"][0]["payer_policy_mode"],
            "permissionless"
        );
        assert_eq!(listed["deposit_requests"][0]["has_attachment"], false);

        // A retry that names a staged-but-unfinalized upload is a different
        // request from the one issued without an attachment.
        let mut with_pending = body.clone();
        with_pending["attachment_id"] =
            json!(pending_attachment(&pool, test_account(&pool).await).await);
        let conflict = app
            .clone()
            .oneshot(create_request(KEY, "lookup-and-cancel", &with_pending))
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(conflict).await["error"]["code"],
            "idempotency_conflict"
        );

        let cancelled = app
            .oneshot(
                Request::post(format!("/v1/deposit-requests/{id}/cancel"))
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cancelled.status(), StatusCode::OK);
        // Cancel answers with the deposit request itself, like every route.
        let cancelled = json_body(cancelled).await;
        assert_eq!(cancelled["id"], id);
        assert_eq!(cancelled["status"], "awaiting_deposit");
        assert!(cancelled["cancellation_requested_at"].is_string());
        assert!(cancelled.get("deposit_request").is_none());
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn scoped_payment_link_serves_status_and_qr_without_merchant_data(pool: PgPool) {
        let app = app(pool.clone()).await;
        let created = app
            .clone()
            .oneshot(create_request(KEY, "payer-link", &valid_body()))
            .await
            .unwrap();
        let created = json_body(created).await;
        let deposit_url = created["deposit_url"].as_str().unwrap();
        let id = created["id"].as_str().unwrap();
        let uuid = Uuid::parse_str(id.strip_prefix("dr_").unwrap()).unwrap();
        assert!(!deposit_url.contains("token"));
        assert!(deposit_url.ends_with(&format!("/pay/{id}")));

        // Browsers on the checkout origin read these routes directly.
        let cors = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/deposit-requests/{id}"))
                    .header(header::ORIGIN, "https://payday.sh")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cors.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");

        let status = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/deposit-requests/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(status.status(), StatusCode::OK);
        assert_eq!(status.headers()[header::CACHE_CONTROL], "no-store");
        let status = json_body(status).await;
        assert_eq!(status["id"], id);
        assert_eq!(status["status"], "awaiting_deposit");
        assert_eq!(status["content_unlocked"], true);
        assert_eq!(status["issuer_name"], "Acme");
        assert_eq!(status["amount_base_units"], "1000000");
        assert_eq!(status["details"]["payer"]["name"], "Globex");
        assert_eq!(status["payer_policy"]["mode"], "permissionless");
        assert_eq!(status["requirements"]["complete"], true);
        assert!(status["deposit_uri"].is_null(), "no wallet is bound yet");
        let (status, _) = bind_wallet(&app, id, None, &PAYER_KEY).await;
        assert!(status["deposit_uri"].as_str().unwrap().starts_with(
            "ethereum:0x0000000000000000000000000000000000000000@1/transfer?address="
        ));
        for private in [
            "payout_address",
            "recovery_address",
            "refund_address",
            "self_settlement",
            "metadata",
            "customer_id",
            "attribution",
            "expected_email",
            "expected_identity",
        ] {
            assert!(
                status.get(private).is_none(),
                "payer response leaked {private}"
            );
        }

        let qr = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/deposit-requests/{id}/qr"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(qr.status(), StatusCode::OK);
        assert_eq!(
            qr.headers()[header::CONTENT_TYPE],
            "image/svg+xml; charset=utf-8"
        );
        assert_eq!(qr.headers()[header::CACHE_CONTROL], "no-store");

        sqlx::query("UPDATE invoices SET confirmed_received = '250000' WHERE id = $1")
            .bind(uuid)
            .execute(&pool)
            .await
            .unwrap();
        let partial = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/deposit-requests/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let partial = json_body(partial).await;
        assert_eq!(partial["remaining_base_units"], "750000");
        assert_eq!(partial["status"], "partially_deposited");
        assert_eq!(partial["payable"], true);
        assert!(
            partial["deposit_uri"]
                .as_str()
                .unwrap()
                .ends_with("uint256=750000")
        );

        sqlx::query("UPDATE invoices SET status = 'expired' WHERE id = $1")
            .bind(uuid)
            .execute(&pool)
            .await
            .unwrap();
        let expired = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/deposit-requests/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let expired = json_body(expired).await;
        assert_eq!(expired["payable"], false);
        assert!(expired["deposit_uri"].is_null());
        let closed_qr = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/deposit-requests/{id}/qr"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(closed_qr.status(), StatusCode::GONE);
        assert_eq!(
            json_body(closed_qr).await["error"]["code"],
            "deposit_request_not_payable"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn idempotency_key_conflicts_on_every_immutable_issuance_field(pool: PgPool) {
        let app = app(pool.clone()).await;
        let account = test_account(&pool).await;
        let customer_id = customer(&pool, account, "Globex").await;
        let first_pdf = ready_attachment(&pool, account, "contract.pdf").await;
        let second_pdf = ready_attachment(&pool, account, "other.pdf").await;
        let pending_pdf = pending_attachment(&pool, account).await;
        let body = json!({
            "payout_address": "0x0000000000000000000000000000000000000002",
            "amount": "25",
            "expires_in": 3600,
            "issuer": {"name": "Acme Corp", "email": "billing@acme.example", "details": "1 Main St"},
            "payer": {"name": "Globex"},
            "notes": "Net 30",
            "heading": "March retainer",
            "reference": "INV-1",
            "metadata": {"po": "42"},
            "customer_id": customer_id,
            "payer_policy": {
                "mode": "verified_email",
                "expected_email": "Alice@Example.com"
            },
            "attachment_id": first_pdf
        });
        let created = app
            .clone()
            .oneshot(create_request(KEY, "document", &body))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = json_body(created).await;
        assert_eq!(created["customer_id"], customer_id.to_string());
        assert_eq!(created["attachment"]["id"], first_pdf.to_string());
        assert_eq!(created["attachment"]["filename"], "contract.pdf");
        assert_eq!(created["attachment"]["byte_length"], "1234");
        assert!(created["attachment"].get("download_url").is_none());
        assert_eq!(
            created["payer_policy"]["expected_email"],
            "alice@example.com"
        );
        assert_eq!(created["heading"], "March retainer");
        assert_eq!(created["notes"], "Net 30");

        // The document is attached now; an identical retry still replays.
        let replay = app
            .clone()
            .oneshot(create_request(KEY, "document", &body))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(replay.headers()["Idempotency-Replayed"], "true");
        assert_eq!(json_body(replay).await["id"], created["id"]);

        let mut variations: Vec<(&str, Value)> = Vec::new();
        let mut vary = |name: &'static str, edit: &dyn Fn(&mut Value)| {
            let mut variant = body.clone();
            edit(&mut variant);
            variations.push((name, variant));
        };
        vary("issuer", &|b| {
            b["issuer"]["email"] = json!("ap@acme.example")
        });
        vary("payer", &|b| b["payer"]["name"] = json!("Initech"));
        vary("notes", &|b| b["notes"] = json!("Net 60"));
        vary("heading", &|b| {
            b.as_object_mut().unwrap().remove("heading");
        });
        vary("reference", &|b| b["reference"] = json!("INV-2"));
        vary("metadata", &|b| b["metadata"] = json!({"po": "43"}));
        vary("customer_id", &|b| {
            b.as_object_mut().unwrap().remove("customer_id");
        });
        vary("policy mode", &|b| {
            b["payer_policy"] = json!({"mode": "permissionless"})
        });
        vary("expected_email", &|b| {
            b["payer_policy"]["expected_email"] = json!("bob@example.com")
        });
        vary("expiration intent", &|b| b["expires_in"] = json!(7200));
        vary("amount", &|b| b["amount"] = json!("26"));
        vary("attachment id", &|b| b["attachment_id"] = json!(second_pdf));
        vary("attachment removed", &|b| {
            b.as_object_mut().unwrap().remove("attachment_id");
        });
        vary("attachment not yet finalized", &|b| {
            b["attachment_id"] = json!(pending_pdf)
        });
        for (name, variant) in variations {
            let response = app
                .clone()
                .oneshot(create_request(KEY, "document", &variant))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT, "{name}");
            assert_eq!(
                json_body(response).await["error"]["code"],
                "idempotency_conflict",
                "{name}"
            );
        }
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM invoices")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "conflicts issue nothing");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn create_rejects_foreign_customers_and_unusable_attachments(pool: PgPool) {
        let app = app(pool.clone()).await;
        let account = test_account(&pool).await;
        let other = other_account(&pool, "payday_live_other0123456789abcdef0123456789abcdef").await;
        let foreign_customer = customer(&pool, other, "Theirs").await;
        let foreign_pdf = ready_attachment(&pool, other, "theirs.pdf").await;
        let pending_pdf = pending_attachment(&pool, account).await;
        let attached_pdf = ready_attachment(&pool, account, "attached.pdf").await;
        let mut first = valid_body();
        first["attachment_id"] = json!(attached_pdf);
        assert_eq!(
            app.clone()
                .oneshot(create_request(KEY, "attached", &first))
                .await
                .unwrap()
                .status(),
            StatusCode::CREATED
        );

        let cases: Vec<(&str, &str, Value, StatusCode, &str)> = vec![
            (
                "foreign customer",
                "customer_id",
                json!(foreign_customer),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                "foreign attachment",
                "attachment_id",
                json!(foreign_pdf),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                "unknown attachment",
                "attachment_id",
                json!(AttachmentId::generate()),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                "bare uuid where an att_ id belongs",
                "attachment_id",
                json!(Uuid::now_v7()),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                "pending attachment",
                "attachment_id",
                json!(pending_pdf),
                StatusCode::CONFLICT,
                "attachment_not_ready",
            ),
            (
                "attached attachment",
                "attachment_id",
                json!(attached_pdf),
                StatusCode::CONFLICT,
                "attachment_already_attached",
            ),
            (
                "blank issuer",
                "issuer",
                json!({"name": " "}),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                "long heading",
                "heading",
                json!("h".repeat(201)),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                "bad expected email",
                "payer_policy",
                json!({"mode": "verified_email", "expected_email": "not-an-email"}),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            // Legal JSON, but Postgres cannot store it: refused before the
            // insert rather than failing inside it.
            (
                "nul in notes",
                "notes",
                json!("Net\u{0}30"),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                "nul in issuer name",
                "issuer",
                json!({"name": "Ac\u{0}me"}),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
        ];
        for (name, field, value, status, code) in cases {
            let mut body = valid_body();
            body[field] = value;
            let response = app
                .clone()
                .oneshot(create_request(KEY, name, &body))
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{name}");
            assert_eq!(json_body(response).await["error"]["code"], code, "{name}");
        }

        // The mode rules are enforced by the wire shape itself.
        let mut body = valid_body();
        body["payer_policy"] = json!({
            "mode": "verified_email",
            "expected_email": "alice@example.com",
            "expected_identity": {"first_name": "Alice", "last_name": "Smith"}
        });
        let response = app
            .clone()
            .oneshot(create_request(KEY, "retired assertion", &body))
            .await
            .unwrap();
        assert!(response.status().is_client_error());
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM invoices")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1, "only the first issuance succeeded");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn gated_invoice_reveals_only_issuer_heading_and_requirements_until_verified(
        pool: PgPool,
    ) {
        let app = app(pool.clone()).await;
        let account = test_account(&pool).await;
        let pdf = ready_attachment(&pool, account, "contract.pdf").await;
        let mut body = valid_body();
        body["heading"] = json!("March retainer");
        body["notes"] = json!("Net 30");
        body["reference"] = json!("INV-1");
        body["attachment_id"] = json!(pdf);
        body["payer_policy"] =
            json!({"mode": "verified_email", "expected_email": "alice@example.com"});
        let created = app
            .clone()
            .oneshot(create_request(KEY, "gated", &body))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = json_body(created).await;
        let id = created["id"].as_str().unwrap();
        assert_eq!(
            created["payer_policy"]["expected_email"],
            "alice@example.com"
        );

        let payer = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/deposit-requests/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(payer.status(), StatusCode::OK);
        let payer = json_body(payer).await;
        assert_eq!(payer["content_unlocked"], false);
        assert_eq!(payer["issuer_name"], "Acme");
        assert_eq!(payer["heading"], "March retainer");
        assert_eq!(payer["status"], "awaiting_deposit");
        assert_eq!(payer["payer_policy"]["mode"], "verified_email");
        assert_eq!(
            payer["payer_policy"]["expected_email_hint"],
            "a****@e***.com"
        );
        assert_eq!(payer["requirements"]["email"], "pending");
        assert!(payer["requirements"].get("document").is_none());
        assert_eq!(payer["requirements"]["complete"], false);
        for withheld in [
            "amount",
            "amount_base_units",
            "received",
            "remaining",
            "address",
            "address_explorer_url",
            "deposit_uri",
            "chain",
            "token",
            "invoice",
            "settlement_tx_hash",
            "settlement_explorer_url",
        ] {
            assert!(payer[withheld].is_null(), "{withheld} must be withheld");
        }
        let serialized = payer.to_string();
        for secret in [
            "alice@example.com",
            "Globex",
            "Net 30",
            "INV-1",
            "contract.pdf",
            "0xabab",
        ] {
            assert!(
                !serialized.contains(secret),
                "payer response leaked {secret}"
            );
        }
        // Nothing to leak: no wallet has been bound, so no address exists.
        assert!(created["address"].is_null());

        let qr = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/deposit-requests/{id}/qr"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(qr.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(qr).await["error"]["code"],
            "verification_required"
        );
        // The PDF is content: it stays behind the same gate as the QR.
        let withheld = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/deposit-requests/{id}/attachment"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(withheld.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(withheld).await["error"]["code"],
            "verification_required"
        );
        let merchant_pdf = app
            .clone()
            .oneshot(get_request(
                KEY,
                &format!("/v1/deposit-requests/{id}/attachment"),
            ))
            .await
            .unwrap();
        assert_eq!(merchant_pdf.status(), StatusCode::OK);
        assert!(
            json_body(merchant_pdf).await["download_url"]
                .as_str()
                .is_some_and(|url| url.starts_with("memory://"))
        );

        // Invoice-level completion is not evidence that this browser verified;
        // the content and approved facts unlock per payer session. The settlement
        // transaction is content too: on chain it names the payment address,
        // the amount, and the payout address.
        let settlement = alloy_primitives::B256::repeat_byte(0x5e);
        sqlx::query(
            "UPDATE invoices SET verification_completed_at = now(), settlement_tx_hash = $2 WHERE id = $1",
        )
        .bind(Uuid::parse_str(id.strip_prefix("dr_").unwrap()).unwrap())
        .bind(settlement.as_slice())
        .execute(&pool)
        .await
        .unwrap();
        let completed = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/deposit-requests/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let completed = json_body(completed).await;
        assert_eq!(completed["requirements"]["email"], "pending");
        assert_eq!(completed["requirements"]["complete"], false);
        assert_eq!(completed["content_unlocked"], false);
        assert!(completed["settlement_tx_hash"].is_null());
        assert!(completed["settlement_explorer_url"].is_null());
        assert!(!completed.to_string().contains("5e5e5e5e"));

        // The merchant, and the payer of a permissionless invoice, still see it.
        let open = app
            .clone()
            .oneshot(create_request(KEY, "open", &valid_body()))
            .await
            .unwrap();
        let open_id = json_body(open).await["id"].as_str().unwrap().to_owned();
        sqlx::query("UPDATE invoices SET settlement_tx_hash = $2 WHERE id = $1")
            .bind(Uuid::parse_str(open_id.strip_prefix("dr_").unwrap()).unwrap())
            .bind(settlement.as_slice())
            .execute(&pool)
            .await
            .unwrap();
        let open_payer = app
            .clone()
            .oneshot(
                Request::get(format!("/v1/payer/deposit-requests/{open_id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            json_body(open_payer).await["settlement_tx_hash"],
            settlement.to_string()
        );

        let merchant = app
            .oneshot(
                Request::get(format!("/v1/deposit-requests/{id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let merchant = json_body(merchant).await;
        assert_eq!(merchant["attachment"]["filename"], "contract.pdf");
        assert!(merchant["verification_completed_at"].is_string());
        assert_eq!(merchant["settlement_tx_hash"], settlement.to_string());
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn create_rejects_unknown_refund_address(pool: PgPool) {
        let app = app(pool).await;
        let mut body = valid_body();
        body["refund_address"] = json!("0x0000000000000000000000000000000000000003");
        let response = app
            .clone()
            .oneshot(create_request(KEY, "refund", &body))
            .await
            .unwrap();
        assert!(response.status().is_client_error(), "{}", response.status());
        assert_eq!(
            json_body(response).await["error"]["code"],
            "invalid_request"
        );

        let listed = app
            .oneshot(
                Request::get("/v1/deposit-requests")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            json_body(listed).await["deposit_requests"]
                .as_array()
                .unwrap()
                .is_empty(),
            "no invoice may be created from a rejected request"
        );
    }

    /// The payer, not the merchant, picks the network: the create route
    /// refuses chain fields, and a challenge on the second network signs
    /// under that chain's domain and binds the request there.
    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn the_payer_chooses_the_network(pool: PgPool) {
        let app = app(pool.clone()).await;
        for (field, value) in [("chain_id", "1"), ("token_address", "0x0000000000000000000000000000000000000000")] {
            let mut body = valid_body();
            body[field] = json!(value);
            let refused = app
                .clone()
                .oneshot(create_request(KEY, "chain-field", &body))
                .await
                .unwrap();
            assert_eq!(refused.status(), StatusCode::BAD_REQUEST, "{field}");
            let error = json_body(refused).await;
            assert_eq!(error["error"]["code"], "invalid_request");
            assert!(
                error["error"]["message"].as_str().unwrap().contains(field),
                "{error}"
            );
        }

        let created = app
            .clone()
            .oneshot(create_request(KEY, "second-network", &valid_body()))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = json_body(created).await;
        let id = created["id"].as_str().unwrap().to_owned();
        let wallet = gateway_core::wallet_of(&PAYER_KEY);
        let challenge = app
            .clone()
            .oneshot(payer_post(
                &format!("/v1/payer/deposit-requests/{id}/wallet/challenge"),
                None,
                Some(&json!({"wallet": wallet.to_checksum(None), "chain_id": "2"})),
            ))
            .await
            .unwrap();
        assert_eq!(challenge.status(), StatusCode::OK);
        let challenge = json_body(challenge).await;
        assert_eq!(challenge["chain"]["id"], "2");
        assert_eq!(challenge["typed_data"]["domain"]["chainId"], 2);
        let session = challenge["payer_session"].as_str().unwrap().to_owned();
        let signature = sign_challenge(&challenge, &PAYER_KEY);
        let bound = app
            .clone()
            .oneshot(payer_post(
                &format!("/v1/payer/deposit-requests/{id}/wallet/attest"),
                Some(&session),
                Some(&json!({"wallet": wallet.to_checksum(None), "signature": signature})),
            ))
            .await
            .unwrap();
        assert_eq!(bound.status(), StatusCode::OK);
        let bound = json_body(bound).await;
        assert_eq!(bound["chain"]["id"], "2");
        assert_eq!(
            bound["token"]["address"],
            Address::repeat_byte(0x02).to_checksum(None)
        );
        assert!(bound["deposit_uri"].as_str().unwrap().contains("@2/transfer"));
        let merchant = json_body(
            app.clone()
                .oneshot(get_request(KEY, &format!("/v1/deposit-requests/{id}")))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(merchant["chain"]["id"], "2");
        assert_eq!(merchant["self_settlement"]["chain_id"], "2");

        // The same document bound on chain 1 lands at another address: the
        // chain is committed into it like the wallet.
        let other = json_body(
            app.clone()
                .oneshot(create_request(KEY, "first-network", &valid_body()))
                .await
                .unwrap(),
        )
        .await;
        let (on_one, _) = bind_wallet(&app, other["id"].as_str().unwrap(), None, &PAYER_KEY).await;
        assert_eq!(on_one["chain"]["id"], "1");
        assert_ne!(on_one["address"], bound["address"]);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn payer_wallet_binding_derives_the_address_and_is_final(pool: PgPool) {
        let app = app(pool.clone()).await;
        let created = app
            .clone()
            .oneshot(create_request(KEY, "bind", &valid_body()))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = json_body(created).await;
        let id = created["id"].as_str().unwrap().to_owned();
        let uuid = Uuid::parse_str(id.strip_prefix("dr_").unwrap()).unwrap();
        assert!(created["address"].is_null());
        assert!(created.get("refund_address").is_none());

        // Before the wallet step the payer sees the request but no address,
        // and nothing invites a transfer.
        let open = app
            .clone()
            .oneshot(payer_get(&format!("/v1/payer/deposit-requests/{id}"), None))
            .await
            .unwrap();
        let open = json_body(open).await;
        assert_eq!(open["content_unlocked"], true);
        assert_eq!(open["amount"], "1.000000");
        assert!(open["address"].is_null());
        assert!(open["deposit_uri"].is_null());
        assert!(open["payer_wallet"].is_null());
        assert!(open["chain"].is_null(), "no network until the payer chooses");
        assert_eq!(open["networks"].as_array().unwrap().len(), 2);
        assert_eq!(open["requirements"]["wallet"], "pending");
        assert_eq!(open["requirements"]["complete"], true);
        let qr = app
            .clone()
            .oneshot(payer_get(
                &format!("/v1/payer/deposit-requests/{id}/qr"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(qr.status(), StatusCode::CONFLICT);
        assert_eq!(json_body(qr).await["error"]["code"], "wallet_required");

        // The challenge is the EIP-712 document the wallet signs, minted on
        // a session the permissionless request did not have yet.
        let wallet = gateway_core::wallet_of(&PAYER_KEY);
        let challenge_path = format!("/v1/payer/deposit-requests/{id}/wallet/challenge");
        let attest_path = format!("/v1/payer/deposit-requests/{id}/wallet/attest");
        // The chain is mandatory and must be one the request offers.
        for (body, status, code) in [
            (
                json!({"wallet": wallet.to_checksum(None)}),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                json!({"wallet": wallet.to_checksum(None), "chain_id": "three"}),
                StatusCode::BAD_REQUEST,
                "invalid_request",
            ),
            (
                json!({"wallet": wallet.to_checksum(None), "chain_id": "3"}),
                StatusCode::UNPROCESSABLE_ENTITY,
                "unsupported_chain",
            ),
        ] {
            let refused = app
                .clone()
                .oneshot(payer_post(&challenge_path, None, Some(&body)))
                .await
                .unwrap();
            assert_eq!(refused.status(), status, "{body}");
            assert_eq!(json_body(refused).await["error"]["code"], code, "{body}");
        }
        let challenge = app
            .clone()
            .oneshot(payer_post(
                &challenge_path,
                None,
                Some(&json!({"wallet": wallet.to_checksum(None), "chain_id": "1"})),
            ))
            .await
            .unwrap();
        assert_eq!(challenge.status(), StatusCode::OK);
        let challenge = json_body(challenge).await;
        let session = challenge["payer_session"].as_str().unwrap().to_owned();
        assert_eq!(challenge["chain"]["id"], "1");
        assert_eq!(challenge["chain"]["name"], "Ethereum");
        assert_eq!(challenge["typed_data"]["primaryType"], "PayerAttestation");
        assert_eq!(challenge["typed_data"]["domain"]["name"], "Payday");
        assert_eq!(challenge["typed_data"]["domain"]["chainId"], 1);
        assert_eq!(
            challenge["typed_data"]["domain"]["verifyingContract"],
            Address::ZERO.to_checksum(None)
        );
        assert_eq!(
            challenge["typed_data"]["message"]["wallet"],
            wallet.to_checksum(None)
        );
        assert_eq!(
            challenge["typed_data"]["message"]["attributionHash"],
            created["attribution"]["hash"]
        );
        assert!(
            challenge["typed_data"]["message"]["statement"]
                .as_str()
                .unwrap()
                .contains("Only transfers from this wallet")
        );

        // Another key's signature over the same document binds nothing.
        let forged = sign_challenge(&challenge, &[9u8; 32]);
        let refused = app
            .clone()
            .oneshot(payer_post(
                &attest_path,
                Some(&session),
                Some(&json!({"wallet": wallet.to_checksum(None), "signature": forged})),
            ))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(refused).await["error"]["code"],
            "wallet_signature_invalid"
        );
        let unbound = InvoiceRepository::new(pool.clone())
            .find_by_id(uuid)
            .await
            .unwrap()
            .unwrap();
        assert!(unbound.payment_address.is_none());

        // The wallet's own signature binds it: the address appears, the
        // wallet is what it commits to, and only transfers from it count.
        let signature = sign_challenge(&challenge, &PAYER_KEY);
        let bound = app
            .clone()
            .oneshot(payer_post(
                &attest_path,
                Some(&session),
                Some(&json!({"wallet": wallet.to_checksum(None), "signature": signature})),
            ))
            .await
            .unwrap();
        assert_eq!(bound.status(), StatusCode::OK);
        let bound = json_body(bound).await;
        let address = bound["address"].as_str().unwrap().to_owned();
        assert!(address.starts_with("0x"));
        assert_eq!(bound["payer_wallet"], wallet.to_checksum(None));
        assert_eq!(bound["requirements"]["wallet"], "approved");
        // The chosen network is now the request's network.
        assert_eq!(bound["chain"]["id"], "1");
        assert_eq!(bound["token"]["address"], Address::ZERO.to_checksum(None));
        assert!(bound["deposit_uri"].as_str().unwrap().contains("@1/transfer"));
        assert!(
            bound["deposit_uri"]
                .as_str()
                .unwrap()
                .contains(&format!("address={address}"))
        );
        for private in ["recovery_address", "self_settlement", "payout_address"] {
            assert!(bound.get(private).is_none(), "{private}");
        }

        // The merchant sees the same address, the wallet as the recovery
        // term, and the material to settle it themselves; the row's binding
        // recomputes to the address it stored.
        let merchant = app
            .clone()
            .oneshot(get_request(KEY, &format!("/v1/deposit-requests/{id}")))
            .await
            .unwrap();
        let merchant = json_body(merchant).await;
        assert_eq!(merchant["address"], address);
        assert_eq!(merchant["payer_wallet"], wallet.to_checksum(None));
        assert_eq!(merchant["recovery_address"], wallet.to_checksum(None));
        assert!(merchant["wallet_bound_at"].is_string());
        assert!(merchant["self_settlement"]["salt"].is_string());
        assert_eq!(merchant["self_settlement"]["chain_id"], "1");
        assert_eq!(merchant["chain"]["id"], "1");
        let row = InvoiceRepository::new(pool.clone())
            .find_by_id(uuid)
            .await
            .unwrap()
            .unwrap();
        let invoice = Invoice::try_from(&row).unwrap();
        let binding = invoice.binding.as_ref().unwrap();
        assert_eq!(binding.payer_wallet, wallet);
        assert_eq!(binding.recovery.0, wallet);
        assert_eq!(binding.network.chain_id, ChainId(1));
        assert_eq!(binding.payment_address.0.to_checksum(None), address);
        assert!(invoice.address_matches_parameters());
        assert_eq!(
            binding.salt,
            gateway_core::recompute_salt(
                invoice.attribution_hash,
                binding.attestation.digest.parse().unwrap()
            )
        );
        let events: Vec<String> =
            sqlx::query_scalar("SELECT event_type FROM webhook_events WHERE invoice_id = $1")
                .bind(uuid)
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(events, ["deposit_request.ready"]);

        // The challenge was consumed by the binding; the request takes no
        // second wallet, and the merchant's verification view lists the
        // wallet attempt.
        let spent = app
            .clone()
            .oneshot(payer_post(
                &attest_path,
                Some(&session),
                Some(&json!({"wallet": wallet.to_checksum(None), "signature": signature})),
            ))
            .await
            .unwrap();
        assert_eq!(spent.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(spent).await["error"]["code"],
            "wallet_challenge_required"
        );
        let other = gateway_core::wallet_of(&[9u8; 32]);
        let taken = app
            .clone()
            .oneshot(payer_post(
                &challenge_path,
                None,
                Some(&json!({"wallet": other.to_checksum(None), "chain_id": "2"})),
            ))
            .await
            .unwrap();
        assert_eq!(taken.status(), StatusCode::CONFLICT);
        let taken = json_body(taken).await;
        assert_eq!(taken["error"]["code"], "wallet_already_bound");
        assert!(
            taken["error"]["message"]
                .as_str()
                .unwrap()
                .contains(&wallet.to_checksum(None))
        );
        let verification = app
            .clone()
            .oneshot(get_request(
                KEY,
                &format!("/v1/deposit-requests/{id}/verification"),
            ))
            .await
            .unwrap();
        let verification = json_body(verification).await;
        assert_eq!(verification["facts"]["wallet"], "approved");
        assert_eq!(verification["facts"]["email"], "not_required");
        assert_eq!(verification["attempts"][0]["kind"], "wallet");
        assert_eq!(verification["attempts"][0]["status"], "approved");

        // A gated request takes the wallet step only from a session that
        // has satisfied its policy.
        let (gated_id, _) = create_gated(
            &app,
            "bind-gated",
            json!({"mode": "verified_email", "expected_email": "alice@example.com"}),
            "Gated",
        )
        .await;
        let no_session = app
            .clone()
            .oneshot(payer_post(
                &format!("/v1/payer/deposit-requests/{gated_id}/wallet/challenge"),
                None,
                Some(&json!({"wallet": wallet.to_checksum(None), "chain_id": "1"})),
            ))
            .await
            .unwrap();
        assert_eq!(no_session.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(no_session).await["error"]["code"],
            "payer_session_invalid"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn create_invoice_rejects_malformed_parameters(pool: PgPool) {
        let app = app(pool).await;
        let cases: Vec<(&str, Value, &str)> = vec![
            ("negative amount", json!("-1.5"), "invalid_amount"),
            ("signed amount", json!("+1"), "invalid_amount"),
            ("zero amount", json!("0"), "invalid_amount"),
            ("too many decimals", json!("1.0000001"), "invalid_amount"),
        ];
        for (name, amount, code) in cases {
            let mut body = valid_body();
            body["amount"] = amount;
            let response = app
                .clone()
                .oneshot(create_request(KEY, name, &body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{name}");
            assert_eq!(json_body(response).await["error"]["code"], code, "{name}");
        }

        let expiration_cases = [
            ("deadline too soon", 60),
            ("beyond a year", 400 * 24 * 3600),
        ];
        for (name, expiration) in expiration_cases {
            let mut body = valid_body();
            body["expires_in"] = json!(expiration);
            let response = app
                .clone()
                .oneshot(create_request(KEY, name, &body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{name}");
            assert_eq!(
                json_body(response).await["error"]["code"],
                "invalid_request",
                "{name}"
            );
        }

        let mut body = valid_body();
        body["payout_address"] = json!("0x0000000000000000000000000000000000000000");
        let response = app
            .clone()
            .oneshot(create_request(KEY, "zero beneficiary", &body))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(
            json_body(response).await["error"]["message"]
                .as_str()
                .unwrap()
                .contains("payout_address")
        );

        for (name, key) in [("empty key", String::new()), ("long key", "k".repeat(256))] {
            let response = app
                .clone()
                .oneshot(create_request(KEY, &key, &valid_body()))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{name}");
            assert_eq!(
                json_body(response).await["error"]["code"],
                "invalid_request",
                "{name}"
            );
        }

        let response = app
            .oneshot(create_request(KEY, "k".repeat(255).as_str(), &valid_body()))
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            StatusCode::CREATED,
            "255-byte keys are allowed"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn invoices_and_idempotency_keys_are_isolated_by_account(pool: PgPool) {
        const FIRST: &str = "payday_live_first0123456789abcdef0123456789abcdef";
        const SECOND: &str = "payday_live_second0123456789abcdef0123456789abcdef";
        let accounts = AccountRepository::new(pool.clone());
        accounts
            .issue_api_key_with_email(
                "https://issuer.example/",
                "email|first",
                None,
                "first-event",
                FIRST,
                "first@example.com",
            )
            .await
            .unwrap();
        accounts
            .issue_api_key_with_email(
                "https://issuer.example/",
                "email|second",
                None,
                "second-event",
                SECOND,
                "second@example.com",
            )
            .await
            .unwrap();
        let app = app(pool).await;
        let body = valid_body();

        let first = app
            .clone()
            .oneshot(create_request(FIRST, "same-key", &body))
            .await
            .unwrap();
        let second = app
            .clone()
            .oneshot(create_request(SECOND, "same-key", &body))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::CREATED);
        assert_eq!(second.status(), StatusCode::CREATED);
        let first_id = json_body(first).await["id"].as_str().unwrap().to_string();
        let second_id = json_body(second).await["id"].as_str().unwrap().to_string();
        assert_ne!(first_id, second_id);

        let cross_account = app
            .oneshot(
                Request::get(format!("/v1/deposit-requests/{first_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {SECOND}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(cross_account.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_dashboard_session_can_issue_inspect_rotate_and_revoke_keys(pool: PgPool) {
        let (privy, session) = merchant_session();
        let app = build(pool, Some(privy.verifier), Address::ZERO)
            .await
            .router;
        let with_session = |method: &str, path: &str, body: &str| {
            Request::builder()
                .method(method)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {session}"))
                .header(header::CONTENT_TYPE, "application/json")
                .body(Body::from(body.to_owned()))
                .unwrap()
        };

        // Signing in provisions the account keyless, at generation 1, with
        // the mailbox and the wallet the session carried.
        let account = app
            .clone()
            .oneshot(with_session("GET", "/v1/account", ""))
            .await
            .unwrap();
        assert_eq!(account.status(), StatusCode::OK);
        let account = json_body(account).await;
        assert_eq!(account["generation"], 1);
        assert!(account["key_hint"].is_null());
        assert_eq!(account["email"], "merchant@example.com");
        assert_eq!(account["wallet_address"], MERCHANT_WALLET);
        let account_id = account["account_id"].as_str().unwrap().to_owned();

        // A first key: issued against the generation the account reports.
        let provisioned = app
            .clone()
            .oneshot(with_session(
                "POST",
                "/v1/account/api-key",
                r#"{"expected_generation":1}"#,
            ))
            .await
            .unwrap();
        assert_eq!(provisioned.status(), StatusCode::CREATED);
        let provisioned_json = json_body(provisioned).await;
        assert_eq!(provisioned_json["generation"], 2);
        assert_eq!(provisioned_json["replaced_previous_key"], false);
        let first_key = provisioned_json["api_key"].as_str().unwrap().to_string();

        // Replaying the same intent against the generation it already moved
        // past is a conflict, not a second key.
        let duplicate = app
            .clone()
            .oneshot(with_session(
                "POST",
                "/v1/account/api-key",
                r#"{"expected_generation":1}"#,
            ))
            .await
            .unwrap();
        assert_eq!(duplicate.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(duplicate).await["error"]["code"],
            "api_key_generation_conflict"
        );

        // The same session rolls the key; no second sign-in is asked for.
        let replaced = app
            .clone()
            .oneshot(with_session(
                "POST",
                "/v1/account/api-key",
                r#"{"expected_generation":2}"#,
            ))
            .await
            .unwrap();
        assert_eq!(replaced.status(), StatusCode::OK);
        let replaced_json = json_body(replaced).await;
        assert_eq!(replaced_json["generation"], 3);
        assert_eq!(replaced_json["replaced_previous_key"], true);
        let second_key = replaced_json["api_key"].as_str().unwrap().to_string();
        assert_ne!(first_key, second_key);

        // The key reads the same account, wallet and all — but cannot manage
        // itself: only a session may mint or revoke.
        let metadata = app
            .clone()
            .oneshot(get_request(&second_key, "/v1/account"))
            .await
            .unwrap();
        assert_eq!(metadata.status(), StatusCode::OK);
        let metadata = json_body(metadata).await;
        assert_eq!(metadata["account_id"], account_id);
        assert_eq!(metadata["generation"], 3);
        assert_eq!(metadata["wallet_address"], MERCHANT_WALLET);
        assert!(metadata["key_hint"].is_string());
        assert!(metadata["previous_key_expires_at"].is_string());
        assert!(metadata["revoked_at"].is_null());
        for (method, body) in [
            ("POST", r#"{"expected_generation":3}"#),
            ("DELETE", r#"{"expected_generation":3}"#),
        ] {
            let refused = app
                .clone()
                .oneshot(json_request(
                    method,
                    &second_key,
                    "/v1/account/api-key",
                    &serde_json::from_str::<Value>(body).unwrap(),
                ))
                .await
                .unwrap();
            assert_eq!(refused.status(), StatusCode::UNAUTHORIZED, "{method}");
            assert_eq!(
                json_body(refused).await["error"]["code"],
                "identity_unauthorized",
                "{method}"
            );
        }
        // The key route has no read of its own: the account is the resource.
        let key_metadata = app
            .clone()
            .oneshot(with_session("GET", "/v1/account", ""))
            .await
            .unwrap();
        assert_eq!(key_metadata.status(), StatusCode::OK);
        assert_eq!(json_body(key_metadata).await["generation"], 3);
        let unknown_field = app
            .clone()
            .oneshot(with_session(
                "POST",
                "/v1/account/api-key",
                r#"{"expected_generation":3,"rotate":true}"#,
            ))
            .await
            .unwrap();
        assert_eq!(unknown_field.status(), StatusCode::BAD_REQUEST);

        let invoice_request = |key: &str| {
            Request::get("/v1/deposit-requests/not-an-id")
                .header(header::AUTHORIZATION, format!("Bearer {key}"))
                .body(Body::empty())
                .unwrap()
        };
        assert_eq!(
            app.clone()
                .oneshot(invoice_request(&first_key))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST,
            "the replaced key keeps working through its grace window"
        );
        assert_eq!(
            app.clone()
                .oneshot(invoice_request(&second_key))
                .await
                .unwrap()
                .status(),
            StatusCode::BAD_REQUEST
        );

        let revoked = app
            .clone()
            .oneshot(with_session(
                "DELETE",
                "/v1/account/api-key",
                r#"{"expected_generation":3}"#,
            ))
            .await
            .unwrap();
        assert_eq!(revoked.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            app.oneshot(invoice_request(&second_key))
                .await
                .unwrap()
                .status(),
            StatusCode::UNAUTHORIZED
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn privy_sessions_authenticate_without_a_key_and_carry_the_wallet(pool: PgPool) {
        let privy = PrivyApp::new();
        // The very first session, before Privy has finished making the wallet.
        let early = privy.token(MERCHANT_DID, "Dashboard@Example.com", None);
        let app = build(pool.clone(), Some(privy.verifier.clone()), Address::ZERO)
            .await
            .router;

        let created = app
            .clone()
            .oneshot(create_request(&early, "dashboard-session", &valid_body()))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let id = json_body(created).await["id"].as_str().unwrap().to_owned();

        let accounts = AccountRepository::new(pool.clone());
        let account = accounts
            .find_by_identity(auth::PRIVY_ISSUER, MERCHANT_DID)
            .await
            .unwrap()
            .expect("the first session provisions the account");
        let metadata = accounts.metadata(account).await.unwrap();
        assert!(metadata.hint.is_none(), "no key was issued");
        assert_eq!(metadata.email.as_deref(), Some("dashboard@example.com"));
        assert!(metadata.wallet_address.is_none());

        // The next session names the wallet, so the account learns it, and
        // it is the same account: the payment issued a moment ago is there.
        let later = privy.token(MERCHANT_DID, "dashboard@example.com", Some(MERCHANT_WALLET));
        let listed = app
            .clone()
            .oneshot(get_request(&later, "/v1/deposit-requests"))
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        assert_eq!(json_body(listed).await["deposit_requests"][0]["id"], id);
        assert_eq!(
            accounts
                .metadata(account)
                .await
                .unwrap()
                .wallet_address
                .as_deref(),
            Some(MERCHANT_WALLET)
        );

        // Another Privy user is another account, with none of this one's data.
        let stranger = privy.token("did:privy:someoneelse", "other@example.com", None);
        let listed = app
            .clone()
            .oneshot(get_request(&stranger, "/v1/deposit-requests"))
            .await
            .unwrap();
        assert_eq!(listed.status(), StatusCode::OK);
        assert!(
            json_body(listed).await["deposit_requests"]
                .as_array()
                .unwrap()
                .is_empty()
        );

        // Tokens for another app, from another issuer, or signed by someone
        // else are invalid credentials, exactly like a wrong key.
        let accounts_json = Some(
            serde_json::json!([{"type": "email", "address": "dashboard@example.com"}]).to_string(),
        );
        let other = PrivyApp::new();
        for bad in [
            privy.token_with(
                MERCHANT_DID,
                auth::PRIVY_ISSUER,
                "other-app",
                u64::MAX,
                accounts_json.clone(),
            ),
            privy.token_with(
                MERCHANT_DID,
                "https://issuer.example/",
                auth::testing::APP_ID,
                u64::MAX,
                accounts_json.clone(),
            ),
            other.token(MERCHANT_DID, "dashboard@example.com", None),
        ] {
            assert_unauthorized(app.clone(), get_request(&bad, "/v1/deposit-requests")).await;
        }

        // A deployment without a Privy app refuses every session outright.
        let keys_only = build(pool, None, Address::ZERO).await.router;
        assert_unauthorized(keys_only, get_request(&later, "/v1/deposit-requests")).await;
    }

    /// Reserve an upload slot; returns the attachment id and its object key.
    async fn reserve_upload(
        app: &Router,
        key: &str,
        filename: &str,
    ) -> (AttachmentId, String, Value) {
        let response = app
            .clone()
            .oneshot(json_request(
                "POST",
                key,
                "/v1/attachments",
                &json!({"filename": filename}),
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CREATED);
        let body = json_body(response).await;
        let id = AttachmentId::parse(body["id"].as_str().unwrap()).expect("an att_ id");
        let url = body["upload_url"].as_str().unwrap();
        let object_key = url
            .strip_prefix("memory://")
            .and_then(|rest| rest.split('?').next())
            .unwrap()
            .to_owned();
        (id, object_key, body)
    }

    async fn finalize(app: &Router, key: &str, id: AttachmentId) -> (StatusCode, Value) {
        let response = app
            .clone()
            .oneshot(
                Request::post(format!("/v1/attachments/{id}/finalize"))
                    .header(header::AUTHORIZATION, format!("Bearer {key}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        (status, json_body(response).await)
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn attachment_upload_finalize_issuance_and_download_follow_the_scan_verdict(
        pool: PgPool,
    ) {
        let TestApp {
            router: app,
            storage,
        } = test_app(pool.clone()).await;
        let account = test_account(&pool).await;

        let (id, object_key, reserved) = reserve_upload(&app, KEY, "contract.pdf").await;
        // Object keys carry the raw UUIDs behind the acct_ and att_ ids.
        assert_eq!(
            object_key,
            format!("uploads/{}/{}.pdf", account.0, id.0),
            "the key is reserved under the account"
        );
        assert_eq!(reserved["headers"]["content-type"], "application/pdf");
        assert_eq!(
            reserved["headers"]["x-amz-tagging"],
            "payday-upload=pending"
        );
        assert_eq!(
            reserved["headers"]["if-none-match"], "*",
            "the key is write-once"
        );
        assert!(reserved["expires_at"].is_string());

        // Nothing uploaded yet.
        let (status, body) = finalize(&app, KEY, id).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "attachment_not_ready");

        // Uploaded, but the scanner has not spoken.
        let version = storage.put(&object_key, "application/pdf", PDF);
        let (status, body) = finalize(&app, KEY, id).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "attachment_scan_pending");

        storage.tag(&object_key, SCAN_STATUS_TAG, CLEAN_SCAN);
        let (status, ready) = finalize(&app, KEY, id).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(ready["id"], id.to_string());
        assert_eq!(ready["filename"], "contract.pdf");
        assert_eq!(ready["mime_type"], "application/pdf");
        assert_eq!(ready["byte_length"], PDF.len().to_string());
        assert_eq!(ready["sha256"], sha256_hex(PDF));
        assert!(ready.get("download_url").is_none());
        let (status, again) = finalize(&app, KEY, id).await;
        assert_eq!(status, StatusCode::OK, "finalize is idempotent");
        assert_eq!(again, ready);
        let pinned: Option<String> =
            sqlx::query_scalar("SELECT version_id FROM invoice_attachments WHERE id = $1")
                .bind(id.0)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(pinned.as_deref(), Some(version.as_str()));

        // A second PUT that somehow lands is never what the invoice serves:
        // the retag and the download link both name the hashed version.
        let replaced = storage.put(&object_key, "application/pdf", b"%PDF-1.4 replaced");
        assert_ne!(replaced, version);

        let mut body = valid_body();
        body["attachment_id"] = json!(id);
        let created = app
            .clone()
            .oneshot(create_request(KEY, "with-pdf", &body))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = json_body(created).await;
        let payment_id = created["id"].as_str().unwrap().to_owned();
        assert_eq!(created["attachment"]["sha256"], sha256_hex(PDF));
        let tags = storage.tags_of_version(&object_key, Some(&version));
        assert_eq!(
            tags["payday-upload"], "attached",
            "issuance retags the hashed version"
        );
        assert_eq!(tags[SCAN_STATUS_TAG], CLEAN_SCAN, "the verdict is kept");
        assert_eq!(
            storage.tags_of_version(&object_key, Some(&replaced))["payday-upload"],
            "pending",
            "the replacement is left to the lifecycle rule"
        );

        let reuse = app
            .clone()
            .oneshot(create_request(KEY, "reuse-pdf", &body))
            .await
            .unwrap();
        assert_eq!(reuse.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(reuse).await["error"]["code"],
            "attachment_already_attached"
        );
        let (status, _) = finalize(&app, KEY, id).await;
        assert_eq!(status, StatusCode::OK, "an attached upload still answers");

        let merchant = app
            .clone()
            .oneshot(get_request(
                KEY,
                &format!("/v1/deposit-requests/{payment_id}/attachment"),
            ))
            .await
            .unwrap();
        assert_eq!(merchant.status(), StatusCode::OK);
        assert_eq!(merchant.headers()[header::CACHE_CONTROL], "no-store");
        let merchant = json_body(merchant).await;
        assert_eq!(merchant["sha256"], sha256_hex(PDF));
        let download_url = merchant["download_url"].as_str().unwrap();
        assert!(download_url.starts_with(&format!("memory://{object_key}?download")));
        assert!(
            download_url.ends_with(&format!("&versionId={version}")),
            "the download serves the hashed version: {download_url}"
        );

        // A permissionless invoice's PDF is open to its payer, from any origin.
        let payer = app
            .clone()
            .oneshot(
                Request::get(format!(
                    "/v1/payer/deposit-requests/{payment_id}/attachment"
                ))
                .header(header::ORIGIN, "https://payday.sh")
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(payer.status(), StatusCode::OK);
        assert_eq!(payer.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], "*");
        assert_eq!(payer.headers()[header::CACHE_CONTROL], "no-store");
        let payer = json_body(payer).await;
        assert_eq!(payer["filename"], "contract.pdf");
        assert!(
            payer["download_url"]
                .as_str()
                .unwrap()
                .starts_with("memory://")
        );

        // An invoice without a document has nothing to serve.
        let plain = app
            .clone()
            .oneshot(create_request(KEY, "no-pdf", &valid_body()))
            .await
            .unwrap();
        let plain_id = json_body(plain).await["id"].as_str().unwrap().to_owned();
        for path in [
            format!("/v1/deposit-requests/{plain_id}/attachment"),
            format!("/v1/payer/deposit-requests/{plain_id}/attachment"),
        ] {
            let response = app.clone().oneshot(get_request(KEY, &path)).await.unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
            assert_eq!(
                json_body(response).await["error"]["code"],
                "attachment_not_found",
                "{path}"
            );
        }
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn finalize_rejects_non_pdf_oversized_dirty_and_foreign_uploads(pool: PgPool) {
        let TestApp {
            router: app,
            storage,
        } = test_app(pool.clone()).await;
        let oversized = vec![b'%'; 5 * 1024 * 1024 + 1];
        let cases: Vec<(&str, &str, &[u8], &str)> = vec![
            ("wrong content type", "text/plain", PDF, CLEAN_SCAN),
            ("empty", "application/pdf", b"", CLEAN_SCAN),
            ("oversized", "application/pdf", &oversized, CLEAN_SCAN),
            ("dirty", "application/pdf", PDF, "THREATS_FOUND"),
            (
                "not a pdf",
                "application/pdf",
                b"hello, not a pdf",
                CLEAN_SCAN,
            ),
        ];
        for (name, content_type, bytes, verdict) in cases {
            let (id, object_key, _) = reserve_upload(&app, KEY, "upload.pdf").await;
            storage.put(&object_key, content_type, bytes);
            storage.tag(&object_key, SCAN_STATUS_TAG, verdict);
            let (status, body) = finalize(&app, KEY, id).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{name}");
            assert_eq!(body["error"]["code"], "attachment_rejected", "{name}");
            assert!(
                !storage.exists(&object_key),
                "{name}: a rejected upload's object is deleted"
            );
            // The decision is final: a retry does not re-inspect the object.
            storage.put(&object_key, "application/pdf", PDF);
            let (status, body) = finalize(&app, KEY, id).await;
            assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{name} retry");
            assert_eq!(body["error"]["code"], "attachment_rejected", "{name} retry");
            let mut issue = valid_body();
            issue["attachment_id"] = json!(id);
            let issued = app
                .clone()
                .oneshot(create_request(KEY, name, &issue))
                .await
                .unwrap();
            assert_eq!(issued.status(), StatusCode::CONFLICT, "{name}");
            assert_eq!(
                json_body(issued).await["error"]["code"],
                "attachment_not_ready",
                "{name}"
            );
        }
        let rejected: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM invoice_attachments WHERE status = 'rejected' AND scan_result IS NOT NULL",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(rejected, 5);

        // Another account sees none of it.
        const OTHER: &str = "payday_live_other0123456789abcdef0123456789abcdef";
        other_account(&pool, OTHER).await;
        let (id, object_key, _) = reserve_upload(&app, KEY, "mine.pdf").await;
        storage.put(&object_key, "application/pdf", PDF);
        storage.tag(&object_key, SCAN_STATUS_TAG, CLEAN_SCAN);
        let (status, body) = finalize(&app, OTHER, id).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["error"]["code"], "attachment_not_found");
        // Unknown, bare (unprefixed), and malformed ids all read as missing.
        for missing in [
            AttachmentId::generate().to_string(),
            Uuid::now_v7().to_string(),
            "not-a-uuid".into(),
        ] {
            let response = app
                .clone()
                .oneshot(
                    Request::post(format!("/v1/attachments/{missing}/finalize"))
                        .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        let (status, _) = finalize(&app, KEY, id).await;
        assert_eq!(status, StatusCode::OK);
        let mut issue = valid_body();
        issue["attachment_id"] = json!(id);
        let created = app
            .clone()
            .oneshot(create_request(KEY, "mine", &issue))
            .await
            .unwrap();
        let payment_id = json_body(created).await["id"].as_str().unwrap().to_owned();
        let foreign = app
            .clone()
            .oneshot(get_request(
                OTHER,
                &format!("/v1/deposit-requests/{payment_id}/attachment"),
            ))
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_body(foreign).await["error"]["code"],
            "deposit_request_not_found"
        );

        for (name, body) in [
            ("blank", json!({"filename": "  "})),
            ("long", json!({"filename": "x".repeat(256)})),
            ("unknown field", json!({"filename": "a.pdf", "size": 1})),
        ] {
            let response = app
                .clone()
                .oneshot(json_request("POST", KEY, "/v1/attachments", &body))
                .await
                .unwrap();
            assert!(response.status().is_client_error(), "{name}");
            assert_eq!(
                json_body(response).await["error"]["code"],
                "invalid_request",
                "{name}"
            );
        }
    }

    /// A finalized upload, exactly as a client and the scanner leave it in
    /// the bucket, then admitted by the finalize route.
    async fn finalized_upload(
        app: &Router,
        storage: &MemoryObjectStorage,
    ) -> (AttachmentId, String) {
        let (id, object_key, _) = reserve_upload(app, KEY, "contract.pdf").await;
        storage.put(&object_key, "application/pdf", PDF);
        storage.tag(&object_key, SCAN_STATUS_TAG, CLEAN_SCAN);
        let (status, _) = finalize(app, KEY, id).await;
        assert_eq!(status, StatusCode::OK);
        (id, object_key)
    }

    async fn attachment_row(pool: &PgPool, id: AttachmentId) -> (String, Option<String>) {
        sqlx::query_as("SELECT status, scan_result FROM invoice_attachments WHERE id = $1")
            .bind(id.0)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn an_upload_the_bucket_expired_cannot_be_issued_and_says_so(pool: PgPool) {
        let TestApp {
            router: app,
            storage,
        } = test_app(pool.clone()).await;
        const EXPIRED: &str = "The upload expired before it was attached; upload the PDF again";

        // Issuance discovers the expiry while retagging.
        let (id, object_key) = finalized_upload(&app, &storage).await;
        storage.expire(&object_key);
        let mut body = valid_body();
        body["attachment_id"] = json!(id);
        for attempt in ["expired", "expired again"] {
            let response = app
                .clone()
                .oneshot(create_request(KEY, attempt, &body))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::CONFLICT, "{attempt}");
            let error = json_body(response).await["error"].clone();
            assert_eq!(error["code"], "attachment_not_ready", "{attempt}");
            assert_eq!(error["message"], EXPIRED, "{attempt}");
        }
        assert_eq!(
            attachment_row(&pool, id).await,
            ("rejected".into(), Some("expired".into()))
        );
        let (status, body) = finalize(&app, KEY, id).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["message"], EXPIRED);

        // Finalize discovers it too, so a client that re-checks before
        // issuing learns to upload again instead of being told 500 later.
        let (id, object_key) = finalized_upload(&app, &storage).await;
        storage.expire(&object_key);
        let (status, body) = finalize(&app, KEY, id).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"]["code"], "attachment_not_ready");
        assert_eq!(body["error"]["message"], EXPIRED);
        assert_eq!(
            attachment_row(&pool, id).await,
            ("rejected".into(), Some("expired".into()))
        );
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM invoices")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0);
    }

    /// The retag precedes the issuance transaction. When that transaction
    /// then loses an idempotency race, the object must go back to the
    /// lifecycle rule instead of sitting in the bucket forever, tagged
    /// attached and bound to nothing.
    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_lost_idempotency_race_after_the_retag_restores_the_pending_tag(pool: PgPool) {
        let TestApp {
            router: app,
            storage,
        } = test_app(pool.clone()).await;
        let (id, object_key) = finalized_upload(&app, &storage).await;

        // An invoice issued under another key is the template for the racer.
        let seed = app
            .clone()
            .oneshot(create_request(KEY, "seed", &valid_body()))
            .await
            .unwrap();
        assert_eq!(seed.status(), StatusCode::CREATED);
        let seed_id = json_body(seed).await["id"].as_str().unwrap().to_owned();
        let seed_uuid = Uuid::parse_str(seed_id.strip_prefix("dr_").unwrap()).unwrap();

        // The racer: a row under the key "raced", inserted but not yet
        // committed. The handler's pre-insert lookup cannot see it, so the
        // request passes the ready check, retags the object, and then blocks
        // inside the insert on the uncommitted unique key.
        let mut racer = pool.begin().await.unwrap();
        sqlx::query(
            "CREATE TEMP TABLE racer ON COMMIT DROP AS SELECT * FROM invoices WHERE id = $1",
        )
        .bind(seed_uuid)
        .execute(&mut *racer)
        .await
        .unwrap();
        sqlx::query("UPDATE racer SET id = $1, idempotency_key = 'raced'")
            .bind(Uuid::now_v7())
            .execute(&mut *racer)
            .await
            .unwrap();
        sqlx::query("INSERT INTO invoices SELECT * FROM racer")
            .execute(&mut *racer)
            .await
            .unwrap();

        let mut body = valid_body();
        body["attachment_id"] = json!(id);
        let request = tokio::spawn(app.clone().oneshot(create_request(KEY, "raced", &body)));
        // Wait until the request is blocked on the racer's lock, so the
        // conflict is decided inside the transaction, after the retag.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let waiting: i64 = sqlx::query_scalar(
                "SELECT count(*) FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock'",
            )
            .fetch_one(&pool)
            .await
            .unwrap();
            if waiting > 0 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "the request never reached the insert"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert_eq!(
            storage.tags_of(&object_key)["payday-upload"],
            "attached",
            "the retag happened before the insert"
        );
        racer.commit().await.unwrap();

        let response = request.await.unwrap().unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(response).await["error"]["code"],
            "idempotency_conflict"
        );
        assert_eq!(
            storage.tags_of(&object_key)["payday-upload"],
            "pending",
            "the object is handed back to the lifecycle rule"
        );
        assert_eq!(storage.tags_of(&object_key)[SCAN_STATUS_TAG], CLEAN_SCAN);
        assert_eq!(attachment_row(&pool, id).await.0, "ready");
        let bound: Option<Uuid> =
            sqlx::query_scalar("SELECT invoice_id FROM invoice_attachments WHERE id = $1")
                .bind(id.0)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert!(bound.is_none());
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_replay_against_another_factory_conflicts(pool: PgPool) {
        let app = app(pool.clone()).await;
        let created = app
            .clone()
            .oneshot(create_request(KEY, "factory", &valid_body()))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);

        // The factory is committed into the payment address, so the same
        // request against a redeployed factory is not the same issuance.
        let redeployed = app_with_factory(pool.clone(), Address::repeat_byte(0x0f)).await;
        let response = redeployed
            .oneshot(create_request(KEY, "factory", &valid_body()))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(response).await["error"]["code"],
            "idempotency_conflict"
        );
        let replay = app
            .oneshot(create_request(KEY, "factory", &valid_body()))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(replay.headers()["Idempotency-Replayed"], "true");
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn merchant_routes_answer_cors_for_the_dashboard_origin_only(pool: PgPool) {
        let app = app(pool.clone()).await;
        let origin = payer_access().origin().to_owned();
        assert_eq!(origin, "http://127.0.0.1:3000");
        let preflight = |origin: &str| {
            Request::options("/v1/deposit-requests")
                .header(header::ORIGIN, origin)
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .header(
                    header::ACCESS_CONTROL_REQUEST_HEADERS,
                    "authorization, content-type, idempotency-key",
                )
                .body(Body::empty())
                .unwrap()
        };

        // The preflight carries no credential and is answered before auth.
        let allowed = app.clone().oneshot(preflight(&origin)).await.unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
        let headers = allowed.headers();
        assert_eq!(headers[header::ACCESS_CONTROL_ALLOW_ORIGIN], origin);
        let methods = headers[header::ACCESS_CONTROL_ALLOW_METHODS]
            .to_str()
            .unwrap()
            .to_owned();
        // Every method a browser client uses, or its preflight fails before
        // the request is ever made.
        for method in ["GET", "POST", "PATCH", "PUT", "DELETE", "OPTIONS"] {
            assert!(methods.contains(method), "{methods}");
        }
        let allowed_headers = headers[header::ACCESS_CONTROL_ALLOW_HEADERS]
            .to_str()
            .unwrap()
            .to_ascii_lowercase();
        for name in ["authorization", "content-type", "accept", "idempotency-key"] {
            assert!(allowed_headers.contains(name), "{allowed_headers}");
        }
        assert_eq!(headers[header::ACCESS_CONTROL_MAX_AGE], "86400");

        let foreign = app
            .clone()
            .oneshot(preflight("https://evil.example"))
            .await
            .unwrap();
        assert!(
            foreign
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none(),
            "a foreign origin is not allowed"
        );

        // Actual requests echo the exact origin, never a wildcard.
        let get = app
            .clone()
            .oneshot(
                Request::get("/v1/account")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .header(header::ORIGIN, &origin)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(get.status(), StatusCode::OK);
        assert_eq!(get.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN], origin);
        let foreign_get = app
            .oneshot(
                Request::get("/v1/account")
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .header(header::ORIGIN, "https://evil.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            foreign_get
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn an_issuer_identity_proves_its_contact_mailbox_before_it_counts(pool: PgPool) {
        let (app, tenant) = app_with_payer_verification(pool.clone()).await;

        let created = app
            .clone()
            .oneshot(json_request(
                "POST",
                KEY,
                "/v1/issuers",
                &json!({"name": "Acme Inc.", "contact_email": "Billing@Acme.example"}),
            ))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = json_body(created).await;
        let id = created["id"].as_str().unwrap().to_owned();
        assert_eq!(
            created["contact_email"], "billing@acme.example",
            "the address is normalized on the way in"
        );
        assert_eq!(created["email_verified"], false);
        assert!(created["payout_addresses"].as_array().unwrap().is_empty());

        // One name per account, so two identities are never the same row to a
        // merchant reading a list.
        for taken in ["Acme Inc.", "acme inc.", "  ACME INC.  "] {
            let refused = app
                .clone()
                .oneshot(json_request(
                    "POST",
                    KEY,
                    "/v1/issuers",
                    &json!({"name": taken, "contact_email": "other@acme.example"}),
                ))
                .await
                .unwrap();
            assert_eq!(refused.status(), StatusCode::CONFLICT, "{taken}");
            assert_eq!(
                json_body(refused).await["error"]["code"],
                "issuer_name_taken"
            );
        }

        // The address is the stored one; nothing in the request names it.
        let started = app
            .clone()
            .oneshot(json_request(
                "POST",
                KEY,
                &format!("/v1/issuers/{id}/verify/email/start"),
                &json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(started.status(), StatusCode::ACCEPTED);
        assert_eq!(
            json_body(started).await["contact_email"],
            "billing@acme.example"
        );
        assert_eq!(
            tenant.started.lock().unwrap().as_slice(),
            ["billing@acme.example"]
        );

        // One code per identity per minute, whoever asks.
        let again = app
            .clone()
            .oneshot(json_request(
                "POST",
                KEY,
                &format!("/v1/issuers/{id}/verify/email/start"),
                &json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(again.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(
            json_body(again).await["error"]["code"],
            "otp_resend_cooldown"
        );

        let wrong = app
            .clone()
            .oneshot(json_request(
                "POST",
                KEY,
                &format!("/v1/issuers/{id}/verify/email/confirm"),
                &json!({"otp": "000000"}),
            ))
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(json_body(wrong).await["error"]["code"], "otp_invalid");

        let confirmed = app
            .clone()
            .oneshot(json_request(
                "POST",
                KEY,
                &format!("/v1/issuers/{id}/verify/email/confirm"),
                &json!({"otp": OTP}),
            ))
            .await
            .unwrap();
        assert_eq!(confirmed.status(), StatusCode::OK);
        let confirmed = json_body(confirmed).await;
        assert_eq!(confirmed["email_verified"], true);
        assert!(confirmed["email_verified_at"].is_string());

        // Once proven there is nothing left to send or confirm.
        let spent = app
            .clone()
            .oneshot(json_request(
                "POST",
                KEY,
                &format!("/v1/issuers/{id}/verify/email/start"),
                &json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(spent.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(spent).await["error"]["code"],
            "issuer_email_already_verified"
        );

        // A rename alone never touches the proven mailbox: the update is
        // partial, so the fields left out keep their values.
        let renamed = json_body(
            app.clone()
                .oneshot(json_request(
                    "PATCH",
                    KEY,
                    &format!("/v1/issuers/{id}"),
                    &json!({"name": "Acme Inc."}),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(renamed["name"], "Acme Inc.");
        assert_eq!(renamed["contact_email"], "billing@acme.example");
        assert_eq!(renamed["email_verified"], true);

        // Moving the mailbox is a new claim, and starts unproven.
        let moved = app
            .clone()
            .oneshot(json_request(
                "PATCH",
                KEY,
                &format!("/v1/issuers/{id}"),
                &json!({"contact_email": "support@acme.example"}),
            ))
            .await
            .unwrap();
        let moved = json_body(moved).await;
        assert_eq!(moved["email_verified"], false);
        assert_eq!(moved["name"], "Acme Inc.");

        // Nobody else's identity, listed or fetched.
        let theirs = other_account(&pool, "payday_live_ffffffffffffffffffffffffffffffff").await;
        let _ = theirs;
        let foreign = app
            .clone()
            .oneshot(get_request(
                "payday_live_ffffffffffffffffffffffffffffffff",
                &format!("/v1/issuers/{id}"),
            ))
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_body(foreign).await["error"]["code"],
            "issuer_not_found"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn payout_addresses_are_saved_once_and_attached_to_identities(pool: PgPool) {
        let app = app(pool.clone()).await;
        let wallet = "0x70997970c51812dc3a010c7d01b50e0d17dc79c8";
        let checksummed = "0x70997970C51812dc3A010C7d01b50e0d17dc79C8";
        const OTHER_WALLET: &str = "0x3C44CdDdB6a900fa2b585dd299e03d12FA4293BC";

        let saved = app
            .clone()
            .oneshot(json_request(
                "POST",
                KEY,
                "/v1/payout-addresses",
                &json!({"address": wallet, "label": "Treasury"}),
            ))
            .await
            .unwrap();
        assert_eq!(saved.status(), StatusCode::CREATED);
        let saved = json_body(saved).await;
        assert_eq!(
            saved["address"], checksummed,
            "stored EIP-55 whatever case it arrived in"
        );
        let address_id = saved["id"].as_str().unwrap().to_owned();

        // The same wallet again is the same row, not a second one.
        let repeat = app
            .clone()
            .oneshot(json_request(
                "POST",
                KEY,
                "/v1/payout-addresses",
                &json!({"address": checksummed}),
            ))
            .await
            .unwrap();
        assert_eq!(json_body(repeat).await["id"], address_id);
        let listed = app
            .clone()
            .oneshot(get_request(KEY, "/v1/payout-addresses"))
            .await
            .unwrap();
        assert_eq!(
            json_body(listed).await["payout_addresses"]
                .as_array()
                .unwrap()
                .len(),
            1,
            "saving the same wallet twice is one row"
        );

        let rejected = app
            .clone()
            .oneshot(json_request(
                "POST",
                KEY,
                "/v1/payout-addresses",
                &json!({"address": "0xnope"}),
            ))
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);

        // A label is a short handle, not free text.
        for label in [
            "This label is far too long to be one",
            "-leading",
            "semi;colon",
            "emoji 🙂",
        ] {
            let refused = app
                .clone()
                .oneshot(json_request(
                    "POST",
                    KEY,
                    "/v1/payout-addresses",
                    &json!({"address": OTHER_WALLET, "label": label}),
                ))
                .await
                .unwrap();
            assert_eq!(refused.status(), StatusCode::BAD_REQUEST, "{label}");
        }
        let accepted = app
            .clone()
            .oneshot(json_request(
                "POST",
                KEY,
                "/v1/payout-addresses",
                &json!({"address": OTHER_WALLET, "label": "Ops (EU) & co."}),
            ))
            .await
            .unwrap();
        assert_eq!(accepted.status(), StatusCode::CREATED);

        let issuer = json_body(
            app.clone()
                .oneshot(json_request(
                    "POST",
                    KEY,
                    "/v1/issuers",
                    &json!({"name": "Acme", "contact_email": "billing@acme.example"}),
                ))
                .await
                .unwrap(),
        )
        .await;
        let issuer_id = issuer["id"].as_str().unwrap().to_owned();

        let attached = app
            .clone()
            .oneshot(json_request(
                "PUT",
                KEY,
                &format!("/v1/issuers/{issuer_id}/payout-addresses"),
                &json!({"payout_address_ids": [address_id]}),
            ))
            .await
            .unwrap();
        assert_eq!(attached.status(), StatusCode::OK);
        let attached = json_body(attached).await;
        assert_eq!(attached["payout_addresses"][0]["address"], checksummed);

        // An id that is not this account's reads as a missing address.
        let stranger = app
            .clone()
            .oneshot(json_request(
                "PUT",
                KEY,
                &format!("/v1/issuers/{issuer_id}/payout-addresses"),
                &json!({"payout_address_ids": [PayoutAddressId::generate()]}),
            ))
            .await
            .unwrap();
        assert_eq!(stranger.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_body(stranger).await["error"]["code"],
            "payout_address_not_found"
        );

        // A request issued under the identity keeps the link, and the link
        // survives the identity being renamed.
        let issued = json_body(
            app.clone()
                .oneshot(create_request(KEY, "issuer-link", &{
                    let mut body = valid_body();
                    body["issuer_id"] = json!(issuer_id);
                    body
                }))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(issued["issuer_id"], issuer_id);

        let renamed = app
            .clone()
            .oneshot(json_request(
                "PATCH",
                KEY,
                &format!("/v1/issuers/{issuer_id}"),
                &json!({"name": "Acme GmbH", "contact_email": "billing@acme.example"}),
            ))
            .await
            .unwrap();
        assert_eq!(json_body(renamed).await["name"], "Acme GmbH");
        let after = json_body(
            app.clone()
                .oneshot(get_request(
                    KEY,
                    &format!("/v1/deposit-requests/{}", issued["id"].as_str().unwrap()),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(after["issuer_id"], issuer_id, "the link is unchanged");
        assert_eq!(
            after["issuer"]["name"], "Acme",
            "and the document still says what it said when issued"
        );

        // An identity that history refers to cannot be deleted out from under it.
        let refused = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/v1/issuers/{issuer_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(json_body(refused).await["error"]["code"], "issuer_in_use");

        // A request is findable by the identity it was issued under, by the
        // customer it is billed to, and by where its verification stands —
        // three separate facts, three separate filters.
        let listed = |query: &str| get_request(KEY, &format!("/v1/deposit-requests?{query}"));
        for (query, expected) in [
            (format!("issuer_id={issuer_id}"), 1),
            (format!("issuer_id={}", IssuerId::generate()), 0),
            ("verification=not_required".to_string(), 1),
            ("verification=verified".to_string(), 0),
        ] {
            let page = json_body(app.clone().oneshot(listed(&query)).await.unwrap()).await;
            assert_eq!(
                page["deposit_requests"].as_array().unwrap().len(),
                expected,
                "{query}"
            );
        }
        let refused = app
            .clone()
            .oneshot(listed("verification=nonsense"))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);

        // The listing carries each identity's addresses with it.
        let page = json_body(
            app.clone()
                .oneshot(get_request(KEY, "/v1/issuers"))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(page["issuers"][0]["payout_addresses"][0]["id"], address_id);

        // Deleting the wallet takes its associations with it.
        let removed = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri(format!("/v1/payout-addresses/{address_id}"))
                    .header(header::AUTHORIZATION, format!("Bearer {KEY}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(removed.status(), StatusCode::NO_CONTENT);
        let after = json_body(
            app.clone()
                .oneshot(get_request(KEY, &format!("/v1/issuers/{issuer_id}")))
                .await
                .unwrap(),
        )
        .await;
        assert!(after["payout_addresses"].as_array().unwrap().is_empty());
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn customers_are_created_listed_updated_and_scoped_to_the_account(pool: PgPool) {
        let app = app(pool.clone()).await;
        let post = |body: Value| json_request("POST", KEY, "/v1/customers", &body);

        let created = app
            .clone()
            .oneshot(post(json!({
                "name": "Globex", "email": "ap@globex.example", "details": "Net 30"
            })))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = json_body(created).await;
        let id = created["id"].as_str().unwrap().to_owned();
        assert!(CustomerId::parse(&id).is_some(), "{id}");
        assert_eq!(created["name"], "Globex");
        assert_eq!(created["email"], "ap@globex.example");
        assert_eq!(created["details"], "Net 30");
        assert!(created["created_at"].is_string());
        assert_eq!(created["created_at"], created["updated_at"]);

        let second = app
            .clone()
            .oneshot(post(json!({"name": "Initech"})))
            .await
            .unwrap();
        let second = json_body(second).await;
        assert!(second["email"].is_null());
        assert!(second["details"].is_null());
        let second_id = second["id"].as_str().unwrap().to_owned();

        let fetched = app
            .clone()
            .oneshot(get_request(KEY, &format!("/v1/customers/{id}")))
            .await
            .unwrap();
        assert_eq!(fetched.status(), StatusCode::OK);
        let fetched = json_body(fetched).await;
        assert_eq!(fetched["name"], "Globex");
        // Only `get` carries stats, and a customer with no invoices yet reads
        // as zero rather than null.
        assert_eq!(fetched["stats"]["request_count"], 0);
        assert_eq!(fetched["stats"]["collected_base_units"], "0");
        assert_eq!(fetched["stats"]["pending_base_units"], "0");

        // Update is partial: a field left out keeps its value, and only an
        // explicit null clears one.
        let updated = app
            .clone()
            .oneshot(json_request(
                "PATCH",
                KEY,
                &format!("/v1/customers/{id}"),
                &json!({"name": "Globex Corp", "details": "Net 45"}),
            ))
            .await
            .unwrap();
        assert_eq!(updated.status(), StatusCode::OK);
        let updated = json_body(updated).await;
        assert_eq!(updated["name"], "Globex Corp");
        assert_eq!(
            updated["email"], created["email"],
            "an omitted field is kept"
        );
        assert_eq!(updated["details"], "Net 45");
        assert!(updated["updated_at"].as_str() >= created["updated_at"].as_str());
        let cleared = json_body(
            app.clone()
                .oneshot(json_request(
                    "PATCH",
                    KEY,
                    &format!("/v1/customers/{id}"),
                    &json!({"email": null}),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert!(cleared["email"].is_null(), "an explicit null clears");
        assert_eq!(cleared["name"], "Globex Corp");
        assert_eq!(cleared["details"], "Net 45");
        // The merged record is validated whole.
        let blank = app
            .clone()
            .oneshot(json_request(
                "PATCH",
                KEY,
                &format!("/v1/customers/{id}"),
                &json!({"name": "  "}),
            ))
            .await
            .unwrap();
        assert_eq!(blank.status(), StatusCode::BAD_REQUEST);

        let page = app
            .clone()
            .oneshot(get_request(KEY, "/v1/customers?limit=1"))
            .await
            .unwrap();
        assert_eq!(page.status(), StatusCode::OK);
        let page = json_body(page).await;
        assert_eq!(page["customers"].as_array().unwrap().len(), 1);
        assert_eq!(page["customers"][0]["id"], second_id, "newest first");
        assert_eq!(page["next_cursor"], second_id);
        let rest = app
            .clone()
            .oneshot(get_request(
                KEY,
                &format!("/v1/customers?limit=1&starting_after={second_id}"),
            ))
            .await
            .unwrap();
        let rest = json_body(rest).await;
        assert_eq!(rest["customers"][0]["id"], id);
        assert!(rest["next_cursor"].is_null());

        for query in [
            format!("starting_after={}", Uuid::now_v7()),
            "limit=0".into(),
            "limit=101".into(),
            "sort=name".into(),
        ] {
            let response = app
                .clone()
                .oneshot(get_request(KEY, &format!("/v1/customers?{query}")))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
        }
        for (name, body) in [
            ("blank name", json!({"name": "  "})),
            ("long name", json!({"name": "x".repeat(256)})),
            ("short email", json!({"name": "x", "email": "ab"})),
            (
                "long details",
                json!({"name": "x", "details": "d".repeat(4001)}),
            ),
            ("unknown field", json!({"name": "x", "phone": "1"})),
            // Postgres cannot store U+0000; it is refused before the insert.
            // Details may wrap, a name may not.
            ("nul in name", json!({"name": "Glo\u{0}bex"})),
            (
                "nul in details",
                json!({"name": "Globex", "details": "Net\u{0}30"}),
            ),
            ("line break in name", json!({"name": "Globex\nCorp"})),
        ] {
            let response = app.clone().oneshot(post(body)).await.unwrap();
            assert!(response.status().is_client_error(), "{name}");
            assert_eq!(
                json_body(response).await["error"]["code"],
                "invalid_request",
                "{name}"
            );
        }
        let wrapped = app
            .clone()
            .oneshot(post(
                json!({"name": "Wrapped", "details": "1 Main St\nSpringfield"}),
            ))
            .await
            .unwrap();
        assert_eq!(wrapped.status(), StatusCode::CREATED);

        const OTHER: &str = "payday_live_other0123456789abcdef0123456789abcdef";
        other_account(&pool, OTHER).await;
        let foreign_get = app
            .clone()
            .oneshot(get_request(OTHER, &format!("/v1/customers/{id}")))
            .await
            .unwrap();
        assert_eq!(foreign_get.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            json_body(foreign_get).await["error"]["code"],
            "customer_not_found"
        );
        let foreign_update = app
            .clone()
            .oneshot(json_request(
                "PATCH",
                OTHER,
                &format!("/v1/customers/{id}"),
                &json!({"name": "Taken"}),
            ))
            .await
            .unwrap();
        assert_eq!(foreign_update.status(), StatusCode::NOT_FOUND);
        let foreign_list = app
            .clone()
            .oneshot(get_request(OTHER, "/v1/customers"))
            .await
            .unwrap();
        assert!(
            json_body(foreign_list).await["customers"]
                .as_array()
                .unwrap()
                .is_empty()
        );
        let malformed = app
            .clone()
            .oneshot(get_request(KEY, "/v1/customers/not-a-uuid"))
            .await
            .unwrap();
        assert_eq!(malformed.status(), StatusCode::NOT_FOUND);

        // The record is what an invoice links to.
        let mut body = valid_body();
        body["customer_id"] = json!(id);
        let invoice = app
            .clone()
            .oneshot(create_request(KEY, "for-customer", &body))
            .await
            .unwrap();
        assert_eq!(invoice.status(), StatusCode::CREATED);
        assert_eq!(json_body(invoice).await["customer_id"], id);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn invoice_pdf_is_served_as_a_deterministic_download(pool: PgPool) {
        let app = app(pool).await;
        let mut body = valid_body();
        body["heading"] = json!("March retainer");
        body["reference"] = json!("INV-1");
        body["notes"] = json!("Net 30");
        let created = app
            .clone()
            .oneshot(create_request(KEY, "pdf", &body))
            .await
            .unwrap();
        let id = json_body(created).await["id"].as_str().unwrap().to_owned();

        let fetch = || {
            app.clone().oneshot(get_request(
                KEY,
                &format!("/v1/deposit-requests/{id}/request.pdf"),
            ))
        };
        let first = fetch().await.unwrap();
        assert_eq!(first.status(), StatusCode::OK);
        assert_eq!(first.headers()[header::CONTENT_TYPE], "application/pdf");
        assert_eq!(
            first.headers()[header::CONTENT_DISPOSITION],
            format!("attachment; filename=\"deposit-request-{id}.pdf\"").as_str()
        );
        assert_eq!(first.headers()[header::CACHE_CONTROL], "no-store");
        let first = to_bytes(first.into_body(), 1024 * 1024).await.unwrap();
        assert!(first.starts_with(b"%PDF-"));
        let text = String::from_utf8_lossy(&first);
        assert!(text.contains("(INV-1)"));
        assert!(text.contains("(Net 30)"));

        let second = fetch().await.unwrap();
        let second = to_bytes(second.into_body(), 1024 * 1024).await.unwrap();
        assert_eq!(first, second, "byte-identical on every fetch");

        assert_unauthorized(
            app.clone(),
            Request::get(format!("/v1/deposit-requests/{id}/request.pdf"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn proof_is_served_after_settlement_and_verifies_offline(pool: PgPool) {
        let app = app(pool.clone()).await;
        let created = app
            .clone()
            .oneshot(create_request(KEY, "proof", &valid_body()))
            .await
            .unwrap();
        let created = json_body(created).await;
        let id = created["id"].as_str().unwrap().to_owned();
        let uuid = Uuid::parse_str(id.strip_prefix("dr_").unwrap()).unwrap();
        let (bound, _) = bind_wallet(&app, &id, None, &PAYER_KEY).await;
        let address = Address::parse_checksummed(bound["address"].as_str().unwrap(), None).unwrap();
        let payer_wallet = gateway_core::wallet_of(&PAYER_KEY);
        let proof_request =
            |key: &str| get_request(key, &format!("/v1/deposit-requests/{id}/proof"));

        let early = app.clone().oneshot(proof_request(KEY)).await.unwrap();
        assert_eq!(early.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(early).await["error"]["code"],
            "deposit_request_not_settled"
        );

        // Settle it the way the indexer would: one credited transfer, then
        // the fulfilment transaction.
        let settlement = [9u8; 32];
        sqlx::query(
            r#"INSERT INTO payment_observations
                 (chain_id, token_address, block_number, block_hash, block_timestamp,
                  transaction_hash, transaction_index, log_index, sender_address,
                  recipient_address, invoice_id, amount, disposition)
               VALUES (1, $1, 3, $2, 1800000000, $3, 0, 0, $4, $5, $6, '1000000', 'credited')"#,
        )
        .bind(Address::ZERO.as_slice())
        .bind([1u8; 32].as_slice())
        .bind([3u8; 32].as_slice())
        .bind(payer_wallet.as_slice())
        .bind(address.as_slice())
        .bind(uuid)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE invoices SET status = 'fulfilled', confirmed_received = '1000000', settlement_tx_hash = $2, resolved_at_block = 9, settled_at = now() WHERE id = $1",
        )
        .bind(uuid)
        .bind(settlement.as_slice())
        .execute(&pool)
        .await
        .unwrap();

        // Money from a stranger's wallet was credited and settled, but it is
        // not the attested wallet paying, so no proof claims it was.
        sqlx::query(
            r#"INSERT INTO payment_observations
                 (chain_id, token_address, block_number, block_hash, block_timestamp,
                  transaction_hash, transaction_index, log_index, sender_address,
                  recipient_address, invoice_id, amount, disposition)
               VALUES (1, $1, 4, $2, 1800000001, $3, 0, 0, $4, $5, $6, '5', 'credited')"#,
        )
        .bind(Address::ZERO.as_slice())
        .bind([2u8; 32].as_slice())
        .bind([4u8; 32].as_slice())
        .bind(Address::repeat_byte(0xf3).as_slice())
        .bind(address.as_slice())
        .bind(uuid)
        .execute(&pool)
        .await
        .unwrap();
        let mismatch = app.clone().oneshot(proof_request(KEY)).await.unwrap();
        assert_eq!(mismatch.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(mismatch).await["error"]["code"],
            "deposit_sender_mismatch"
        );
        sqlx::query("DELETE FROM payment_observations WHERE transaction_hash = $1")
            .bind([4u8; 32].as_slice())
            .execute(&pool)
            .await
            .unwrap();

        let served = app.clone().oneshot(proof_request(KEY)).await.unwrap();
        assert_eq!(served.status(), StatusCode::OK);
        let proof: ProofOfPayment = serde_json::from_value(json_body(served).await).unwrap();
        assert_eq!(proof.payment_id, id);
        assert_eq!(proof.payment_address, address.to_checksum(None));
        assert_eq!(proof.verification.payload.result, "not_required");
        assert_eq!(
            proof.verification.payload.payer_policy_mode,
            "permissionless"
        );
        assert!(proof.verification.payload.verified_at.is_none());
        assert_eq!(
            proof.verification.payload.payer_wallet,
            payer_wallet.to_checksum(None)
        );
        assert_eq!(proof.payer_wallet.address, payer_wallet.to_checksum(None));
        assert_eq!(proof.recovery_address, payer_wallet.to_checksum(None));
        assert_eq!(
            proof
                .verification
                .payload
                .facts
                .iter()
                .map(|fact| fact.kind.as_str())
                .collect::<Vec<_>>(),
            ["wallet"]
        );
        assert_eq!(
            proof.verification.signer,
            attestor().address().to_checksum(None)
        );
        assert_eq!(proof.transfers.len(), 1);
        assert_eq!(proof.transfers[0].amount_base_units, "1000000");
        // The proof names the fulfilment transaction recorded on the row;
        // the transfers are what paid the address.
        assert_eq!(
            proof.settlement_transaction_hash,
            alloy_primitives::B256::from(settlement).to_string()
        );
        assert_eq!(
            proof.transfers[0].transaction_hash,
            alloy_primitives::B256::from([3u8; 32]).to_string()
        );

        let verified = verify_proof(&proof, None, &[attestor().address()])
            .unwrap_or_else(|error| panic!("the served proof must verify offline: {error}"));
        assert_eq!(verified.payment_address, address);
        assert_eq!(verified.payer_wallet, payer_wallet);
        assert_eq!(verified.attestation_signer, attestor().address());

        let mut tampered = proof.clone();
        tampered.canonical_issuance_snapshot.notes = Some("edited after payment".into());
        assert!(matches!(
            verify_proof(&tampered, None, &[attestor().address()]).unwrap_err(),
            ProofError::AttributionHashMismatch
        ));
        assert!(matches!(
            verify_proof(&proof, None, &[Address::repeat_byte(0x01)]).unwrap_err(),
            ProofError::UntrustedAttestor
        ));

        // Fulfilled proof inputs are immutable, so repeated owned GETs return
        // the cached signed package rather than querying/signing it again.
        let repeated = app.clone().oneshot(proof_request(KEY)).await.unwrap();
        assert_eq!(repeated.status(), StatusCode::OK);
        assert_eq!(
            json_body(repeated).await,
            serde_json::to_value(&proof).unwrap()
        );

        const OTHER: &str = "payday_live_other0123456789abcdef0123456789abcdef";
        other_account(&pool, OTHER).await;
        let foreign = app.oneshot(proof_request(OTHER)).await.unwrap();
        assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn email_verification_unlocks_only_the_verifying_session(pool: PgPool) {
        let (app, tenant) = app_with_payer_verification(pool.clone()).await;
        let (email_id, email_created) = create_gated(
            &app,
            "gated-email",
            json!({"mode": "verified_email", "expected_email": "Alice@Example.com"}),
            "Email retainer",
        )
        .await;
        let (other_id, _) = create_gated(
            &app,
            "gated-other",
            json!({"mode": "verified_email", "expected_email": "bob@example.com"}),
            "Other retainer",
        )
        .await;

        // The mailbox comes from the merchant's assertion, normalized; the
        // payer names nothing and learns only the masked hint.
        let session = start_session(&app, &email_id).await;
        assert_eq!(*tenant.started.lock().unwrap(), ["alice@example.com"]);
        let pending = app
            .clone()
            .oneshot(payer_get(
                &format!("/v1/payer/deposit-requests/{email_id}"),
                Some(&session),
            ))
            .await
            .unwrap();
        let pending = json_body(pending).await;
        assert_eq!(pending["content_unlocked"], false);
        assert_eq!(pending["requirements"]["email"], "pending");
        assert_eq!(
            pending["payer_policy"]["expected_email_hint"],
            "a****@e***.com"
        );
        assert!(pending["amount"].is_null());
        assert!(!pending.to_string().contains("alice@example.com"));

        // One code per invoice per minute, whoever asks.
        let again = app
            .clone()
            .oneshot(payer_post(
                &format!("/v1/payer/deposit-requests/{email_id}/verify/email/start"),
                Some(&session),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(again.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(again.headers().contains_key(header::RETRY_AFTER));
        assert_eq!(
            json_body(again).await["error"]["code"],
            "otp_resend_cooldown"
        );
        assert_eq!(tenant.started.lock().unwrap().len(), 1);

        let confirm_path = format!("/v1/payer/deposit-requests/{email_id}/verify/email/confirm");
        let wrong = app
            .clone()
            .oneshot(payer_post(
                &confirm_path,
                Some(&session),
                Some(&json!({"otp": "000000"})),
            ))
            .await
            .unwrap();
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(json_body(wrong).await["error"]["code"], "otp_invalid");
        let no_session = app
            .clone()
            .oneshot(payer_post(&confirm_path, None, Some(&json!({"otp": OTP}))))
            .await
            .unwrap();
        assert_eq!(no_session.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(no_session).await["error"]["code"],
            "payer_session_invalid"
        );
        let bogus = app
            .clone()
            .oneshot(payer_post(
                &confirm_path,
                Some("not-a-session"),
                Some(&json!({"otp": OTP})),
            ))
            .await
            .unwrap();
        assert_eq!(bogus.status(), StatusCode::UNAUTHORIZED);
        let malformed = app
            .clone()
            .oneshot(payer_post(
                &confirm_path,
                Some(&session),
                Some(&json!({"otp": "12 34"})),
            ))
            .await
            .unwrap();
        assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
        let unknown_field = app
            .clone()
            .oneshot(payer_post(
                &confirm_path,
                Some(&session),
                Some(&json!({"otp": OTP, "email": "mallory@example.com"})),
            ))
            .await
            .unwrap();
        assert_eq!(unknown_field.status(), StatusCode::BAD_REQUEST);
        let unknown_field = json_body(unknown_field).await;
        assert_eq!(unknown_field["error"]["code"], "invalid_request");
        assert!(
            unknown_field["error"]["message"]
                .as_str()
                .unwrap()
                .contains("unknown field `email`"),
            "{unknown_field}"
        );

        let confirmed = app
            .clone()
            .oneshot(payer_post(
                &confirm_path,
                Some(&session),
                Some(&json!({"otp": OTP})),
            ))
            .await
            .unwrap();
        assert_eq!(confirmed.status(), StatusCode::OK);
        let confirmed = json_body(confirmed).await;
        assert_eq!(confirmed["requirements"]["email"], "approved");
        assert_eq!(confirmed["requirements"]["complete"], true);
        assert!(confirmed.get("identity_start_available").is_none());
        // The code is spent with the attempt.
        let spent = app
            .clone()
            .oneshot(payer_post(
                &confirm_path,
                Some(&session),
                Some(&json!({"otp": OTP})),
            ))
            .await
            .unwrap();
        assert_eq!(spent.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(spent).await["error"]["code"],
            "verification_not_started"
        );

        // The verifying session sees everything; the bare link still does
        // not, even though the invoice as a whole is now complete.
        let unlocked = app
            .clone()
            .oneshot(payer_get(
                &format!("/v1/payer/deposit-requests/{email_id}"),
                Some(&session),
            ))
            .await
            .unwrap();
        assert_eq!(unlocked.status(), StatusCode::OK);
        let unlocked = json_body(unlocked).await;
        assert_eq!(unlocked["content_unlocked"], true);
        assert_eq!(unlocked["amount"], "1.000000");
        assert_eq!(unlocked["details"]["payer"]["name"], "Globex");
        assert_eq!(unlocked["details"]["reference"], "INV-9");
        // Unlocked is not yet payable: the address waits for the wallet
        // step, which this verified session may now take.
        assert!(email_created["address"].is_null());
        assert!(unlocked["address"].is_null());
        assert!(unlocked["deposit_uri"].is_null());
        assert_eq!(unlocked["requirements"]["wallet"], "pending");
        let (bound, _) = bind_wallet(&app, &email_id, Some(&session), &PAYER_KEY).await;
        assert!(bound["address"].as_str().unwrap().starts_with("0x"));
        assert_eq!(bound["requirements"]["wallet"], "approved");
        assert!(
            bound["deposit_uri"]
                .as_str()
                .unwrap()
                .starts_with("ethereum:")
        );
        let merchant_view = app
            .clone()
            .oneshot(get_request(
                KEY,
                &format!("/v1/deposit-requests/{email_id}"),
            ))
            .await
            .unwrap();
        assert_eq!(json_body(merchant_view).await["address"], bound["address"]);
        let bare = app
            .clone()
            .oneshot(payer_get(
                &format!("/v1/payer/deposit-requests/{email_id}"),
                None,
            ))
            .await
            .unwrap();
        let bare = json_body(bare).await;
        assert_eq!(bare["content_unlocked"], false);
        assert_eq!(bare["requirements"]["email"], "pending");
        assert!(bare["address"].is_null());
        let stale = app
            .clone()
            .oneshot(payer_get(
                &format!("/v1/payer/deposit-requests/{email_id}"),
                Some("expired-or-forged"),
            ))
            .await
            .unwrap();
        assert_eq!(json_body(stale).await["content_unlocked"], false);

        // The QR needs that session, and only that session.
        let qr = app
            .clone()
            .oneshot(payer_get(
                &format!("/v1/payer/deposit-requests/{email_id}/qr"),
                Some(&session),
            ))
            .await
            .unwrap();
        assert_eq!(qr.status(), StatusCode::OK);
        assert!(
            qr.headers()[header::CONTENT_TYPE]
                .to_str()
                .unwrap()
                .starts_with("image/svg+xml")
        );
        let qr_bare = app
            .clone()
            .oneshot(payer_get(
                &format!("/v1/payer/deposit-requests/{email_id}/qr"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(qr_bare.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(qr_bare).await["error"]["code"],
            "verification_required"
        );

        // Another invoice is a stranger to this session.
        let other = app
            .clone()
            .oneshot(payer_get(
                &format!("/v1/payer/deposit-requests/{other_id}"),
                Some(&session),
            ))
            .await
            .unwrap();
        let other = json_body(other).await;
        assert_eq!(other["content_unlocked"], false);
        assert_eq!(other["requirements"]["email"], "pending");
        let other_qr = app
            .clone()
            .oneshot(payer_get(
                &format!("/v1/payer/deposit-requests/{other_id}/qr"),
                Some(&session),
            ))
            .await
            .unwrap();
        assert_eq!(other_qr.status(), StatusCode::UNAUTHORIZED);
        let other_status = app
            .clone()
            .oneshot(payer_get(
                &format!("/v1/payer/deposit-requests/{other_id}/verify"),
                Some(&session),
            ))
            .await
            .unwrap();
        assert_eq!(other_status.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(other_status).await["error"]["code"],
            "payer_session_invalid"
        );
        let own_status = app
            .clone()
            .oneshot(payer_get(
                &format!("/v1/payer/deposit-requests/{email_id}/verify"),
                Some(&session),
            ))
            .await
            .unwrap();
        assert_eq!(
            json_body(own_status).await["requirements"]["complete"],
            true
        );

        // The merchant sees the invoice complete; settlement may proceed.
        let merchant = json_body(
            app.clone()
                .oneshot(get_request(
                    KEY,
                    &format!("/v1/deposit-requests/{email_id}"),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert!(merchant["verification_completed_at"].is_string());

        // A terminal invoice never accepts an expired bearer. Re-proving the
        // asserted mailbox explicitly mints a fresh, invoice-scoped receipt session.
        let email_uuid = Uuid::parse_str(email_id.strip_prefix("dr_").unwrap()).unwrap();
        sqlx::query("UPDATE invoices SET status = 'fulfilled', settlement_tx_hash = $2, settled_at = now() WHERE id = $1")
            .bind(email_uuid)
            .bind([0x44u8; 32].as_slice())
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE payer_sessions SET created_at = now() - interval '25 hours', expires_at = now() - interval '1 hour' WHERE invoice_id = $1")
            .bind(email_uuid)
            .execute(&pool)
            .await
            .unwrap();
        let expired = payer_read(&app, &email_id, Some(&session)).await;
        assert_eq!(expired["content_unlocked"], false);
        sqlx::query("UPDATE payer_verifications SET created_at = now() - interval '2 minutes' WHERE invoice_id = $1")
            .bind(email_uuid)
            .execute(&pool)
            .await
            .unwrap();
        let receipt_session = start_session(&app, &email_id).await;
        confirm_email(&app, &email_id, &receipt_session).await;
        let receipt = payer_read(&app, &email_id, Some(&receipt_session)).await;
        assert_eq!(receipt["content_unlocked"], true);

        // The merchant's verification view lists every attempt on the
        // invoice and nothing about the sessions behind them.
        let activity = json_body(
            app.clone()
                .oneshot(get_request(
                    KEY,
                    &format!("/v1/deposit-requests/{email_id}/verification"),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(activity["payer_policy_mode"], "verified_email");
        assert!(activity["verification_completed_at"].is_string());
        assert_eq!(activity["facts"]["email"], "approved");
        assert_eq!(activity["facts"]["complete"], true);
        let attempts = activity["attempts"].as_array().unwrap();
        assert!(attempts.len() >= 3, "{activity}");
        assert!(
            attempts
                .iter()
                .all(|attempt| attempt["kind"] == "email" || attempt["kind"] == "wallet")
        );
        assert_eq!(activity["facts"]["wallet"], "approved");
        assert!(
            attempts.iter().any(
                |attempt| attempt["status"] == "approved" && attempt["verified_at"].is_string()
            )
        );
        assert!(!activity.to_string().contains("alice@example.com"));
        assert!(!activity.to_string().contains("payer_session"));
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn email_verification_refuses_open_closed_and_unconfigured_invoices(pool: PgPool) {
        let (app, tenant) = app_with_payer_verification(pool.clone()).await;
        let open = json_body(
            app.clone()
                .oneshot(create_request(KEY, "open", &valid_body()))
                .await
                .unwrap(),
        )
        .await;
        let open_id = open["id"].as_str().unwrap();
        let refused = app
            .clone()
            .oneshot(payer_post(
                &format!("/v1/payer/deposit-requests/{open_id}/verify/email/start"),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(refused).await["error"]["code"],
            "verification_not_required"
        );
        let open_status = json_body(
            app.clone()
                .oneshot(payer_get(
                    &format!("/v1/payer/deposit-requests/{open_id}/verify"),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(open_status["requirements"]["complete"], true);
        assert_eq!(open_status["requirements"]["email"], "not_required");

        // Verification after the deadline cannot revive anything, so it is
        // not even started.
        let (expired_id, _) = create_gated(
            &app,
            "expired",
            json!({"mode": "verified_email", "expected_email": "alice@example.com"}),
            "Too late",
        )
        .await;
        sqlx::query("UPDATE invoices SET status = 'expired' WHERE id = $1")
            .bind(Uuid::parse_str(expired_id.strip_prefix("dr_").unwrap()).unwrap())
            .execute(&pool)
            .await
            .unwrap();
        let too_late = app
            .clone()
            .oneshot(payer_post(
                &format!("/v1/payer/deposit-requests/{expired_id}/verify/email/start"),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(too_late.status(), StatusCode::GONE);
        assert!(tenant.started.lock().unwrap().is_empty());

        let unknown = app
            .clone()
            .oneshot(payer_post(
                "/v1/payer/deposit-requests/dr_not-an-id/verify/email/start",
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);

        // A tenant outage is reported as such, not as a wrong code.
        let (gated_id, _) = create_gated(
            &app,
            "outage",
            json!({"mode": "verified_email", "expected_email": "alice@example.com"}),
            "Outage",
        )
        .await;
        *tenant.outage.lock().unwrap() = true;
        let down = app
            .clone()
            .oneshot(payer_post(
                &format!("/v1/payer/deposit-requests/{gated_id}/verify/email/start"),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(down.status(), StatusCode::BAD_GATEWAY);
        assert_eq!(
            json_body(down).await["error"]["code"],
            "identity_provider_unavailable"
        );

        // Without a payer audience the routes say so; the reads still work.
        let plain = build(pool.clone(), None, Address::ZERO).await.router;
        let unavailable = plain
            .clone()
            .oneshot(payer_post(
                &format!("/v1/payer/deposit-requests/{gated_id}/verify/email/start"),
                None,
                None,
            ))
            .await
            .unwrap();
        assert_eq!(unavailable.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            json_body(unavailable).await["error"]["code"],
            "verification_unavailable"
        );
        let read = plain
            .oneshot(payer_get(
                &format!("/v1/payer/deposit-requests/{gated_id}"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(read.status(), StatusCode::OK);
    }

    /// The merchant-session create body: the merchant's app has signed the
    /// payer in and names them by its own id.
    fn merchant_session_body(payer_reference: &str) -> Value {
        let mut body = valid_body();
        body["heading"] = json!("Deposit 1 USDC");
        body["reference"] = json!("dep-8042");
        body["metadata"] = json!({"order": "8042"});
        body["payer_policy"] =
            json!({"mode": "merchant_session", "payer_reference": payer_reference});
        body
    }

    /// Exchange a client secret from the hosted checkout.
    fn exchange_request(id: &str, secret: &str) -> Request<Body> {
        payer_post(
            &format!("/v1/payer/deposit-requests/{id}/session"),
            None,
            Some(&json!({"client_secret": secret})),
        )
    }

    /// Every webhook event enqueued for one invoice, oldest first, as
    /// (type, payload).
    async fn webhook_events(pool: &PgPool, id: &str) -> Vec<(String, Value)> {
        sqlx::query_as(
            "SELECT event_type, payload FROM webhook_events WHERE invoice_id = $1 ORDER BY created_at, id",
        )
        .bind(Uuid::parse_str(id.strip_prefix("dr_").unwrap()).unwrap())
        .fetch_all(pool)
        .await
        .unwrap()
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn a_merchant_session_opens_the_checkout_once_and_carries_the_payer_reference_to_settlement(
        pool: PgPool,
    ) {
        // No payer audience is configured: merchant sessions never touch the
        // email provider.
        let app = app(pool.clone()).await;

        // 1. The merchant's server creates the deposit request and receives
        //    the secret exactly once.
        let created = app
            .clone()
            .oneshot(create_request(
                KEY,
                "ms-1",
                &merchant_session_body("user_123"),
            ))
            .await
            .unwrap();
        assert_eq!(created.status(), StatusCode::CREATED);
        let created = json_body(created).await;
        let id = created["id"].as_str().unwrap().to_owned();
        let uuid = Uuid::parse_str(id.strip_prefix("dr_").unwrap()).unwrap();
        let secret = created["client_secret"].as_str().unwrap().to_owned();
        assert!(secret.starts_with("cs_"), "{secret}");
        assert_eq!(secret.len(), 46);
        assert!(created["client_secret_expires_at"].is_string());
        assert_eq!(created["payer_policy"]["mode"], "merchant_session");
        assert_eq!(created["payer_policy"]["payer_reference"], "user_123");
        assert!(created["payer_policy"].get("expected_email").is_none());
        assert!(created["verification_completed_at"].is_null());
        assert_eq!(
            created["deposit_url"],
            format!("http://127.0.0.1:3000/pay/{id}")
        );

        // A replay is the same invoice without the secret; a different
        // payer is a different request.
        let replay = app
            .clone()
            .oneshot(create_request(
                KEY,
                "ms-1",
                &merchant_session_body("user_123"),
            ))
            .await
            .unwrap();
        assert_eq!(replay.status(), StatusCode::OK);
        assert_eq!(replay.headers()["Idempotency-Replayed"], "true");
        let replay = json_body(replay).await;
        assert_eq!(replay["id"], id);
        assert!(replay.get("client_secret").is_none());
        assert!(replay.get("client_secret_expires_at").is_none());
        let conflict = app
            .clone()
            .oneshot(create_request(
                KEY,
                "ms-1",
                &merchant_session_body("user_456"),
            ))
            .await
            .unwrap();
        assert_eq!(conflict.status(), StatusCode::CONFLICT);
        // Neither does a merchant read: the secret lives only in the response
        // that minted it.
        let fetched = json_body(
            app.clone()
                .oneshot(get_request(KEY, &format!("/v1/deposit-requests/{id}")))
                .await
                .unwrap(),
        )
        .await;
        assert!(fetched.get("client_secret").is_none());
        assert_eq!(fetched["payer_policy"]["payer_reference"], "user_123");
        let stored: Vec<Vec<u8>> = sqlx::query_scalar(
            "SELECT secret_hash FROM payer_client_secrets WHERE invoice_id = $1",
        )
        .bind(uuid)
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(stored, vec![Sha256::digest(secret.as_bytes()).to_vec()]);

        // 2. Whoever holds the bare link sees a locked page that says the
        //    app must open it, and learns nothing about the payer.
        let bare = payer_read(&app, &id, None).await;
        assert_eq!(bare["content_unlocked"], false);
        assert_eq!(bare["payer_policy"]["mode"], "merchant_session");
        assert!(bare["payer_policy"]["expected_email_hint"].is_null());
        assert_eq!(bare["requirements"]["merchant_session"], "pending");
        assert_eq!(bare["requirements"]["email"], "not_required");
        assert_eq!(bare["requirements"]["complete"], false);
        assert_eq!(bare["issuer_name"], "Acme");
        assert_eq!(bare["heading"], "Deposit 1 USDC");
        assert!(bare["amount"].is_null());
        assert!(bare["address"].is_null());
        assert!(!bare.to_string().contains("user_123"));

        // 3. The checkout exchanges the secret. Guesses and secrets for
        //    other payments fail alike; the real one mints the session.
        let (other_id, other_created) = create_gated(
            &app,
            "ms-other",
            json!({"mode": "merchant_session", "payer_reference": "user_999"}),
            "Other deposit",
        )
        .await;
        let other_secret = other_created["client_secret"].as_str().unwrap();
        for (name, wrong) in [
            ("malformed", "not-a-secret"),
            ("wrong shape", "cs_short"),
            ("another payment's", other_secret),
            (
                "right shape, unknown",
                "cs_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            ),
        ] {
            let refused = app
                .clone()
                .oneshot(exchange_request(&id, wrong))
                .await
                .unwrap();
            assert_eq!(refused.status(), StatusCode::UNAUTHORIZED, "{name}");
            assert_eq!(
                json_body(refused).await["error"]["code"],
                "client_secret_invalid",
                "{name}"
            );
        }
        let sessions: i64 =
            sqlx::query_scalar("SELECT count(*) FROM payer_sessions WHERE invoice_id = $1")
                .bind(uuid)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(sessions, 0, "a refused exchange mints nothing");
        let extra_field = app
            .clone()
            .oneshot(payer_post(
                &format!("/v1/payer/deposit-requests/{id}/session"),
                None,
                Some(&json!({"client_secret": secret, "payer_reference": "user_1"})),
            ))
            .await
            .unwrap();
        assert_eq!(extra_field.status(), StatusCode::BAD_REQUEST);

        let opened = app
            .clone()
            .oneshot(exchange_request(&id, &secret))
            .await
            .unwrap();
        assert_eq!(opened.status(), StatusCode::OK);
        assert_eq!(opened.headers()[header::CACHE_CONTROL], "no-store");
        let opened = json_body(opened).await;
        let session = opened["payer_session"].as_str().unwrap().to_owned();
        assert!(opened["expires_at"].is_string());
        assert_eq!(opened["requirements"]["merchant_session"], "approved");
        assert_eq!(opened["requirements"]["email"], "not_required");
        assert_eq!(opened["requirements"]["complete"], true);

        // Once. The same URL pasted into a second window is told so.
        let again = app
            .clone()
            .oneshot(exchange_request(&id, &secret))
            .await
            .unwrap();
        assert_eq!(again.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(again).await["error"]["code"],
            "client_secret_used"
        );

        // 4. The session unlocks this payment, and only this payment; the
        //    bare link stays locked even though the invoice is now verified.
        let unlocked = payer_read(&app, &id, Some(&session)).await;
        assert_eq!(unlocked["content_unlocked"], true);
        assert_eq!(unlocked["amount"], "1.000000");
        assert_eq!(unlocked["details"]["reference"], "dep-8042");
        assert_eq!(unlocked["requirements"]["complete"], true);
        // Unlocked is not yet payable: the address waits for the wallet
        // step, which this verified session may now take.
        assert!(created["address"].is_null());
        assert!(unlocked["address"].is_null());
        assert!(unlocked["deposit_uri"].is_null());
        assert_eq!(unlocked["requirements"]["wallet"], "pending");
        let (bound, _) = bind_wallet(&app, &id, Some(&session), &PAYER_KEY).await;
        assert!(bound["address"].as_str().unwrap().starts_with("0x"));
        assert_eq!(bound["requirements"]["wallet"], "approved");
        assert!(
            bound["deposit_uri"]
                .as_str()
                .unwrap()
                .starts_with("ethereum:")
        );
        let qr = app
            .clone()
            .oneshot(payer_get(
                &format!("/v1/payer/deposit-requests/{id}/qr"),
                Some(&session),
            ))
            .await
            .unwrap();
        assert_eq!(qr.status(), StatusCode::OK);
        let still_bare = payer_read(&app, &id, None).await;
        assert_eq!(still_bare["content_unlocked"], false);
        assert_eq!(still_bare["requirements"]["merchant_session"], "pending");
        let stranger = payer_read(&app, &other_id, Some(&session)).await;
        assert_eq!(stranger["content_unlocked"], false);
        let own_status = json_body(
            app.clone()
                .oneshot(payer_get(
                    &format!("/v1/payer/deposit-requests/{id}/verify"),
                    Some(&session),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(own_status["requirements"]["merchant_session"], "approved");

        // 5. The exchange completed the invoice's verification, which the
        //    merchant sees and which raised verification.approved with the
        //    merchant's own payer reference on it; the wallet step then
        //    raised deposit_request.ready.
        let merchant = json_body(
            app.clone()
                .oneshot(get_request(KEY, &format!("/v1/deposit-requests/{id}")))
                .await
                .unwrap(),
        )
        .await;
        assert!(merchant["verification_completed_at"].is_string());
        assert_eq!(merchant["address"], bound["address"]);
        let events = webhook_events(&pool, &id).await;
        assert_eq!(
            events
                .iter()
                .map(|(kind, _)| kind.as_str())
                .collect::<Vec<_>>(),
            ["verification.approved", "deposit_request.ready"]
        );
        let approved = &events[0].1;
        assert_eq!(approved["type"], "verification.approved");
        // The webhook names the request exactly as the API does, so a handler
        // can hand the id straight back to GET /v1/deposit-requests/{id}.
        assert_eq!(approved["data"]["deposit_request"]["id"], id);
        assert_eq!(
            approved["data"]["deposit_request"]["id"],
            format!("dr_{uuid}")
        );
        assert_eq!(
            approved["data"]["deposit_request"]["payer_policy_mode"],
            "merchant_session"
        );
        assert_eq!(
            approved["data"]["deposit_request"]["payer_reference"],
            "user_123"
        );
        assert_eq!(approved["data"]["deposit_request"]["reference"], "dep-8042");
        assert_eq!(
            approved["data"]["deposit_request"]["metadata"]["order"],
            "8042"
        );
        assert!(approved["data"]["deposit_request"]["verification_completed_at"].is_string());
        assert_eq!(
            approved["data"]["deposit_request"]["status"],
            "awaiting_deposit"
        );

        let activity = json_body(
            app.clone()
                .oneshot(get_request(
                    KEY,
                    &format!("/v1/deposit-requests/{id}/verification"),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(activity["payer_policy_mode"], "merchant_session");
        assert_eq!(activity["facts"]["merchant_session"], "approved");
        assert_eq!(activity["facts"]["email"], "not_required");
        assert_eq!(activity["facts"]["complete"], true);
        assert_eq!(activity["facts"]["wallet"], "approved");
        let attempts = activity["attempts"].as_array().unwrap();
        assert_eq!(attempts.len(), 2);
        assert_eq!(attempts[0]["kind"], "merchant_session");
        assert_eq!(attempts[0]["status"], "approved");
        assert!(attempts[0]["verified_at"].is_string());
        assert_eq!(attempts[1]["kind"], "wallet");
        assert_eq!(attempts[1]["status"], "approved");
        assert!(!activity.to_string().contains("cs_"));
        assert!(!activity.to_string().contains("payer_session"));

        // 6. The payer comes back later from the app: the merchant mints a
        //    fresh secret, and the first, spent one is still spent.
        let minted = app
            .clone()
            .oneshot(json_request(
                "POST",
                KEY,
                &format!("/v1/deposit-requests/{id}/client-secret"),
                &json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(minted.status(), StatusCode::CREATED);
        assert_eq!(minted.headers()[header::CACHE_CONTROL], "no-store");
        let minted = json_body(minted).await;
        let second_secret = minted["client_secret"].as_str().unwrap().to_owned();
        assert_ne!(second_secret, secret);
        assert!(minted["expires_at"].is_string());
        let reopened = json_body(
            app.clone()
                .oneshot(exchange_request(&id, &second_secret))
                .await
                .unwrap(),
        )
        .await;
        let second_session = reopened["payer_session"].as_str().unwrap().to_owned();
        assert_ne!(second_session, session);
        assert_eq!(
            payer_read(&app, &id, Some(&second_session)).await["content_unlocked"],
            true
        );
        assert_eq!(
            app.clone()
                .oneshot(exchange_request(&id, &secret))
                .await
                .unwrap()
                .status(),
            StatusCode::CONFLICT
        );
        // Verification completed once; a second exchange adds an attempt,
        // not a second completion event.
        assert_eq!(webhook_events(&pool, &id).await.len(), 2);

        // Only the owner mints, and only for this mode.
        const OTHER: &str = "payday_live_other0123456789abcdef0123456789abcdef";
        other_account(&pool, OTHER).await;
        let foreign = app
            .clone()
            .oneshot(json_request(
                "POST",
                OTHER,
                &format!("/v1/deposit-requests/{id}/client-secret"),
                &json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::NOT_FOUND);
        let open_id = json_body(
            app.clone()
                .oneshot(create_request(KEY, "ms-open", &valid_body()))
                .await
                .unwrap(),
        )
        .await["id"]
            .as_str()
            .unwrap()
            .to_owned();
        let not_required = app
            .clone()
            .oneshot(json_request(
                "POST",
                KEY,
                &format!("/v1/deposit-requests/{open_id}/client-secret"),
                &json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(not_required.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(not_required).await["error"]["code"],
            "verification_not_required"
        );
        let (email_id, _) = create_gated(
            &app,
            "ms-email",
            json!({"mode": "verified_email", "expected_email": "alice@example.com"}),
            "Email retainer",
        )
        .await;
        let wrong_mode = app
            .clone()
            .oneshot(json_request(
                "POST",
                KEY,
                &format!("/v1/deposit-requests/{email_id}/client-secret"),
                &json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(wrong_mode.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(wrong_mode).await["error"]["code"],
            "verification_method_not_applicable"
        );
        let email_exchange = app
            .clone()
            .oneshot(exchange_request(&email_id, &second_secret))
            .await
            .unwrap();
        assert_eq!(email_exchange.status(), StatusCode::CONFLICT);
        assert_eq!(
            json_body(email_exchange).await["error"]["code"],
            "verification_method_not_applicable"
        );

        // 7. The payer pays from the checkout. Funding and settlement raise
        //    deposit_request.deposited and deposit_request.settled, each carrying the payer
        //    reference the merchant's ledger credits by.
        let address = Address::parse_checksummed(bound["address"].as_str().unwrap(), None).unwrap();
        sqlx::query(
            r#"INSERT INTO payment_observations
                 (chain_id, token_address, block_number, block_hash, block_timestamp,
                  transaction_hash, transaction_index, log_index, sender_address,
                  recipient_address, invoice_id, amount, disposition)
               VALUES (1, $1, 3, $2, 1800000000, $3, 0, 0, $4, $5, $6, '1000000', 'credited')"#,
        )
        .bind(Address::ZERO.as_slice())
        .bind([1u8; 32].as_slice())
        .bind([3u8; 32].as_slice())
        .bind(gateway_core::wallet_of(&PAYER_KEY).as_slice())
        .bind(address.as_slice())
        .bind(uuid)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "UPDATE invoices SET status = 'funded', confirmed_received = '1000000', funded_at_block = 3, paid_at = now() WHERE id = $1",
        )
        .bind(uuid)
        .execute(&pool)
        .await
        .unwrap();
        let paid = json_body(
            app.clone()
                .oneshot(get_request(KEY, &format!("/v1/deposit-requests/{id}")))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(paid["status"], "deposited");
        assert!(
            paid["likely_unsolicited_at"].is_null(),
            "verified before funding"
        );
        let settlement = [9u8; 32];
        sqlx::query(
            "UPDATE invoices SET status = 'fulfilled', settlement_tx_hash = $2, resolved_at_block = 9, settled_at = now() WHERE id = $1",
        )
        .bind(uuid)
        .bind(settlement.as_slice())
        .execute(&pool)
        .await
        .unwrap();
        let events = webhook_events(&pool, &id).await;
        assert_eq!(
            events
                .iter()
                .map(|(kind, _)| kind.as_str())
                .collect::<Vec<_>>(),
            [
                "verification.approved",
                "deposit_request.ready",
                "deposit_request.deposited",
                "deposit_request.settled"
            ]
        );
        for (kind, payload) in &events {
            let payment = &payload["data"]["deposit_request"];
            assert_eq!(payment["payer_reference"], "user_123", "{kind}");
            assert_eq!(payment["payer_policy_mode"], "merchant_session", "{kind}");
            assert_eq!(payment["metadata"]["order"], "8042", "{kind}");
            assert!(payment["verification_completed_at"].is_string(), "{kind}");
            assert!(payment["likely_unsolicited_at"].is_null(), "{kind}");
            // The payload names the merchant's own reference, never a secret
            // or a session.
            let serialized = payload.to_string();
            assert!(!serialized.contains("cs_"), "{kind}: {serialized}");
            assert!(!serialized.contains(&session), "{kind}");
        }
        assert_eq!(
            events[2].1["data"]["deposit_request"]["status"],
            "deposited"
        );
        // Amounts carry the API's units and names: the human decimal under
        // `received`, the integer under `received_base_units`.
        assert_eq!(
            events[2].1["data"]["deposit_request"]["received"],
            "1.000000"
        );
        assert_eq!(
            events[2].1["data"]["deposit_request"]["received_base_units"],
            "1000000"
        );
        assert_eq!(events[2].1["data"]["deposit_request"]["amount"], "1.000000");
        assert_eq!(events[3].1["data"]["deposit_request"]["status"], "settled");

        // 8. The proof attests the merchant session; the snapshot carries
        //    the merchant's assertion; it verifies offline.
        let proof: ProofOfPayment = serde_json::from_value(
            json_body(
                app.clone()
                    .oneshot(get_request(
                        KEY,
                        &format!("/v1/deposit-requests/{id}/proof"),
                    ))
                    .await
                    .unwrap(),
            )
            .await,
        )
        .unwrap();
        assert_eq!(
            proof.verification.payload.payer_policy_mode,
            "merchant_session"
        );
        assert_eq!(proof.verification.payload.result, "approved");
        assert!(proof.verification.payload.verified_at.is_some());
        assert_eq!(
            proof.canonical_issuance_snapshot.payer_policy,
            gateway_core::PayerPolicy::MerchantSession {
                payer_reference: "user_123".into()
            }
        );
        let verified = verify_proof(&proof, None, &[attestor().address()])
            .unwrap_or_else(|error| panic!("the served proof must verify offline: {error}"));
        assert_eq!(verified.payment_address, address);

        // 9. Sessions expire; the app can still open the receipt with a fresh
        //    secret, which never revives the settled invoice.
        sqlx::query("UPDATE payer_sessions SET created_at = now() - interval '25 hours', expires_at = now() - interval '1 hour' WHERE invoice_id = $1")
            .bind(uuid)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(
            payer_read(&app, &id, Some(&second_session)).await["content_unlocked"],
            false
        );
        let receipt_secret = json_body(
            app.clone()
                .oneshot(json_request(
                    "POST",
                    KEY,
                    &format!("/v1/deposit-requests/{id}/client-secret"),
                    &json!({}),
                ))
                .await
                .unwrap(),
        )
        .await["client_secret"]
            .as_str()
            .unwrap()
            .to_owned();
        let receipt = json_body(
            app.clone()
                .oneshot(exchange_request(&id, &receipt_secret))
                .await
                .unwrap(),
        )
        .await;
        let receipt_session = receipt["payer_session"].as_str().unwrap();
        let receipt_view = payer_read(&app, &id, Some(receipt_session)).await;
        assert_eq!(receipt_view["content_unlocked"], true);
        assert_eq!(receipt_view["status"], "settled");
        assert_eq!(receipt_view["payable"], false);
        assert!(receipt_view["deposit_uri"].is_null());
        assert_eq!(webhook_events(&pool, &id).await.len(), 4);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn merchant_sessions_refuse_bad_references_stale_secrets_and_closed_payments(
        pool: PgPool,
    ) {
        let (app, tenant) = app_with_payer_verification(pool.clone()).await;

        // The assertion is required, bounded, and one token. A body that does
        // not fit the policy's shape and one that fails its rules answer the
        // same way: 400 invalid_request.
        for (name, policy, status) in [
            (
                "missing reference",
                json!({"mode": "merchant_session"}),
                StatusCode::BAD_REQUEST,
            ),
            (
                "email on merchant session",
                json!({"mode": "merchant_session", "payer_reference": "u1", "expected_email": "a@b.co"}),
                StatusCode::BAD_REQUEST,
            ),
            (
                "reference on permissionless",
                json!({"mode": "permissionless", "payer_reference": "u1"}),
                StatusCode::BAD_REQUEST,
            ),
            (
                "blank reference",
                json!({"mode": "merchant_session", "payer_reference": "   "}),
                StatusCode::BAD_REQUEST,
            ),
            (
                "spaced reference",
                json!({"mode": "merchant_session", "payer_reference": "user 123"}),
                StatusCode::BAD_REQUEST,
            ),
            (
                "long reference",
                json!({"mode": "merchant_session", "payer_reference": "x".repeat(129)}),
                StatusCode::BAD_REQUEST,
            ),
        ] {
            let mut body = valid_body();
            body["payer_policy"] = policy;
            let response = app
                .clone()
                .oneshot(create_request(KEY, name, &body))
                .await
                .unwrap();
            assert_eq!(response.status(), status, "{name}");
        }
        // Trimmed, case kept.
        let mut body = valid_body();
        body["payer_policy"] =
            json!({"mode": "merchant_session", "payer_reference": "  User_ABC "});
        let created = json_body(
            app.clone()
                .oneshot(create_request(KEY, "trimmed", &body))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(created["payer_policy"]["payer_reference"], "User_ABC");
        let id = created["id"].as_str().unwrap().to_owned();
        let uuid = Uuid::parse_str(id.strip_prefix("dr_").unwrap()).unwrap();
        let secret = created["client_secret"].as_str().unwrap().to_owned();

        // Email codes are not this mode's method, whichever way they are asked for.
        for path in ["verify/email/start", "verify/email/confirm"] {
            let body = (path == "verify/email/confirm").then(|| json!({"otp": OTP}));
            let refused = app
                .clone()
                .oneshot(payer_post(
                    &format!("/v1/payer/deposit-requests/{id}/{path}"),
                    None,
                    body.as_ref(),
                ))
                .await
                .unwrap();
            assert_eq!(refused.status(), StatusCode::CONFLICT, "{path}");
            assert_eq!(
                json_body(refused).await["error"]["code"],
                "verification_method_not_applicable",
                "{path}"
            );
        }
        assert!(tenant.started.lock().unwrap().is_empty());

        // A secret past its fifteen minutes is refused like an unknown one.
        sqlx::query("UPDATE payer_client_secrets SET created_at = now() - interval '20 minutes', expires_at = now() - interval '5 minutes' WHERE invoice_id = $1")
            .bind(uuid)
            .execute(&pool)
            .await
            .unwrap();
        let stale = app
            .clone()
            .oneshot(exchange_request(&id, &secret))
            .await
            .unwrap();
        assert_eq!(stale.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(stale).await["error"]["code"],
            "client_secret_invalid"
        );
        assert!(
            payer_read(&app, &id, None).await["requirements"]["complete"] == false,
            "an expired secret verifies nothing"
        );

        // A closed, never-verified payment can neither mint nor exchange.
        sqlx::query("UPDATE invoices SET status = 'expired', expired_at = now() WHERE id = $1")
            .bind(uuid)
            .execute(&pool)
            .await
            .unwrap();
        let fresh = json_body(
            app.clone()
                .oneshot(create_request(
                    KEY,
                    "fresh",
                    &merchant_session_body("user_7"),
                ))
                .await
                .unwrap(),
        )
        .await;
        let fresh_secret = fresh["client_secret"].as_str().unwrap();
        let too_late_mint = app
            .clone()
            .oneshot(json_request(
                "POST",
                KEY,
                &format!("/v1/deposit-requests/{id}/client-secret"),
                &json!({}),
            ))
            .await
            .unwrap();
        assert_eq!(too_late_mint.status(), StatusCode::GONE);
        let too_late_exchange = app
            .clone()
            .oneshot(exchange_request(&id, fresh_secret))
            .await
            .unwrap();
        assert_eq!(too_late_exchange.status(), StatusCode::GONE);
        assert_eq!(
            json_body(too_late_exchange).await["error"]["code"],
            "deposit_request_not_payable"
        );
        let unknown = app
            .clone()
            .oneshot(exchange_request("dr_not-an-id", fresh_secret))
            .await
            .unwrap();
        assert_eq!(unknown.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            json_body(unknown).await["error"]["code"],
            "invalid_deposit_link"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn payer_writes_answer_cors_for_the_hosted_checkout_only(pool: PgPool) {
        let (app, _) = app_with_payer_verification(pool).await;
        let path = "/v1/payer/deposit-requests/dr_x/verify/email/start";
        // The client-secret exchange is a verification write like the email
        // routes, with the same one-origin answer.
        let session_preflight = app
            .clone()
            .oneshot(
                Request::options("/v1/payer/deposit-requests/dr_x/session")
                    .header(header::ORIGIN, CHECKOUT_ORIGIN)
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                    .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            session_preflight.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            CHECKOUT_ORIGIN
        );
        let foreign_session = app
            .clone()
            .oneshot(
                Request::options("/v1/payer/deposit-requests/dr_x/session")
                    .header(header::ORIGIN, "https://evil.example")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            foreign_session
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );
        let preflight = |origin: &str| {
            Request::options(path)
                .header(header::ORIGIN, origin)
                .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                .header(
                    header::ACCESS_CONTROL_REQUEST_HEADERS,
                    "content-type, payday-payer-session",
                )
                .body(Body::empty())
                .unwrap()
        };
        let allowed = app
            .clone()
            .oneshot(preflight(CHECKOUT_ORIGIN))
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::OK);
        let headers = allowed.headers();
        assert_eq!(
            headers[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            CHECKOUT_ORIGIN
        );
        let methods = headers[header::ACCESS_CONTROL_ALLOW_METHODS]
            .to_str()
            .unwrap()
            .to_owned();
        assert!(methods.contains("POST"), "{methods}");
        assert!(!methods.contains("GET"), "{methods}");
        let allowed_headers = headers[header::ACCESS_CONTROL_ALLOW_HEADERS]
            .to_str()
            .unwrap()
            .to_ascii_lowercase();
        for name in ["content-type", "payday-payer-session"] {
            assert!(allowed_headers.contains(name), "{allowed_headers}");
        }
        assert!(
            !allowed_headers.contains("authorization"),
            "{allowed_headers}"
        );

        // Foreign origins get no allow header at all: never `*`, never an
        // echo.
        for origin in ["https://evil.example", "http://127.0.0.1:3001", "null"] {
            let foreign = app.clone().oneshot(preflight(origin)).await.unwrap();
            assert!(
                foreign
                    .headers()
                    .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                    .is_none(),
                "{origin} was allowed: {:?} {:?}",
                foreign.status(),
                foreign.headers()
            );
        }
        let foreign_post = app
            .clone()
            .oneshot(
                Request::post(path)
                    .header(header::ORIGIN, "https://evil.example")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(
            foreign_post
                .headers()
                .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
                .is_none()
        );

        // The reads stay open to every origin and now admit the session
        // header, so a self-hosted checkout can present it.
        let read_preflight = app
            .oneshot(
                Request::options("/v1/payer/deposit-requests/dr_x")
                    .header(header::ORIGIN, "https://merchant.example")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "GET")
                    .header(
                        header::ACCESS_CONTROL_REQUEST_HEADERS,
                        "payday-payer-session",
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(read_preflight.status(), StatusCode::OK);
        assert_eq!(
            read_preflight.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "*"
        );
        assert!(
            read_preflight.headers()[header::ACCESS_CONTROL_ALLOW_HEADERS]
                .to_str()
                .unwrap()
                .to_ascii_lowercase()
                .contains("payday-payer-session")
        );
    }

    fn pregenerate_request(email: &str) -> Request<Body> {
        Request::post("/v1/wallets/pregenerate")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({ "email": email }).to_string()))
            .unwrap()
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn pregenerating_a_wallet_asks_the_configured_provider_a_normalized_email(pool: PgPool) {
        let (app, pregenerator) = app_with_pregeneration(pool).await;
        let response = app
            .oneshot(pregenerate_request("  Founder@Example.com  "))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(
            *pregenerator.asked.lock().unwrap(),
            vec!["founder@example.com".to_string()]
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn pregenerating_a_wallet_rejects_an_invalid_email(pool: PgPool) {
        let (app, pregenerator) = app_with_pregeneration(pool).await;
        let response = app
            .oneshot(pregenerate_request("not-an-email"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(pregenerator.asked.lock().unwrap().is_empty());
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn pregenerating_a_wallet_is_unavailable_without_a_configured_provider(pool: PgPool) {
        let app = app(pool).await;
        let response = app
            .oneshot(pregenerate_request("founder@example.com"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(
            json_body(response).await["error"]["code"],
            "wallet_pregeneration_unavailable"
        );
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn pregenerating_a_wallet_reports_an_upstream_outage(pool: PgPool) {
        let (app, pregenerator) = app_with_pregeneration(pool).await;
        *pregenerator.outage.lock().unwrap() = true;
        let response = app
            .oneshot(pregenerate_request("founder@example.com"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn pregenerating_a_wallet_is_cooled_down_per_email(pool: PgPool) {
        let (app, pregenerator) = app_with_pregeneration(pool).await;
        let first = app
            .clone()
            .oneshot(pregenerate_request("founder@example.com"))
            .await
            .unwrap();
        assert_eq!(first.status(), StatusCode::NO_CONTENT);
        // A resend, or a second tab, asking again right away must not spend
        // a second call against Privy for the same mailbox.
        let second = app
            .oneshot(pregenerate_request("founder@example.com"))
            .await
            .unwrap();
        assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(pregenerator.asked.lock().unwrap().len(), 1);
    }

    fn preview_session_request(key: &str, id: &str) -> Request<Body> {
        json_request(
            "POST",
            key,
            &format!("/v1/deposit-requests/{id}/preview-session"),
            &json!({}),
        )
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn preview_session_unlocks_the_payer_view_for_the_issuing_merchant_without_verifying_it(
        pool: PgPool,
    ) {
        let app = app(pool).await;
        let (id, _) = create_gated(
            &app,
            "preview-email",
            json!({"mode": "verified_email", "expected_email": "alice@example.com"}),
            "Preview retainer",
        )
        .await;

        // A stranger holding the link sees the locked view.
        assert_eq!(payer_read(&app, &id, None).await["content_unlocked"], false);

        let minted = app
            .clone()
            .oneshot(preview_session_request(KEY, &id))
            .await
            .unwrap();
        assert_eq!(minted.status(), StatusCode::OK);
        let minted = json_body(minted).await;
        let session = minted["payer_session"].as_str().unwrap().to_owned();
        assert!(minted["expires_at"].is_string());

        // The issuing merchant, holding the same link, now sees it unlocked.
        assert_eq!(
            payer_read(&app, &id, Some(&session)).await["content_unlocked"],
            true
        );

        // Previewing is not verifying: the invoice's own completion, and the
        // merchant's own verification activity log, are both untouched.
        let activity = json_body(
            app.clone()
                .oneshot(get_request(
                    KEY,
                    &format!("/v1/deposit-requests/{id}/verification"),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(activity["facts"]["complete"], false);
        assert!(activity["attempts"].as_array().unwrap().is_empty());
    }

    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn preview_session_is_scoped_to_the_owning_merchant_and_a_payable_request(pool: PgPool) {
        let app = app(pool.clone()).await;
        let (id, _) = create_gated(
            &app,
            "preview-owner",
            json!({"mode": "verified_email", "expected_email": "alice@example.com"}),
            "Owner retainer",
        )
        .await;

        const OTHER: &str = "payday_live_previewother0123456789abcdef012345";
        other_account(&pool, OTHER).await;
        let foreign = app
            .clone()
            .oneshot(preview_session_request(OTHER, &id))
            .await
            .unwrap();
        assert_eq!(foreign.status(), StatusCode::NOT_FOUND);

        // Cancellation is advisory only (see cancel_deposit_request) and does
        // not affect payability; a real terminal, never-verified state does.
        let uuid = gateway_core::deposit_request_id(&id).unwrap();
        sqlx::query("UPDATE invoices SET status = 'expired' WHERE id = $1")
            .bind(uuid)
            .execute(&pool)
            .await
            .unwrap();
        let refused = app
            .clone()
            .oneshot(preview_session_request(KEY, &id))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::GONE);
        assert_eq!(
            json_body(refused).await["error"]["code"],
            "deposit_request_not_payable"
        );
    }

    /// Webhook endpoints follow the same conventions as every other merchant
    /// resource: enveloped lists, `404 webhook_not_found` for a missing,
    /// malformed, or foreign id, `204` on disable, and paged deliveries.
    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn webhook_routes_follow_the_resource_conventions(pool: PgPool) {
        let app = app(pool.clone()).await;
        let bad_url = app
            .clone()
            .oneshot(json_request(
                "POST",
                KEY,
                "/v1/webhooks",
                &json!({"url": "http://8.8.8.8/hook"}),
            ))
            .await
            .unwrap();
        assert_eq!(bad_url.status(), StatusCode::BAD_REQUEST);
        assert_eq!(json_body(bad_url).await["error"]["code"], "invalid_request");

        // Registering resolves the hostname, which a unit test has no
        // network for, so the endpoint is written the way the route would
        // write it and the reads are exercised from there.
        let account = test_account(&pool).await;
        let endpoint = gateway_db::WebhookRepository::new(pool.clone())
            .add(
                account,
                "https://hooks.example/payday",
                b"whsec_test",
                &[1u8; 28],
            )
            .await
            .unwrap();
        let id = WebhookId(endpoint.id).to_string();
        // A bare UUID is not an id the API knows.
        let bare = app
            .clone()
            .oneshot(get_request(KEY, &format!("/v1/webhooks/{}", endpoint.id)))
            .await
            .unwrap();
        assert_eq!(bare.status(), StatusCode::NOT_FOUND);

        let listed = json_body(
            app.clone()
                .oneshot(get_request(KEY, "/v1/webhooks"))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(listed["webhooks"][0]["id"], id);
        assert!(listed["webhooks"][0].get("secret").is_none());
        assert!(listed["webhooks"][0]["disabled_at"].is_null());
        let created_at = listed["webhooks"][0]["created_at"].as_str().unwrap();
        assert!(
            created_at.len() == 20 && created_at.ends_with('Z'),
            "{created_at}"
        );

        let fetched = json_body(
            app.clone()
                .oneshot(get_request(KEY, &format!("/v1/webhooks/{id}")))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(fetched["url"], "https://hooks.example/payday");
        assert!(fetched.get("secret").is_none());

        // A test event is one delivery, listed with its (so far empty) history.
        let test = app
            .clone()
            .oneshot(json_request(
                "POST",
                KEY,
                &format!("/v1/webhooks/{id}/test"),
                &json!(null),
            ))
            .await
            .unwrap();
        assert_eq!(test.status(), StatusCode::ACCEPTED);
        let delivery_id = json_body(test).await["delivery_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let deliveries = json_body(
            app.clone()
                .oneshot(get_request(
                    KEY,
                    &format!("/v1/webhook-deliveries?endpoint_id={id}&limit=1"),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(deliveries["deliveries"][0]["id"], delivery_id);
        assert_eq!(deliveries["deliveries"][0]["endpoint_id"], id);
        assert_eq!(deliveries["deliveries"][0]["state"], "pending");
        assert_eq!(deliveries["deliveries"][0]["attempts"], json!([]));
        assert!(deliveries["next_cursor"].is_null());
        for query in [
            "limit=0".to_owned(),
            format!("starting_after={}", Uuid::now_v7()),
            "sort=asc".to_owned(),
        ] {
            let response = app
                .clone()
                .oneshot(get_request(KEY, &format!("/v1/webhook-deliveries?{query}")))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{query}");
        }

        // Missing, malformed, and foreign ids all read as not found.
        const OTHER: &str = "payday_live_webhookother0123456789abcdef012345";
        other_account(&pool, OTHER).await;
        for (key, path) in [
            (KEY, format!("/v1/webhooks/{}", WebhookId::generate())),
            (KEY, format!("/v1/webhooks/{}", Uuid::now_v7())),
            (KEY, "/v1/webhooks/not-a-uuid".to_owned()),
            (OTHER, format!("/v1/webhooks/{id}")),
        ] {
            for method in ["GET", "DELETE"] {
                let response = app
                    .clone()
                    .oneshot(json_request(method, key, &path, &json!(null)))
                    .await
                    .unwrap();
                assert_eq!(response.status(), StatusCode::NOT_FOUND, "{method} {path}");
                assert_eq!(
                    json_body(response).await["error"]["code"],
                    "webhook_not_found",
                    "{method} {path}"
                );
            }
            let test = app
                .clone()
                .oneshot(json_request(
                    "POST",
                    key,
                    &format!("{path}/test"),
                    &json!(null),
                ))
                .await
                .unwrap();
            assert_eq!(test.status(), StatusCode::NOT_FOUND, "{path}");
        }
        let foreign_deliveries = app
            .clone()
            .oneshot(get_request(
                OTHER,
                &format!("/v1/webhook-deliveries?endpoint_id={id}"),
            ))
            .await
            .unwrap();
        assert_eq!(foreign_deliveries.status(), StatusCode::NOT_FOUND);

        // Disabling answers 204, is idempotent, drops the endpoint from the
        // list, refuses further test events, and keeps the history readable.
        for _ in 0..2 {
            let disabled = app
                .clone()
                .oneshot(json_request(
                    "DELETE",
                    KEY,
                    &format!("/v1/webhooks/{id}"),
                    &json!(null),
                ))
                .await
                .unwrap();
            assert_eq!(disabled.status(), StatusCode::NO_CONTENT);
        }
        let listed = json_body(
            app.clone()
                .oneshot(get_request(KEY, "/v1/webhooks"))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(listed["webhooks"], json!([]));
        let fetched = json_body(
            app.clone()
                .oneshot(get_request(KEY, &format!("/v1/webhooks/{id}")))
                .await
                .unwrap(),
        )
        .await;
        assert!(fetched["disabled_at"].is_string());
        let refused = app
            .clone()
            .oneshot(json_request(
                "POST",
                KEY,
                &format!("/v1/webhooks/{id}/test"),
                &json!(null),
            ))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::NOT_FOUND);
        let history = json_body(
            app.clone()
                .oneshot(get_request(KEY, "/v1/webhook-deliveries"))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(history["deliveries"][0]["id"], delivery_id);
    }

    /// A request may name saved records instead of retyping them: the
    /// issuer identity supplies the issuer party and, with a saved payout
    /// address, the payout address; the customer supplies the payer. What is
    /// issued is the same snapshot an inline request would have produced.
    #[sqlx::test(migrator = "gateway_db::MIGRATOR")]
    async fn create_takes_parties_and_payout_address_from_saved_records(pool: PgPool) {
        let app = app(pool.clone()).await;
        let customer = json_body(
            app.clone()
                .oneshot(json_request(
                    "POST",
                    KEY,
                    "/v1/customers",
                    &json!({"name": "Globex", "email": "ap@globex.example", "details": "Net 30"}),
                ))
                .await
                .unwrap(),
        )
        .await;
        let issuer = json_body(
            app.clone()
                .oneshot(json_request(
                    "POST",
                    KEY,
                    "/v1/issuers",
                    &json!({"name": "Acme", "contact_email": "billing@acme.example", "details": "1 Main St"}),
                ))
                .await
                .unwrap(),
        )
        .await;
        let issuer_id = issuer["id"].as_str().unwrap().to_owned();
        let mut body = json!({
            "amount": "1",
            "issuer_id": issuer_id,
            "customer_id": customer["id"],
            "payer_policy": {"mode": "permissionless"}
        });

        // Without a saved payout address there is nowhere to settle.
        let refused = app
            .clone()
            .oneshot(create_request(KEY, "saved-no-address", &body))
            .await
            .unwrap();
        assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
        assert!(
            json_body(refused).await["error"]["message"]
                .as_str()
                .unwrap()
                .contains("payout_address")
        );

        let address = json_body(
            app.clone()
                .oneshot(json_request(
                    "POST",
                    KEY,
                    "/v1/payout-addresses",
                    &json!({"address": "0x0000000000000000000000000000000000000002", "label": "Ops"}),
                ))
                .await
                .unwrap(),
        )
        .await;
        app.clone()
            .oneshot(json_request(
                "PUT",
                KEY,
                &format!("/v1/issuers/{issuer_id}/payout-addresses"),
                &json!({"payout_address_ids": [address["id"]]}),
            ))
            .await
            .unwrap();

        let issued = app
            .clone()
            .oneshot(create_request(KEY, "saved-records", &body))
            .await
            .unwrap();
        assert_eq!(issued.status(), StatusCode::CREATED);
        let issued = json_body(issued).await;
        assert_eq!(
            issued["issuer"],
            json!({"name": "Acme", "email": "billing@acme.example", "details": "1 Main St"})
        );
        assert_eq!(
            issued["payer"],
            json!({"name": "Globex", "email": "ap@globex.example", "details": "Net 30"})
        );
        assert_eq!(
            issued["payout_address"],
            "0x0000000000000000000000000000000000000002"
        );
        assert_eq!(issued["issuer_id"], issuer_id);
        assert_eq!(issued["customer_id"], customer["id"]);

        // An inline party still wins over the saved record it sits beside.
        body["payer"] = json!({"name": "Globex Holdings"});
        let inline = json_body(
            app.clone()
                .oneshot(create_request(KEY, "saved-records-inline", &body))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(inline["payer"]["name"], "Globex Holdings");
        assert!(inline["payer"].get("email").is_none());

        // With neither an inline party nor a record to take it from, the
        // request says which field is missing.
        let bare = app
            .clone()
            .oneshot(create_request(
                KEY,
                "saved-records-bare",
                &json!({"amount": "1", "payer_policy": {"mode": "permissionless"},
                        "payout_address": "0x0000000000000000000000000000000000000002"}),
            ))
            .await
            .unwrap();
        assert_eq!(bare.status(), StatusCode::BAD_REQUEST);
        assert!(
            json_body(bare).await["error"]["message"]
                .as_str()
                .unwrap()
                .starts_with("issuer is required")
        );
    }
}
