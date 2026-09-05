use aws_sdk_sesv2::{
    Client as SesClient,
    types::{Body, Content, Destination, EmailContent, Message},
};
use gateway_db::{NotificationEvent, NotificationRepository};
use std::time::Duration;
use tokio::sync::watch;

pub async fn run(
    repo: NotificationRepository,
    ses: SesClient,
    from: String,
    mut shutdown: watch::Receiver<bool>,
) {
    loop {
        if *shutdown.borrow() {
            break;
        }
        match repo.claim().await {
            Ok(Some(event)) => deliver(&repo, &ses, &from, event).await,
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

async fn deliver(
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
        "Payout for payment {} needs attention.\n\nReason: {reason}\nAction: {action}\n\nThe funds remain safe while payout is paused.",
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
            if let Err(error) = repo
                .retry(event.id, event.attempts, &error.to_string())
                .await
            {
                tracing::error!(event_id=%event.id, %error, "notification retry could not be recorded");
            }
        }
    }
}

pub fn guidance(code: &str) -> (&'static str, &'static str) {
    match code {
        "beneficiary_blacklisted" => (
            "The payout address is restricted by the USDC issuer.",
            "Contact Payday support to agree on recovery after the payment expires.",
        ),
        "recovery_blacklisted" => (
            "The payer's wallet, where excess funds return, is restricted by the USDC issuer.",
            "Contact Payday support with the payment ID; the payer may need to be contacted.",
        ),
        "payment_address_blacklisted" => (
            "The payment address is restricted by the USDC issuer.",
            "Contact Payday support for a compliance escalation.",
        ),
        "balance_below_amount" => (
            "The finalized payment record does not match the on-chain balance.",
            "No action is needed from the payer; Payday support is investigating.",
        ),
        "parameters_mismatch" | "corrupt_row" => (
            "The stored payment details require manual review.",
            "Contact Payday support to review the payment before payout resumes.",
        ),
        "retries_exhausted" => (
            "Automatic payout attempts were unsuccessful.",
            "No action is needed from the payer; Payday support will inspect and retry the payout.",
        ),
        _ => (
            "Automatic payout requires a manual review.",
            "Contact Payday support and provide the payment ID.",
        ),
    }
}
