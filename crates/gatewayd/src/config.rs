use std::str::FromStr;
use std::time::Duration;

use alloy_primitives::{Address, B256};
use gateway_core::ChainId;

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
    chain_id: ChainId,
    factory_address: Address,
    usdc_address: Address,
    settlement: SettlementConfig,
    attachments: AttachmentConfig,
    attestation: AttestationSignerConfig,
    /// The wallet that pays the onboarding walkthrough's one self-issued
    /// deposit request; `None` leaves that endpoint unavailable. Unlike
    /// `attestation`, this stays optional — production may legitimately
    /// never fund this feature.
    onboarding_payer: Option<OnboardingPayerSignerConfig>,
    /// The payer audience and the payer-reference key; both `None` leaves
    /// the email verification routes unavailable.
    payer_verification: Option<PayerVerificationConfig>,
    public_base_url: String,
    /// The one browser origin that may call the verification write routes.
    hosted_checkout_origin: Option<String>,
    explorer_base_url: Option<String>,
    api_key_prefix: String,
    webhook_encryption_key: Option<[u8; 32]>,
    notification_from_address: Option<String>,
    /// The Resend key that sends payers their deposit request emails;
    /// `None` leaves those queued and unsent.
    resend: Option<ResendConfig>,
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
        let dev_identity = std::env::var("PAYDAY_DEV_IDENTITY").as_deref() == Ok("1");
        // Absent (or empty, as an unconfigured task might set it) means
        // dashboard sessions are not accepted at all.
        let privy_app_secret = std::env::var("PAYDAY_PRIVY_APP_SECRET")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        let privy = std::env::var("PAYDAY_PRIVY_APP_ID")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .map(|app_id| PrivyConfig {
                app_id,
                app_secret: privy_app_secret.clone(),
            });
        if privy.is_none() && privy_app_secret.is_some() {
            panic!(
                "PAYDAY_PRIVY_APP_SECRET is set but PAYDAY_PRIVY_APP_ID is not; wallet pregeneration needs both"
            );
        }

        let payer_verification = match (
            std::env::var("PAYDAY_PAYER_AUTH0_ISSUER").ok(),
            std::env::var("PAYDAY_PAYER_AUTH0_AUDIENCE").ok(),
            std::env::var("PAYDAY_PAYER_AUTH0_CLIENT_ID").ok(),
            std::env::var("PAYDAY_PAYER_REF_MASTER_KEY").ok(),
        ) {
            (Some(issuer), Some(audience), Some(client_id), Some(master_key)) => {
                Some(PayerVerificationConfig {
                    issuer,
                    audience,
                    client_id,
                    payer_ref_master_key: decode_key_32("PAYDAY_PAYER_REF_MASTER_KEY", &master_key)
                        .unwrap_or_else(|message| panic!("{message}")),
                })
            }
            (None, None, None, None) => None,
            _ => panic!(
                "PAYDAY_PAYER_AUTH0_ISSUER, PAYDAY_PAYER_AUTH0_AUDIENCE, PAYDAY_PAYER_AUTH0_CLIENT_ID, and PAYDAY_PAYER_REF_MASTER_KEY must be set together"
            ),
        };

        let factory_address =
            std::env::var("PAYDAY_FACTORY_ADDRESS").expect("PAYDAY_FACTORY_ADDRESS must be set");
        let factory_address = Address::from_str(&factory_address)
            .unwrap_or_else(|e| panic!("invalid PAYDAY_FACTORY_ADDRESS '{factory_address}': {e}"));

        let chain_id: u64 = std::env::var("PAYDAY_CHAIN_ID")
            .expect("PAYDAY_CHAIN_ID must be set")
            .parse()
            .unwrap_or_else(|e| panic!("invalid PAYDAY_CHAIN_ID: {e}"));

        let usdc_address =
            std::env::var("PAYDAY_USDC_ADDRESS").expect("PAYDAY_USDC_ADDRESS must be set");
        let usdc_address = Address::from_str(&usdc_address)
            .unwrap_or_else(|e| panic!("invalid PAYDAY_USDC_ADDRESS '{usdc_address}': {e}"));

        let required =
            |name: &str| std::env::var(name).unwrap_or_else(|_| panic!("{name} must be set"));
        let settlement = SettlementConfig {
            rpc_url: required("PAYDAY_RPC_URL"),
            batch_sweeper_address: Address::from_str(&required("PAYDAY_BATCH_SWEEPER_ADDRESS"))
                .unwrap_or_else(|e| panic!("invalid PAYDAY_BATCH_SWEEPER_ADDRESS: {e}")),
            factory_code_hash: parse_code_hash(
                "PAYDAY_FACTORY_CODE_HASH",
                &required("PAYDAY_FACTORY_CODE_HASH"),
            )
            .unwrap_or_else(|message| panic!("{message}")),
            batch_sweeper_code_hash: parse_code_hash(
                "PAYDAY_BATCH_SWEEPER_CODE_HASH",
                &required("PAYDAY_BATCH_SWEEPER_CODE_HASH"),
            )
            .unwrap_or_else(|message| panic!("{message}")),
        };

        let attachments = AttachmentConfig {
            bucket: required("PAYDAY_ATTACHMENT_BUCKET"),
            s3_endpoint: std::env::var("PAYDAY_ATTACHMENT_S3_ENDPOINT")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            force_path_style: std::env::var("PAYDAY_ATTACHMENT_S3_FORCE_PATH_STYLE").as_deref()
                == Ok("1"),
            download_ttl: parse_download_ttl(
                std::env::var("PAYDAY_ATTACHMENT_DOWNLOAD_TTL_SECS")
                    .ok()
                    .as_deref(),
            )
            .unwrap_or_else(|message| panic!("{message}")),
        };
        let attestation = attestation_signer(
            std::env::var("PAYDAY_ATTESTATION_SIGNER_KEY").ok(),
            std::env::var("PAYDAY_ATTESTATION_KMS_KEY_ID").ok(),
        )
        .unwrap_or_else(|message| panic!("{message}"));

        let onboarding_payer = onboarding_payer_signer(
            std::env::var("PAYDAY_ONBOARDING_PAYER_KEY").ok(),
            std::env::var("PAYDAY_ONBOARDING_PAYER_KMS_KEY_ID").ok(),
        )
        .unwrap_or_else(|message| panic!("{message}"));

        let api_key_prefix =
            std::env::var("PAYDAY_API_KEY_PREFIX").unwrap_or_else(|_| "payday_live_".into());
        validate_api_key_prefix(&api_key_prefix).unwrap_or_else(|message| panic!("{message}"));
        let webhook_encryption_key =
            std::env::var("PAYDAY_WEBHOOK_ENCRYPTION_KEY")
                .ok()
                .map(|value| {
                    decode_key_32("PAYDAY_WEBHOOK_ENCRYPTION_KEY", &value)
                        .unwrap_or_else(|message| panic!("{message}"))
                });

        let resend = std::env::var("PAYDAY_RESEND_API_KEY")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty())
            .map(|api_key| ResendConfig {
                api_key,
                from: payer_email_from(std::env::var("PAYDAY_PAYER_EMAIL_FROM").ok())
                    .unwrap_or_else(|message| panic!("{message}")),
            });

        Config {
            bind_addr: std::env::var("PAYDAY_BIND_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:3000".into()),
            database_url: std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"),
            privy,
            dev_identity,
            chain_id: ChainId(chain_id),
            factory_address,
            usdc_address,
            settlement,
            attachments,
            attestation,
            onboarding_payer,
            payer_verification,
            public_base_url: std::env::var("PAYDAY_PUBLIC_BASE_URL")
                .unwrap_or_else(|_| "http://127.0.0.1:3000".into()),
            hosted_checkout_origin: std::env::var("PAYDAY_HOSTED_CHECKOUT_ORIGIN")
                .ok()
                .filter(|value| !value.trim().is_empty()),
            explorer_base_url: std::env::var("PAYDAY_EXPLORER_BASE_URL").ok(),
            api_key_prefix,
            webhook_encryption_key,
            notification_from_address: std::env::var("PAYDAY_NOTIFICATION_FROM_ADDRESS").ok(),
            resend,
        }
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

    pub fn chain_id(&self) -> ChainId {
        self.chain_id
    }

    pub fn factory_address(&self) -> Address {
        self.factory_address
    }

    pub fn usdc_address(&self) -> Address {
        self.usdc_address
    }

    /// The chain deployment this service issues addresses against.
    pub fn settlement(&self) -> &SettlementConfig {
        &self.settlement
    }

    /// The attachment bucket.
    pub fn attachments(&self) -> &AttachmentConfig {
        &self.attachments
    }

    /// The Proof of Payment attestation key.
    pub fn attestation(&self) -> &AttestationSignerConfig {
        &self.attestation
    }

    /// The onboarding demo payment's signing wallet; absent unless a
    /// deployment has deliberately funded and configured one.
    pub fn onboarding_payer(&self) -> Option<&OnboardingPayerSignerConfig> {
        self.onboarding_payer.as_ref()
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

    pub fn explorer_base_url(&self) -> Option<&str> {
        self.explorer_base_url.as_deref()
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

/// The payer email's `From`: Payday's contact mailbox unless a deployment
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
        _ => return Err(format!("invalid PAYDAY_PAYER_EMAIL_FROM '{value}'")),
    };
    match address.split_once('@') {
        Some((local, domain)) if !local.is_empty() && domain.contains('.') => Ok(value),
        _ => Err(format!("invalid PAYDAY_PAYER_EMAIL_FROM '{value}'")),
    }
}

