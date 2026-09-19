use std::collections::HashMap;
use std::time::Duration;

use alloy_primitives::Address;
use gum_core::ChainRegistry;

/// Signed download links live this long unless configured otherwise.
const DEFAULT_DOWNLOAD_TTL_SECS: u64 = 300;
/// SigV4 presigned URLs cannot outlive a week.
const MAX_PRESIGN_SECS: u64 = 7 * 24 * 3600;

pub struct Config {
    bind_addr: String,
    database_url: String,
    /// The Privy app merchants sign in to; `None` leaves dashboard sessions
    /// unaccepted and only API keys authenticate.
    privy: Option<PrivyConfig>,
    dev_identity: bool,
    /// Every network a deposit request may be paid on, with the contract
    /// generation deployed on each (`GUM_CHAINS`).
    networks: ChainRegistry,
    /// One RPC URL per registered chain (`GUM_RPC_URL_<chain_id>`), read
    /// at startup to verify the deployment.
    rpc_urls: HashMap<u64, String>,
    attachments: AttachmentConfig,
    attestation: AttestationSignerConfig,
    /// The payer audience and the payer-reference key; both `None` leaves
    /// the email verification routes unavailable.
    payer_verification: Option<PayerVerificationConfig>,
    public_base_url: String,
    /// The one browser origin that may call the verification write routes.
    hosted_checkout_origin: Option<String>,
    api_key_prefix: String,
    webhook_encryption_key: Option<[u8; 32]>,
    notification_from_address: Option<String>,
    /// The Resend key that sends payers their deposit request emails;
    /// `None` leaves those queued and unsent.
    resend: Option<ResendConfig>,
    /// Relay (`GUM_RELAY_URL`), for cross-chain payments into a deposit
    /// address; `GUM_RELAY_API_KEY` unset leaves them unoffered.
    relay_url: String,
    relay_api_key: Option<String>,
    /// The listener `gum-indexer` calls (`GUM_INTERNAL_BIND_ADDR`) and
    /// the bearer token it presents (`GUM_INTERNAL_TOKEN`). Never the
    /// public listener: these routes mutate the ledger unauthenticated by
    /// anything but the token.
    internal_bind_addr: String,
    internal_token: String,
    /// Circle's Iris API (`GUM_CCTP_IRIS_URL`), polled for the
    /// attestation of every bridge leg's burn. Mainnet by default.
    iris_url: String,
    /// Gum's own recovery wallet (`GUM_RECOVERY_ADDRESS`), the recovery
    /// term every payment contract commits to. Never a payer's wallet, and
    /// never optional: a deployment without it cannot issue.
    recovery_address: Address,
    /// How often the sweep scheduler looks at the queue when nothing woke it
    /// (`GUM_SWEEP_SCHEDULER_INTERVAL_MS`, 5000 by default).
    sweep_scheduler_interval: Duration,
}

/// Payer email goes out through Resend, the account Auth0 already sends
/// its codes from.
#[derive(Clone, Debug)]
pub struct ResendConfig {
    pub api_key: String,
    /// The `From` header, `Name <address>` or a bare address.
    pub from: String,
}

