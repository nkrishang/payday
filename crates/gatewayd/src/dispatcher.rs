//! Drains the notification outbox. Merchant rows (a payout that needs
//! attention) go out through SES; payer rows (the deposit request they were
//! named on) go out through Resend. Each sender is optional, and the loop
//! claims only the recipients it can actually deliver to, so a deployment
//! without one of them leaves those rows waiting rather than failing them.

use alloy_primitives::utils::format_units;
use aws_sdk_sesv2::{
    Client as SesClient,
    types::{Body, Content, Destination, EmailContent, Message},
};
use chrono::{DateTime, Utc};
use gateway_core::{Invoice, USDC_DECIMALS};
use gateway_db::{
    InvoiceRepository, NotificationEvent, NotificationRecipient, NotificationRepository,
};
use std::time::Duration;
use tokio::sync::watch;

use crate::api::payer::PayerAccess;
use crate::payer_email::{DepositRequestEmail, ResendClient, SendError};

/// The senders a deployment configured. `None` for either leaves that
/// recipient's rows unclaimed.
pub struct Senders {
    pub merchant: Option<(SesClient, String)>,
    pub payer: Option<ResendClient>,
}

impl Senders {
    pub fn recipients(&self) -> Vec<NotificationRecipient> {
        let mut recipients = Vec::new();
        if self.merchant.is_some() {
            recipients.push(NotificationRecipient::Merchant);
        }
        if self.payer.is_some() {
            recipients.push(NotificationRecipient::Payer);
        }
        recipients
    }
}

pub async fn run(
    repo: NotificationRepository,
    invoices: InvoiceRepository,
    payer_access: PayerAccess,
    senders: Senders,
    mut shutdown: watch::Receiver<bool>,
) {
    let recipients = senders.recipients();
    if recipients.is_empty() {
        return;
    }
    loop {
        if *shutdown.borrow() {
            break;
        }
        match repo.claim(&recipients).await {
            Ok(Some(event)) => match event.recipient.as_str() {
                "payer" => {
                    if let Some(resend) = &senders.payer {
                        deliver_payer(&repo, &invoices, &payer_access, resend, event).await
                    }
                }
                _ => {
                    if let Some((ses, from)) = &senders.merchant {
                        deliver_merchant(&repo, ses, from, event).await
                    }
                }
            },
            Ok(None) => tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(1)) => {},
                _ = shutdown.changed() => {},
            },
            Err(error) => {
                tracing::error!(%error, "notification claim failed");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }
}

/// The payer's copy of a deposit request. The row was queued with the
/// request; by the time it is sent the request may have been cancelled,
/// paid, or expired, and a link to any of those is worse than no email.
async fn deliver_payer(
    repo: &NotificationRepository,
    invoices: &InvoiceRepository,
    payer_access: &PayerAccess,
    resend: &ResendClient,
    event: NotificationEvent,
) {
    let Some(to) = &event.email else {
        abandon(repo, &event, "payer row has no email").await;
        return;
    };
    let row = match invoices.find_by_id(event.invoice_id).await {
        Ok(Some(row)) => row,
        Ok(None) => {
            abandon(repo, &event, "deposit request no longer exists").await;
            return;
        }
        Err(error) => {
            tracing::warn!(event_id=%event.id, %error, "deposit request could not be loaded for the payer email; retrying");
            retry(repo, &event, &error.to_string()).await;
            return;
        }
    };
    let invoice = match Invoice::try_from(&row) {
        Ok(invoice) => invoice,
        Err(error) => {
            abandon(
                repo,
                &event,
                &format!("deposit request row is unreadable: {error}"),
            )
            .await;
            return;
        }
    };
    if let Some(reason) = superseded(&invoice, Utc::now()) {
        tracing::info!(event_id=%event.id, payment_id=%event.invoice_id, reason, "payer email not sent");
        abandon(repo, &event, &format!("not sent: {reason}")).await;
        return;
    }
    let email = compose(&invoice, payer_access);
    match resend
        .send(
            &event.id.to_string(),
            to,
            &email.subject(),
            &email.html(),
            &email.text(),
        )
        .await
    {
        Ok(()) => {
            tracing::info!(event_id=%event.id, payment_id=%event.invoice_id, "payer deposit request email sent");
            if let Err(error) = repo.complete(event.id).await {
                tracing::error!(event_id=%event.id, %error, "email delivery could not be recorded");
            }
        }
        Err(error @ SendError::Rejected { .. }) => {
            tracing::warn!(event_id=%event.id, payment_id=%event.invoice_id, %error, "payer email rejected by the provider; giving up");
            abandon(repo, &event, &error.to_string()).await;
        }
        Err(error) => {
            tracing::warn!(event_id=%event.id, attempt=event.attempts, %error, "payer email delivery failed; retrying");
            retry(repo, &event, &error.to_string()).await;
        }
    }
}

