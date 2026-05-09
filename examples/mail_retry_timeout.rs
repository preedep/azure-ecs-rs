/// Example: configurable retry and timeout (Phase 2)
///
/// Shows how to tune `max_retries` and `timeout` on `ACSClientBuilder`
/// for different deployment environments (dev vs. production).
///
/// Run:
///   RUST_LOG=debug cargo run --example mail_retry_timeout
use std::env;
use std::time::Duration;

use azure_ecs_rs::adapters::gateways::acs_email::ACSClientBuilder;
use azure_ecs_rs::domain::entities::models::{
    ACSError, EmailAddress, EmailContent, Recipients, SentEmailBuilder,
};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

fn get_env_var(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("env var {} not set", name))
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

    // Production-grade client:
    //   - 30 s per-request timeout so a hung connection doesn't block forever
    //   - 5 retries with exponential backoff for transient 429/503
    let client = ACSClientBuilder::new()
        .connection_string(&connection_str)
        .timeout(Duration::from_secs(30))
        .max_retries(5)
        .build()
        .expect("Failed to build ACSClient");

    info!(
        timeout_secs = 30,
        max_retries = 5,
        "Client configured"
    );

    let email = SentEmailBuilder::new()
        .sender(sender)
        .content(EmailContent {
            subject: Some("Retry/timeout example".to_string()),
            plain_text: Some("Sent with configurable retry and timeout.".to_string()),
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
        .expect("Failed to build SentEmail");

    match client.send_email(&email).await {
        Ok(id) => info!(message_id = %id, "Email accepted"),
        Err(ACSError::RateLimitExceeded { retries }) => {
            warn!(
                retries,
                "All retries exhausted — consider increasing max_retries \
                 or adding a circuit-breaker at the call site"
            );
        }
        Err(ACSError::Network(msg)) if msg.contains("timed out") => {
            error!(
                timeout_secs = 30,
                "Request timed out — increase .timeout() or check endpoint latency"
            );
        }
        Err(e) => error!(err = %e, "Send failed"),
    }

    Ok(())
}