impl Config {
    pub fn from_env() -> Self {
        let dev_identity = std::env::var("GUM_DEV_IDENTITY").as_deref() == Ok("1");
        // Absent (or empty, as an unconfigured task might set it) means
        // dashboard sessions are not accepted at all.
        let privy_app_secret = std::env::var("GUM_PRIVY_APP_SECRET")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let privy = std::env::var("GUM_PRIVY_APP_ID")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .map(|app_id| PrivyConfig {
                app_id,
                app_secret: privy_app_secret.clone(),
            });
        if privy.is_none() && privy_app_secret.is_some() {
            panic!(
                "GUM_PRIVY_APP_SECRET is set but GUM_PRIVY_APP_ID is not; wallet pregeneration needs both"
            );
        }

        let payer_verification = match (
            std::env::var("GUM_PAYER_AUTH0_ISSUER").ok(),
            std::env::var("GUM_PAYER_AUTH0_AUDIENCE").ok(),
            std::env::var("GUM_PAYER_AUTH0_CLIENT_ID").ok(),
            std::env::var("GUM_PAYER_REF_MASTER_KEY").ok(),
        ) {
            (Some(issuer), Some(audience), Some(client_id), Some(master_key)) => {
                Some(PayerVerificationConfig {
                    issuer,
                    audience,
                    client_id,
                    payer_ref_master_key: decode_key_32("GUM_PAYER_REF_MASTER_KEY", &master_key)
                        .unwrap_or_else(|message| panic!("{message}")),
                })
            }
            (None, None, None, None) => None,
            _ => panic!(
                "GUM_PAYER_AUTH0_ISSUER, GUM_PAYER_AUTH0_AUDIENCE, GUM_PAYER_AUTH0_CLIENT_ID, and GUM_PAYER_REF_MASTER_KEY must be set together"
            ),
        };

        let networks = ChainRegistry::from_env();
        let required =
            |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set"));
        let rpc_urls = networks
            .chains()
            .iter()
            .map(|chain| (chain.chain_id, required(&chain.rpc_url_var())))
            .collect();

        let attachments = AttachmentConfig {
            bucket: required("GUM_ATTACHMENT_BUCKET"),
            s3_endpoint: std::env::var("GUM_ATTACHMENT_S3_ENDPOINT")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            force_path_style: std::env::var("GUM_ATTACHMENT_S3_FORCE_PATH_STYLE").as_deref()
                == Ok("1"),
            download_ttl: parse_download_ttl(
                std::env::var("GUM_ATTACHMENT_DOWNLOAD_TTL_SECS")
                    .ok()
                    .as_deref(),
            )
            .unwrap_or_else(|message| panic!("{message}")),
        };
        let attestation = attestation_signer(
            std::env::var("GUM_ATTESTATION_SIGNER_KEY").ok(),
            std::env::var("GUM_ATTESTATION_KMS_KEY_ID").ok(),
        )
        .unwrap_or_else(|message| panic!("{message}"));

        let api_key_prefix =
            std::env::var("GUM_API_KEY_PREFIX").unwrap_or_else(|_| "gum_live_".into());
        validate_api_key_prefix(&api_key_prefix).unwrap_or_else(|message| panic!("{message}"));
        let webhook_encryption_key =
            std::env::var("GUM_WEBHOOK_ENCRYPTION_KEY")
                .ok()
                .map(|value| {
                    decode_key_32("GUM_WEBHOOK_ENCRYPTION_KEY", &value)
                        .unwrap_or_else(|message| panic!("{message}"))
                });

        let resend = std::env::var("GUM_RESEND_API_KEY")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .map(|api_key| ResendConfig {
                api_key,
                from: payer_email_from(std::env::var("GUM_PAYER_EMAIL_FROM").ok())
                    .unwrap_or_else(|message| panic!("{message}")),
            });

        let recovery_address = recovery_address(required("GUM_RECOVERY_ADDRESS").trim())
            .unwrap_or_else(|message| panic!("{message}"));

        Config {
            bind_addr: std::env::var("GUM_BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:3000".into()),
            database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            privy,
            dev_identity,
            networks,
            rpc_urls,
            attachments,
            attestation,
            payer_verification,
            public_base_url: std::env::var("GUM_PUBLIC_BASE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:3000".into()),
            hosted_checkout_origin: std::env::var("GUM_HOSTED_CHECKOUT_ORIGIN")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            api_key_prefix,
            webhook_encryption_key,
            notification_from_address: std::env::var("GUM_NOTIFICATION_FROM_ADDRESS").ok(),
            resend,
            relay_url: std::env::var("GUM_RELAY_URL")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| gum_relay::DEFAULT_RELAY_URL.to_owned()),
            relay_api_key: std::env::var("GUM_RELAY_API_KEY")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty()),
            internal_bind_addr: std::env::var("GUM_INTERNAL_BIND_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:3010".into()),
            internal_token: required("GUM_INTERNAL_TOKEN"),
            iris_url: std::env::var("GUM_CCTP_IRIS_URL")
                .ok()
                .map(|value| value.trim().to_owned())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| crate::iris::DEFAULT_IRIS_URL.to_owned()),
            recovery_address,
            sweep_scheduler_interval: Duration::from_millis(
                std::env::var("GUM_SWEEP_SCHEDULER_INTERVAL_MS")
                    .ok()
                    .map(|value| {
                        value.parse::<u64>().unwrap_or_else(|_| {
                            panic!("GUM_SWEEP_SCHEDULER_INTERVAL_MS must be a positive integer")
                        })
                    })
                    .unwrap_or(5_000)
                    .max(100),
            ),
        }
    }

    pub fn internal_bind_addr(&self) -> &str {
        &self.internal_bind_addr
    }

    pub fn internal_token(&self) -> &str {
        &self.internal_token
    }

    pub fn iris_url(&self) -> &str {
        &self.iris_url
    }

    /// Gum's own recovery wallet, the recovery term every payment contract
    /// commits to.
    pub fn recovery_address(&self) -> Address {
        self.recovery_address
    }

    pub fn sweep_scheduler_interval(&self) -> Duration {
        self.sweep_scheduler_interval
    }

    pub fn relay_url(&self) -> &str {
        &self.relay_url
    }

    pub fn relay_api_key(&self) -> Option<&str> {
        self.relay_api_key.as_deref()
    }

    pub fn bind_addr(&self) -> &str {
        &self.bind_addr
    }

    pub fn database_url(&self) -> &str {
        &self.database_url
    }

    /// The Privy app whose identity tokens are dashboard sessions; absent
    /// when the deployment accepts API keys only.
    pub fn privy(&self) -> Option<&PrivyConfig> {
        self.privy.as_ref()
    }

    pub fn dev_identity(&self) -> bool {
        self.dev_identity
    }

    /// The networks this service issues addresses against.
    pub fn networks(&self) -> &ChainRegistry {
        &self.networks
    }

    /// The RPC URL for one registered chain; every registered chain has one.
    pub fn rpc_url(&self, chain_id: u64) -> &str {
        self.rpc_urls
            .get(&chain_id)
            .map(String::as_str)
            .expect("every registered chain has an RPC URL")
    }

    /// The attachment bucket.
    pub fn attachments(&self) -> &AttachmentConfig {
        &self.attachments
    }

    /// The Proof of Payment attestation key.
    pub fn attestation(&self) -> &AttestationSignerConfig {
        &self.attestation
    }

    /// The payer Auth0 audience and payer-reference key; absent when email
    /// verification is not configured.
    pub fn payer_verification(&self) -> Option<&PayerVerificationConfig> {
        self.payer_verification.as_ref()
    }

    pub fn public_base_url(&self) -> &str {
        &self.public_base_url
    }

    /// Where the hosted checkout is served from; defaults to the public base
    /// URL, which is where `/pay/{id}` links already point.
    pub fn hosted_checkout_origin(&self) -> Option<&str> {
        self.hosted_checkout_origin.as_deref()
    }

    pub fn api_key_prefix(&self) -> &str {
        &self.api_key_prefix
    }

    pub fn webhook_encryption_key(&self) -> Option<[u8; 32]> {
        self.webhook_encryption_key
    }

    pub fn notification_from_address(&self) -> Option<&str> {
        self.notification_from_address.as_deref()
    }

    pub fn resend(&self) -> Option<&ResendConfig> {
        self.resend.as_ref()
    }
}

