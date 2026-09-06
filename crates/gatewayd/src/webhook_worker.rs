use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use gateway_db::WebhookRepository;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub async fn run(repo: WebhookRepository, key: [u8; 32]) {
    loop {
        match repo.claim().await {
            Ok(Some(job)) => {
                let attempted_at = sqlx::types::chrono::Utc::now();
                let started = Instant::now();
                let result = tokio::time::timeout(Duration::from_secs(15), deliver(key, &job))
                    .await
                    .unwrap_or_else(|_| Err("webhook delivery timed out".into()));
                let duration = started.elapsed().as_millis().min(i64::MAX as u128) as i64;
                let (status, error) = match result {
                    Ok(s) if (200..300).contains(&s) => (Some(s), None),
                    Ok(s) => (Some(s), Some(format!("endpoint returned HTTP {s}"))),
                    Err(e) => (None, Some(e)),
                };
                // Do not discard a completed HTTP attempt. Keep retrying this
                // idempotent transaction until history and delivery state persist.
                loop {
                    match repo
                        .complete(&job, attempted_at, duration, status, error.as_deref())
                        .await
                    {
                        Ok(()) => break,
                        Err(e) => {
                            tracing::error!(delivery_id=%job.delivery_id,error=?e,"webhook completion persistence failed");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                }
            }
            Ok(None) => tokio::time::sleep(Duration::from_secs(1)).await,
            Err(e) => {
                tracing::error!(error=?e,"webhook claim failed");
                tokio::time::sleep(Duration::from_secs(5)).await
            }
        }
    }
}

pub fn validate_url(url: &reqwest::Url) -> Result<(), String> {
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.host_str().is_none()
    {
        return Err("url must use HTTPS, have a host, and contain no credentials".into());
    }
    if url.host_str().unwrap().parse::<IpAddr>().is_ok() {
        return Err("url must use a DNS hostname".into());
    }
    Ok(())
}
fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_multicast()
                || ip == Ipv4Addr::BROADCAST
                || a == 0
                || a >= 240
                || (a == 100 && (64..=127).contains(&b))
                || (a == 192 && b == 0 && (c == 0 || c == 2))
                || (a == 192 && b == 31 && c == 196)
                || (a == 192 && b == 52 && c == 193)
                || (a == 192 && b == 88 && c == 99)
                || (a == 192 && b == 175 && c == 48)
                || (a == 198 && (b == 18 || b == 19))
                || (a == 198 && b == 51 && c == 100)
                || (a == 203 && b == 0 && c == 113))
        }
        IpAddr::V6(ip) => {
            let s = ip.segments();
            if let Some(mapped) = ip.to_ipv4_mapped() {
                return public_ip(IpAddr::V4(mapped));
            }
            (0x2000..=0x3fff).contains(&s[0])
                && !(s[0] == 0x2001 && s[1] <= 0x01ff)
                && !(s[0] == 0x2001 && s[1] == 0x0db8)
                && s[0] != 0x2002
                && !(s[0] == 0x3fff && s[1] <= 0x0fff)
        }
    }
}
pub async fn resolve_public(url: &reqwest::Url) -> Result<Vec<SocketAddr>, String> {
    validate_url(url)?;
    let host = url.host_str().unwrap();
    let port = url.port_or_known_default().unwrap();
    let addresses: Vec<_> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("DNS resolution failed: {e}"))?
        .collect();
    if addresses.is_empty() || addresses.iter().any(|a| !public_ip(a.ip())) {
        return Err("webhook destination resolves to a non-public address".into());
    }
    Ok(addresses)
}
async fn deliver(key: [u8; 32], job: &gateway_db::DeliveryClaim) -> Result<u16, String> {
    if job.secret_cipher_version != 1 || job.secret_ciphertext.len() < 13 {
        return Err("invalid encrypted secret".into());
    }
    let url = reqwest::Url::parse(&job.url).map_err(|_| "invalid webhook URL")?;
    let addresses = resolve_public(&url).await?;
    let host = url.host_str().unwrap().to_owned();
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10));
    for address in addresses {
        builder = builder.resolve(&host, address);
    }
    let http = builder.build().map_err(|e| e.to_string())?;
    let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
    let nonce: [u8; 12] = job.secret_ciphertext[..12]
        .try_into()
        .expect("ciphertext length was checked");
    let secret = cipher
        .decrypt(&Nonce::from(nonce), &job.secret_ciphertext[12..])
        .map_err(|_| "secret decryption failed")?;
    let mut payload = job.payload.clone();
    checksum_addresses(&mut payload);
    let body = serde_json::to_vec(&payload).map_err(|e| e.to_string())?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_secs();
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(&secret).unwrap();
    mac.update(format!("v1.{timestamp}.").as_bytes());
    mac.update(&body);
    let signature = hex::encode(mac.finalize().into_bytes());
    let response = http
        .post(url)
        .header(
            "Payday-Event-Id",
            gateway_core::WebhookEventId(job.event_id).to_string(),
        )
        .header("Payday-Event-Type", &job.event_type)
        .header(
            "Payday-Signature",
            format!("v1,t={timestamp},sha256={signature}"),
        )
        .header("Content-Type", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    Ok(response.status().as_u16())
}