/// keccak256 of the runtime bytecode `eth_getCode` returns for a contract,
/// as `cast keccak "$(cast code <ADDRESS> --rpc-url <RPC_URL>)"` prints it.
fn parse_code_hash(name: &str, value: &str) -> Result<B256, String> {
    B256::from_str(value)
        .map_err(|e| format!("invalid {name} '{value}': expected 0x-prefixed 32-byte hex: {e}"))
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
        (Some(_), Some(_)) => Err(
            "PAYDAY_ATTESTATION_SIGNER_KEY and PAYDAY_ATTESTATION_KMS_KEY_ID are mutually exclusive",
        ),
        (None, None) => Err(
            "exactly one of PAYDAY_ATTESTATION_SIGNER_KEY or PAYDAY_ATTESTATION_KMS_KEY_ID must be set",
        ),
    }
}

/// At most one onboarding payer signing source, same rule as attestation —
/// but unlike attestation, having *neither* set is the expected production
/// default, not a deployment mistake.
fn onboarding_payer_signer(
    signer_key: Option<String>,
    kms_key_id: Option<String>,
) -> Result<Option<OnboardingPayerSignerConfig>, &'static str> {
    let signer_key = signer_key.filter(|value| !value.trim().is_empty());
    let kms_key_id = kms_key_id.filter(|value| !value.trim().is_empty());
    match (signer_key, kms_key_id) {
        (Some(key), None) => Ok(Some(OnboardingPayerSignerConfig::Local(key))),
        (None, Some(key_id)) => Ok(Some(OnboardingPayerSignerConfig::AwsKms(key_id))),
        (Some(_), Some(_)) => Err(
            "PAYDAY_ONBOARDING_PAYER_KEY and PAYDAY_ONBOARDING_PAYER_KMS_KEY_ID are mutually exclusive",
        ),
        (None, None) => Ok(None),
    }
}