/// The payer email's `From`: Gum's contact mailbox unless a deployment
/// (the sandbox, on its own subdomain) says otherwise. Whatever it is, it
/// must at least look like a mailbox, since the dispatcher only learns of a
/// bad sender from the provider, one rejected email at a time.
fn payer_email_from(value: Option<String>) -> Result<String, String> {
    let value = value
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| crate::payer_email::DEFAULT_FROM.to_owned());
    let address = match (value.rfind('<'), value.ends_with('>')) {
        (Some(start), true) => &value[start + 1..value.len() - 1],
        (None, false) => value.as_str(),
        _ => return Err(format!("invalid GUM_PAYER_EMAIL_FROM '{value}'")),
    };
    match address.split_once('@') {
        Some((local, domain)) if !local.is_empty() && domain.contains('.') => Ok(value),
        _ => Err(format!("invalid GUM_PAYER_EMAIL_FROM '{value}'")),
    }
}

/// The recovery wallet: a 20-byte address, checksummed or not, never the
/// zero address — the contracts forward recoveries straight to it, where
/// they would be uncollectible.
fn recovery_address(value: &str) -> Result<Address, String> {
    let address: Address = value
        .parse::<Address>()
        .map_err(|_| format!("invalid GUM_RECOVERY_ADDRESS '{value}': expected an EVM address"))?;
    if address == Address::ZERO {
        return Err(format!(
            "invalid GUM_RECOVERY_ADDRESS '{value}': the zero address cannot custody recovered funds"
        ));
    }
    Ok(address)
}