/// The database renders the deposit request's addresses as lowercase hex,
/// having no keccak to checksum with; the API renders them EIP-55. A handler
/// comparing the two must not have to normalize, so the body is brought to
/// the API's form here, before it is signed. Anything that is not a 20-byte
/// hex address is left exactly as it was.
fn checksum_addresses(payload: &mut serde_json::Value) {
    let Some(deposit_request) = payload
        .get_mut("data")
        .and_then(|data| data.get_mut("deposit_request"))
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };
    for field in ["payer_wallet", "address"] {
        if let Some(serde_json::Value::String(value)) = deposit_request.get_mut(field)
            && let Ok(address) = value.parse::<alloy_primitives::Address>()
        {
            *value = address.to_checksum(None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delivered_addresses_are_checksummed_like_the_api() {
        let mut payload = serde_json::json!({
            "data": {"deposit_request": {
                "payer_wallet": "0x5aaeb6053f3e94c9b9a09f33669435e7ef1beaed",
                "address": null,
                "status": "settled"
            }}
        });
        checksum_addresses(&mut payload);
        assert_eq!(
            payload["data"]["deposit_request"]["payer_wallet"],
            "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed"
        );
        assert!(payload["data"]["deposit_request"]["address"].is_null());
        let mut test_event = serde_json::json!({"data": {"test": true}});
        let before = test_event.clone();
        checksum_addresses(&mut test_event);
        assert_eq!(test_event, before);
    }
    #[test]
    fn rejects_non_public_ranges() {
        for ip in [
            "127.0.0.1",
            "10.0.0.1",
            "169.254.1.1",
            "192.0.2.1",
            "198.51.100.1",
            "203.0.113.1",
            "100.64.0.1",
            "198.18.0.1",
            "240.0.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "2001:db8::1",
            "2001::1",
            "2002::1",
            "3fff::1",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "192.1.1.1", "198.52.1.1", "203.1.1.1"] {
            assert!(public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(public_ip("2606:4700:4700::1111".parse().unwrap()));
    }
    #[test]
    fn rejects_credentials_and_non_https() {
        for u in [
            "http://example.com",
            "https://u:p@example.com",
            "https://127.0.0.1",
        ] {
            let url = reqwest::Url::parse(u).unwrap();
            assert!(validate_url(&url).is_err());
        }
    }
    #[test]
    fn signature_scheme_is_stable() {
        let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(b"secret").unwrap();
        mac.update(b"v1.123.");
        mac.update(b"{}");
        assert_eq!(
            hex::encode(mac.finalize().into_bytes()),
            "2dd04764bc671f4e8cacc04346d4bfbc68d38bbce3aa8c11e218110754745248"
        );
    }
}