/// Why the payer should no longer be pointed at this request, if they
/// should not.
fn superseded(invoice: &Invoice, now: DateTime<Utc>) -> Option<&'static str> {
    if invoice.cancellation_requested_at.is_some() {
        return Some("the deposit request was cancelled");
    }
    if invoice.expiration_timestamp <= now.timestamp().max(0) as u64 {
        return Some("the deposit request has expired");
    }
    if invoice.status != gateway_core::InvoiceStatus::Created {
        return Some("the deposit request is already funded");
    }
    None
}

fn compose(invoice: &Invoice, payer_access: &PayerAccess) -> DepositRequestEmail {
    let snapshot = &invoice.issuance_snapshot;
    DepositRequestEmail {
        issuer_name: snapshot.issuer.name.clone(),
        payer_name: snapshot.bill_to.name.clone(),
        amount: trimmed_usdc(&format_units(invoice.amount.0, USDC_DECIMALS).unwrap_or_default()),
        heading: snapshot.heading.clone(),
        reference: snapshot.reference.clone(),
        expires_at: DateTime::from_timestamp(invoice.expiration_timestamp as i64, 0)
            .unwrap_or_default(),
        deposit_url: payer_access.checkout_url(&invoice.id),
    }
}

/// "1250.500000" reads as "1250.5" in prose; "1.000000" as "1".
fn trimmed_usdc(value: &str) -> String {
    match value.split_once('.') {
        Some((whole, fraction)) => {
            let fraction = fraction.trim_end_matches('0');
            if fraction.is_empty() {
                whole.to_owned()
            } else {
                format!("{whole}.{fraction}")
            }
        }
        None => value.to_owned(),
    }
}

async fn abandon(repo: &NotificationRepository, event: &NotificationEvent, error: &str) {
    if let Err(db) = repo.abandon(event.id, error).await {
        tracing::error!(event_id=%event.id, %db, "abandoned notification could not be recorded");
    }
}

async fn retry(repo: &NotificationRepository, event: &NotificationEvent, error: &str) {
    if let Err(db) = repo.retry(event.id, event.attempts, error).await {
        tracing::error!(event_id=%event.id, %db, "notification retry could not be recorded");
    }
}

async fn deliver_merchant(
    repo: &NotificationRepository,
    ses: &SesClient,
    from: &str,
    event: NotificationEvent,
) {
    let Some(to) = &event.email else {
        match repo.report_missing_email(event.id).await {
            Ok(true) => tracing::error!(
                event_id=%event.id, payment_id=%event.invoice_id,
                "blocked payment notification has no merchant email"
            ),
            Ok(false) => {}
            Err(error) => {
                tracing::error!(event_id=%event.id, %error, "missing merchant email could not be recorded")
            }
        }
        return;
    };
    let (reason, action) = guidance(&event.reason);
    let text = format!(
        "Payout for deposit request {} needs attention.\n\nReason: {reason}\nAction: {action}\n\nThe funds remain safe while payout is paused.",
        event.invoice_id
    );
    let result = ses
        .send_email()
        .from_email_address(from)
        .destination(Destination::builder().to_addresses(to).build())
        .content(
            EmailContent::builder()
                .simple(
                    Message::builder()
                        .subject(
                            Content::builder()
                                .data("Payday payout needs attention")
                                .build()
                                .unwrap(),
                        )
                        .body(
                            Body::builder()
                                .text(Content::builder().data(text).build().unwrap())
                                .build(),
                        )
                        .build(),
                )
                .build(),
        )
        .send()
        .await;
    match result {
        Ok(_) => {
            if let Err(error) = repo.complete(event.id).await {
                tracing::error!(event_id=%event.id, %error, "email delivery could not be recorded");
            }
        }
        Err(error) => {
            tracing::warn!(event_id=%event.id, attempt=event.attempts, %error, "merchant notification delivery failed; retrying");
            retry(repo, &event, &error.to_string()).await;
        }
    }
}

