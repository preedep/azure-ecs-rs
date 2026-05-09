/// Example: stream-based status polling (Phase 4)
///
/// Uses `send_email_stream()` to receive delivery status updates as a
/// `Stream` instead of a callback. The stream yields one item per poll
/// (every 5 s) and closes automatically on a terminal state.
///
/// Run:
///   RUST_LOG=debug cargo run --example mail_stream
use std::env;
use std::time::Duration;

use azure_ecs_rs::adapters::gateways::acs_email::ACSClientBuilder;
use azure_ecs_rs::domain::entities::models::{
    ACSError, EmailAddress, EmailContent, EmailSendStatusType, Recipients, SentEmailBuilder,
};
use futures::StreamExt;
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

    let client = ACSClientBuilder::new()
        .connection_string(&connection_str)
        .timeout(Duration::from_secs(30))
        .max_retries(3)
        .build()
        .expect("Failed to build ACSClient");

    let email = SentEmailBuilder::new()
        .sender(sender)
        .content(EmailContent {
            subject: Some("Stream polling example".to_string()),
            plain_text: Some("Status updates arrive via a Stream.".to_string()),
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

    let (message_id, stream) = client.send_email_stream(&email).await?;
    info!(message_id = %message_id, "Email accepted — polling status stream");

    tokio::pin!(stream);

    while let Some(item) = stream.next().await {
        match item {
            Ok(EmailSendStatusType::Succeeded) => {
                info!(message_id = %message_id, "Delivery confirmed");
                break;
            }
            Ok(EmailSendStatusType::Failed | EmailSendStatusType::Canceled) => {
                error!(message_id = %message_id, "Delivery failed");
                break;
            }
            Ok(EmailSendStatusType::Unknown) => {
                warn!(message_id = %message_id, "Unknown status — stopping poll");
                break;
            }
            Ok(status) => {
                info!(message_id = %message_id, %status, "Status update");
            }
            Err(ACSError::Network(msg)) => {
                error!(message_id = %message_id, msg, "Network error during poll");
                break;
            }
            Err(e) => {
                error!(message_id = %message_id, err = %e, "Poll error");
                break;
            }
        }
    }

    Ok(())
}
