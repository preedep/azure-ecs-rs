/// Example: async attachment loading + tracing (Phase 3)
///
/// Uses `build_async()` to read the attachment file with `tokio::fs`
/// instead of blocking the async executor. Also shows full `tracing-subscriber`
/// setup so library spans appear alongside application logs.
///
/// Run:
///   RUST_LOG=debug cargo run --example mail_attach_async
use std::env;
use std::time::Duration;

use azure_ecs_rs::adapters::gateways::acs_email::ACSClientBuilder;
use azure_ecs_rs::domain::entities::models::{
    EmailAddress, EmailAttachmentBuilder, EmailContent, Recipients, SentEmailBuilder,
};
use tracing::{error, info, instrument};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

fn get_env_var(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("env var {} not set", name))
}

/// Builds an `EmailAttachment` from a file path using non-blocking I/O.
/// MIME type is detected automatically; falls back to `application/octet-stream`.
#[instrument]
async fn load_attachment(
    file_path: &str,
) -> azure_ecs_rs::domain::entities::models::EmailAttachment {
    info!("Loading attachment");
    EmailAttachmentBuilder::new()
        .file_to_base64(file_path)
        .build_async()
        .await
        .unwrap_or_else(|e| panic!("Failed to load attachment '{}': {}", file_path, e))
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Full tracing setup: fmt layer + env-filter.
    // Library spans (send_email, acs_send_email, …) appear at the configured level.
    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer())
        .init();

    dotenv::dotenv().ok();

    let sender = get_env_var("SENDER");
    let recipient = get_env_var("REPLY_EMAIL");
    let display_name = get_env_var("REPLY_EMAIL_DISPLAY");
    let host_name = get_env_var("ASC_URL");
    let tenant_id = get_env_var("TENANT_ID");
    let client_id = get_env_var("CLIENT_ID");
    let client_secret = get_env_var("CLIENT_SECRET");

    // Load the attachment asynchronously — no thread pool blocking
    let attachment = load_attachment("test_file.txt").await;

    let client = ACSClientBuilder::new()
        .host(&host_name)
        .service_principal(&tenant_id, &client_id, &client_secret)
        .timeout(Duration::from_secs(30))
        .max_retries(3)
        .build()
        .expect("Failed to build ACSClient");

    let email = SentEmailBuilder::new()
        .sender(sender)
        .content(EmailContent {
            subject: Some("Async attachment example".to_string()),
            plain_text: Some("See the attached file loaded with tokio::fs.".to_string()),
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
        .attachments(vec![attachment])
        .build()
        .expect("Failed to build SentEmail");

    // send_email_with_callback emits tracing spans — visible at RUST_LOG=debug
    let res = client
        .send_email_with_callback(&email, |msg_id, status, err| {
            if let Some(e) = err {
                error!(message_id = %msg_id, err = %e, "Callback error");
            } else {
                info!(message_id = %msg_id, %status, "Status update");
            }
        })
        .await;

    match res {
        Ok((id, rx)) => {
            let _ = rx.await;
            info!(message_id = %id, "Done");
        }
        Err(e) => error!(err = %e, "Failed to send"),
    }

    Ok(())
}