pub fn guidance(code: &str) -> (&'static str, &'static str) {
    match code {
        "beneficiary_blacklisted" => (
            "The payout address is restricted by the USDC issuer.",
            "Contact Payday support to agree on recovery after the deposit request expires.",
        ),
        "recovery_blacklisted" => (
            "The payer's wallet, where excess funds return, is restricted by the USDC issuer.",
            "Contact Payday support with the deposit request ID; the payer may need to be contacted.",
        ),
        "payment_address_blacklisted" => (
            "The deposit address is restricted by the USDC issuer.",
            "Contact Payday support for a compliance escalation.",
        ),
        "balance_below_amount" => (
            "The finalized deposit record does not match the on-chain balance.",
            "No action is needed from the payer; Payday support is investigating.",
        ),
        "parameters_mismatch" | "corrupt_row" => (
            "The stored deposit request details require manual review.",
            "Contact Payday support to review the deposit request before payout resumes.",
        ),
        "retries_exhausted" => (
            "Automatic payout attempts were unsuccessful.",
            "No action is needed from the payer; Payday support will inspect and retry the payout.",
        ),
        _ => (
            "Automatic payout requires a manual review.",
            "Contact Payday support and provide the deposit request ID.",
        ),
    }
}

#[cfg(test)]
mod tests {
    use alloy_primitives::{U256, address};
    use gateway_core::{
        Amount, BeneficiaryAddress, CanonicalIssuanceSnapshot, ChainId, FactoryAddress,
        InvoiceStatus, Party, PayerPolicy, TokenAddress,
    };

    use super::*;

    fn invoice() -> Invoice {
        let factory = FactoryAddress(address!("0x0000000000000000000000000000000000000001"));
        let token = TokenAddress(address!("0x754704Bc059F8C67012fEd69BC8A327a5aafb603"));
        let beneficiary =
            BeneficiaryAddress(address!("0x0000000000000000000000000000000000000002"));
        let amount = Amount(U256::from(1_250_500_000));
        let party = |name: &str, email: Option<&str>| Party {
            name: name.into(),
            email: email.map(str::to_owned),
            details: None,
        };
        let mut snapshot = CanonicalIssuanceSnapshot::new(
            party("Acme", None),
            party("Globex", Some("payer@example.com")),
            PayerPolicy::Permissionless,
            factory,
            ChainId(143),
            token,
            beneficiary,
            amount,
            1_788_000_000,
        );
        snapshot.heading = Some("March retainer".into());
        snapshot.reference = Some("INV-001".into());
        Invoice::issue(
            factory,
            ChainId(143),
            token,
            beneficiary,
            amount,
            1_788_000_000,
            snapshot,
        )
        .unwrap()
    }

    #[test]
    fn composes_the_payer_email_from_the_issued_document() {
        let access = PayerAccess::new("https://payday.sh/", None, None).unwrap();
        let invoice = invoice();
        let email = compose(&invoice, &access);
        assert_eq!(email.issuer_name, "Acme");
        assert_eq!(email.payer_name, "Globex");
        assert_eq!(email.amount, "1250.5");
        assert_eq!(email.heading.as_deref(), Some("March retainer"));
        assert_eq!(email.reference.as_deref(), Some("INV-001"));
        assert_eq!(email.expires_at.timestamp(), 1_788_000_000);
        assert_eq!(
            email.deposit_url,
            format!("https://payday.sh/pay/{}", invoice.id)
        );
    }

    #[test]
    fn a_request_the_payer_should_no_longer_open_is_not_emailed() {
        let now = DateTime::from_timestamp(1_787_000_000, 0).unwrap();
        let open = invoice();
        assert_eq!(superseded(&open, now), None);

        let mut cancelled = invoice();
        cancelled.cancellation_requested_at = Some("2026-09-06T00:00:00Z".into());
        assert_eq!(
            superseded(&cancelled, now),
            Some("the deposit request was cancelled")
        );

        let expired = invoice();
        let later = DateTime::from_timestamp(1_788_000_000, 0).unwrap();
        assert_eq!(
            superseded(&expired, later),
            Some("the deposit request has expired")
        );

        let mut funded = invoice();
        funded.status = InvoiceStatus::Funded;
        assert_eq!(
            superseded(&funded, now),
            Some("the deposit request is already funded")
        );
    }

    #[test]
    fn usdc_amounts_are_trimmed_for_prose() {
        assert_eq!(trimmed_usdc("1250.500000"), "1250.5");
        assert_eq!(trimmed_usdc("1.000000"), "1");
        assert_eq!(trimmed_usdc("0.000001"), "0.000001");
        assert_eq!(trimmed_usdc("42"), "42");
    }

    #[test]
    fn senders_claim_only_what_they_can_deliver() {
        let none = Senders {
            merchant: None,
            payer: None,
        };
        assert!(none.recipients().is_empty());
        let payer_only = Senders {
            merchant: None,
            payer: Some(ResendClient::new(
                "re_test".into(),
                "Payday <c@payday.sh>".into(),
            )),
        };
        assert_eq!(payer_only.recipients(), vec![NotificationRecipient::Payer]);
    }
}