/// Exactly one attestation key source: a raw key locally, KMS in production.
/// Both or neither is a deployment mistake, not a fallback.
fn attestation_signer(
    signer_key: Option<String>,
    kms_key_id: Option<String>,
) -> Result<AttestationSignerConfig, &'static str> {
    let signer_key = signer_key.filter(|value| !value.trim().is_empty());
    let kms_key_id = kms_key_id.filter(|value| !value.trim().is_empty());
    match (signer_key, kms_key_id) {
        (Some(key), None) => Ok(AttestationSignerConfig::Local(key)),
        (None, Some(key_id)) => Ok(AttestationSignerConfig::AwsKms(key_id)),
        (Some(_), Some(_)) => {
            Err("GUM_ATTESTATION_SIGNER_KEY and GUM_ATTESTATION_KMS_KEY_ID are mutually exclusive")
        }
        (None, None) => Err(
            "exactly one of GUM_ATTESTATION_SIGNER_KEY or GUM_ATTESTATION_KMS_KEY_ID must be set",
        ),
    }
}

fn parse_download_ttl(value: Option<&str>) -> Result<Duration, String> {
    let seconds = match value {
        None => DEFAULT_DOWNLOAD_TTL_SECS,
        Some(value) => value.parse::<u64>().map_err(|_| {
            "GUM_ATTACHMENT_DOWNLOAD_TTL_SECS must be a positive integer".to_string()
        })?,
    };
    if !(1..=MAX_PRESIGN_SECS).contains(&seconds) {
        return Err(format!(
            "GUM_ATTACHMENT_DOWNLOAD_TTL_SECS must be between 1 and {MAX_PRESIGN_SECS}"
        ));
    }
    Ok(Duration::from_secs(seconds))
}

fn validate_api_key_prefix(value: &str) -> Result<(), &'static str> {
    match value {
        "gum_live_" | "gum_test_" => Ok(()),
        _ => Err("GUM_API_KEY_PREFIX must be exactly gum_live_ or gum_test_"),
    }
}

/// A 256-bit key supplied as standard base64.
fn decode_key_32(name: &str, value: &str) -> Result<[u8; 32], String> {
    use base64::Engine;
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| format!("{name} must be valid standard base64"))?;
    decoded
        .try_into()
        .map_err(|_| format!("{name} must decode to exactly 32 bytes"))
}

/// The dedicated payer audience (product plan §6.1): its own Auth0 API and
/// application, so a payer token can never reach the merchant API and a
/// merchant token never unlocks an invoice. The master key derives the
/// merchant-scoped payer references.
pub struct PayerVerificationConfig {
    pub issuer: String,
    pub audience: String,
    pub client_id: String,
    pub payer_ref_master_key: [u8; 32],
}

/// The Privy app merchants sign in to. The public app id is all that dashboard
/// session verification needs: it names the JWKS identity tokens are checked
/// against and is the audience they must carry. `app_secret` is unrelated to
/// verification — it is only for wallet pregeneration
/// (pregenerated_wallet.rs), and stays optional: sign-in creates a merchant's
/// wallet itself regardless of whether this is set.
pub struct PrivyConfig {
    pub app_id: String,
    pub app_secret: Option<String>,
}

