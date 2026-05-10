/// Example: send and wait for terminal delivery status
///
/// Demonstrates two variants:
///
/// 1. `send_email_and_wait` — blocks until Succeeded/Failed or a 60 s deadline.
/// 2. `send_email_and_wait_cancellable` — same, but a `CancellationToken` lets
///    another task (e.g. a web request handler) abort the wait early without
///    leaking the background poll loop.
///
/// Run:
///   RUST_LOG=info cargo run --example mail_wait
use std::env;
use std::time::Duration;

use azure_ecs_rs::adapters::gateways::acs_email::ACSClientBuilder;
use azure_ecs_rs::domain::entities::models::{
    ACSError, EmailAddress, EmailContent, EmailSendStatusType, Recipients, SentEmailBuilder,
};
use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

fn get_env_var(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("env var {} not set", name))
}

fn build_email(
    sender: String,
    recipient: String,
    display_name: String,
    subject: &str,
) -> azure_ecs_rs::domain::entities::models::SentEmail {
    SentEmailBuilder::new()
        .sender(sender)
        .content(EmailContent {
            subject: Some(subject.to_string()),
            plain_text: Some(format!("Sent via {}.", subject)),
            html: None,
        })
        .recipients(Recipients {
            to: Some(vec![EmailAddress {
                email: Some(recipient),
                display_name: Some(display_name),
            }]),
            cc: None,
            b_cc: None,
        })
        .build()
        .expect("Failed to build SentEmail")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    dotenv::dotenv().ok();

    let connection_str = get_env_var("CONNECTION_STR");
    let sender = get_env_var("SENDER");
    let recipient = get_env_var("REPLY_EMAIL");
    let display_name = get_env_var("REPLY_EMAIL_DISPLAY");

    let client = ACSClientBuilder::new()
        .connection_string(&connection_str)
        .timeout(Duration::from_secs(30))
        .max_retries(3)
        .poll_interval(Duration::from_secs(5))
        .build()
        .expect("Failed to build ACSClient");

    // ── 1. send_email_and_wait ────────────────────────────────────────────────
    //
    // Single call that returns the final delivery status.
    // ACSError::Timeout is returned if ACS hasn't reached a terminal state
    // within the deadline — the email may still be in transit.
    info!("Sending via send_email_and_wait (60 s deadline)…");

    let email = build_email(
        sender.clone(),
        recipient.clone(),
        display_name.clone(),
        "send_email_and_wait example",
    );

    match client
        .send_email_and_wait(&email, Duration::from_secs(60))
        .await
    {
        Ok(EmailSendStatusType::Succeeded) => info!("Delivered"),
        Ok(EmailSendStatusType::Failed) => error!("Delivery failed"),
        Ok(status) => warn!(%status, "Unexpected terminal status"),
        Err(ACSError::Timeout) => warn!("Timed out — email may still be in transit"),
        Err(e) => error!(err = %e, "Send failed"),
    }

    // ── 2. send_email_and_wait_cancellable ────────────────────────────────────
    //
    // A CancellationToken lets an external signal (e.g. a shutdown handler or
    // an HTTP request context) abort the wait cleanly. The email remains queued
    // in ACS; only the local wait is abandoned.
    info!("Sending via send_email_and_wait_cancellable (60 s deadline)…");

    let email2 = build_email(
        sender,
        recipient,
        display_name,
        "send_email_and_wait_cancellable example",
    );

    let token = CancellationToken::new();

    // Simulate an external abort after 10 s (e.g. process shutdown signal).
    let abort_token = token.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        info!("Simulating external cancellation…");
        abort_token.cancel();
    });

    match client
        .send_email_and_wait_cancellable(&email2, Duration::from_secs(60), token)
        .await
    {
        Ok(EmailSendStatusType::Succeeded) => info!("Delivered"),
        Ok(EmailSendStatusType::Failed) => error!("Delivery failed"),
        Ok(status) => warn!(%status, "Unexpected terminal status"),
        Err(ACSError::Canceled) => {
            warn!("Wait cancelled — email is still queued in ACS and may be delivered later")
        }
        Err(ACSError::Timeout) => warn!("Timed out — email may still be in transit"),
        Err(e) => error!(err = %e, "Send failed"),
    }

    Ok(())
}