fn parse_download_ttl(value: Option<&str>) -> Result<Duration, String> {
    let seconds = match value {
        None => DEFAULT_DOWNLOAD_TTL_SECS,
        Some(value) => value.parse::<u64>().map_err(|_| {
            "PAYDAY_ATTACHMENT_DOWNLOAD_TTL_SECS must be a positive integer".to_string()
        })?,
    };
    if !(1..=MAX_PRESIGN_SECS).contains(&seconds) {
        return Err(format!(
            "PAYDAY_ATTACHMENT_DOWNLOAD_TTL_SECS must be between 1 and {MAX_PRESIGN_SECS}"
        ));
    }
    Ok(Duration::from_secs(seconds))
}

fn validate_api_key_prefix(value: &str) -> Result<(), &'static str> {
    match value {
        "payday_live_" | "payday_test_" => Ok(()),
        _ => Err("PAYDAY_API_KEY_PREFIX must be exactly payday_live_ or payday_test_"),
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

/// Which key signs the onboarding walkthrough's one demo transfer.
pub enum OnboardingPayerSignerConfig {
    Local(String),
    AwsKms(String),
}

/// The contracts this build must find on the chain.
pub struct SettlementConfig {
    pub rpc_url: String,
    pub batch_sweeper_address: Address,
    pub factory_code_hash: B256,
    pub batch_sweeper_code_hash: B256,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_hashes_are_0x_prefixed_32_byte_hex() {
        let hash = "0x".to_string() + &"ab".repeat(32);
        assert_eq!(
            parse_code_hash("PAYDAY_FACTORY_CODE_HASH", &hash).unwrap(),
            B256::repeat_byte(0xab)
        );
        let error = parse_code_hash("PAYDAY_FACTORY_CODE_HASH", "0xabcd").unwrap_err();
        assert!(error.contains("PAYDAY_FACTORY_CODE_HASH"));
        assert!(parse_code_hash("PAYDAY_BATCH_SWEEPER_CODE_HASH", "").is_err());
        assert!(parse_code_hash("PAYDAY_BATCH_SWEEPER_CODE_HASH", &"zz".repeat(32)).is_err());
    }

    #[test]
    fn accepts_only_supported_api_key_prefixes() {
        assert!(validate_api_key_prefix("payday_live_").is_ok());
        assert!(validate_api_key_prefix("payday_test_").is_ok());
        assert!(validate_api_key_prefix("payday_dev_").is_err());
        assert!(validate_api_key_prefix("payday_live").is_err());
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
    fn onboarding_payer_signer_is_optional_but_not_ambiguous() {
        assert!(matches!(
            onboarding_payer_signer(Some("0xabc".into()), None),
            Ok(Some(OnboardingPayerSignerConfig::Local(key))) if key == "0xabc"
        ));
        assert!(matches!(
            onboarding_payer_signer(None, Some("arn:aws:kms:key".into())),
            Ok(Some(OnboardingPayerSignerConfig::AwsKms(id))) if id == "arn:aws:kms:key"
        ));
        assert!(matches!(onboarding_payer_signer(None, None), Ok(None)));
        assert!(matches!(
            onboarding_payer_signer(Some(" ".into()), Some("".into())),
            Ok(None)
        ));
        assert!(onboarding_payer_signer(Some("0xabc".into()), Some("arn".into())).is_err());
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
                "PAYDAY_WEBHOOK_ENCRYPTION_KEY",
                "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8="
            )
            .unwrap()
            .len(),
            32
        );
        assert!(decode_key_32("PAYDAY_PAYER_REF_MASTER_KEY", "not base64").is_err());
        assert!(
            decode_key_32("PAYDAY_PAYER_REF_MASTER_KEY", "c2hvcnQ=")
                .unwrap_err()
                .contains("PAYDAY_PAYER_REF_MASTER_KEY")
        );
    }

    #[test]
    fn payer_email_from_defaults_to_contact_and_must_be_a_mailbox() {
        assert_eq!(
            payer_email_from(None).unwrap(),
            "Payday <contact@payday.sh>"
        );
        assert_eq!(
            payer_email_from(Some("  ".into())).unwrap(),
            "Payday <contact@payday.sh>"
        );
        assert_eq!(
            payer_email_from(Some(" Payday Sandbox <contact@sandbox.payday.sh> ".into())).unwrap(),
            "Payday Sandbox <contact@sandbox.payday.sh>"
        );
        assert_eq!(
            payer_email_from(Some("contact@payday.sh".into())).unwrap(),
            "contact@payday.sh"
        );
        for bad in [
            "Payday",
            "Payday <contact>",
            "Payday <contact@payday.sh",
            "@payday.sh",
        ] {
            assert!(payer_email_from(Some(bad.into())).is_err(), "{bad}");
        }
    }
}