/// The S3 bucket (or MinIO, through the endpoint override) holding PDFs.
pub struct AttachmentConfig {
    pub bucket: String,
    pub s3_endpoint: Option<String>,
    pub force_path_style: bool,
    pub download_ttl: Duration,
}

/// Which key signs Proof of Payment attestations.
pub enum AttestationSignerConfig {
    Local(String),
    AwsKms(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_supported_api_key_prefixes() {
        assert!(validate_api_key_prefix("gum_live_").is_ok());
        assert!(validate_api_key_prefix("gum_test_").is_ok());
        assert!(validate_api_key_prefix("gum_dev_").is_err());
        assert!(validate_api_key_prefix("gum_live").is_err());
    }

    #[test]
    fn recovery_address_parses_but_never_accepts_the_zero_address() {
        let parsed = recovery_address("0xf78b72F68d560c06C36c3BeF86F1f055b83221e5").unwrap();
        assert_eq!(
            parsed.to_checksum(None),
            "0xf78b72F68d560c06C36c3BeF86F1f055b83221e5"
        );
        assert!(recovery_address("not-an-address").is_err());
        let zero = recovery_address(&format!("0x{}", "0".repeat(40))).unwrap_err();
        assert!(zero.contains("zero address"), "{zero}");
    }

    #[test]
    fn attestation_signer_comes_from_exactly_one_source() {
        assert!(matches!(
            attestation_signer(Some("0xabc".into()), None),
            Ok(AttestationSignerConfig::Local(key)) if key == "0xabc"
        ));
        assert!(matches!(
            attestation_signer(None, Some("arn:aws:kms:key".into())),
            Ok(AttestationSignerConfig::AwsKms(id)) if id == "arn:aws:kms:key"
        ));
        assert!(attestation_signer(None, None).is_err());
        assert!(attestation_signer(Some(" ".into()), Some("".into())).is_err());
        assert!(attestation_signer(Some("0xabc".into()), Some("arn".into())).is_err());
    }

    #[test]
    fn download_ttl_defaults_to_five_minutes_within_presign_limits() {
        assert_eq!(parse_download_ttl(None).unwrap(), Duration::from_secs(300));
        assert_eq!(
            parse_download_ttl(Some("60")).unwrap(),
            Duration::from_secs(60)
        );
        assert!(parse_download_ttl(Some("0")).is_err());
        assert!(parse_download_ttl(Some("604801")).is_err());
        assert!(parse_download_ttl(Some("soon")).is_err());
    }

    #[test]
    fn keys_require_standard_base64_of_32_bytes() {
        assert_eq!(
            decode_key_32(
                "GUM_WEBHOOK_ENCRYPTION_KEY",
                "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8="
            )
            .unwrap()
            .len(),
            32
        );
        assert!(decode_key_32("GUM_PAYER_REF_MASTER_KEY", "not base64").is_err());
        assert!(
            decode_key_32("GUM_PAYER_REF_MASTER_KEY", "c2hvcnQ=")
                .unwrap_err()
                .contains("GUM_PAYER_REF_MASTER_KEY")
        );
    }

    #[test]
    fn payer_email_from_defaults_to_contact_and_must_be_a_mailbox() {
        assert_eq!(payer_email_from(None).unwrap(), "Gum <contact@gum.money>");
        assert_eq!(
            payer_email_from(Some("  ".into())).unwrap(),
            "Gum <contact@gum.money>"
        );
        assert_eq!(
            payer_email_from(Some(" Gum Sandbox <contact@sandbox.gum.money> ".into())).unwrap(),
            "Gum Sandbox <contact@sandbox.gum.money>"
        );
        assert_eq!(
            payer_email_from(Some("contact@gum.money".into())).unwrap(),
            "contact@gum.money"
        );
        for bad in [
            "Gum",
            "Gum <contact>",
            "Gum <contact@gum.money",
            "@gum.money",
        ] {
            assert!(payer_email_from(Some(bad.into())).is_err(), "{bad}");
        }
    }
}
