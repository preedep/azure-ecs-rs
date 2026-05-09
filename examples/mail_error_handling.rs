/// Example: typed error handling with ACSError (Phase 1)
///
/// Demonstrates matching on ACSError variants so callers can react
/// differently to network failures, auth problems, API errors, and
/// rate-limit exhaustion — without inspecting error message strings.
///
/// Run:
///   RUST_LOG=info cargo run --example mail_error_handling
use std::env;
use std::time::Duration;

use azure_ecs_rs::adapters::gateways::acs_email::{ACSApiVersion, ACSClientBuilder};
use azure_ecs_rs::domain::entities::models::{
    ACSError, EmailAddress, EmailContent, Recipients, SentEmailBuilder,
};
use tracing::{error, info, warn};
use tracing_subscriber::EnvFilter;

fn get_env_var(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("env var {} not set", name))
}

fn handle_error(err: ACSError) {
    match err {
        ACSError::RateLimitExceeded { retries } => {
            warn!(
                retries,
                "Rate limit hit after all retries — implement back-off at call site"
            );
        }
        ACSError::Auth(msg) => {
            error!(
                msg,
                "Authentication failed — check credentials or managed identity binding"
            );
        }
        ACSError::Api { code, message } => {
            error!(code = ?code, message, "ACS API returned an error");
        }
        ACSError::Network(msg) => {
            error!(msg, "Network error — check connectivity and endpoint URL");
        }
        ACSError::Deserialization(msg) => {
            error!(
                msg,
                "Unexpected response shape from ACS — may indicate API version mismatch"
            );
        }
        other => {
            error!(err = %other, "Unhandled ACS error");
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Structured tracing output — respects RUST_LOG env var
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    dotenv::dotenv().ok();

    let connection_str = get_env_var("CONNECTION_STR");
    let sender = get_env_var("SENDER");
    let recipient = get_env_var("REPLY_EMAIL");
    let display_name = get_env_var("REPLY_EMAIL_DISPLAY");

    // Opt in to the latest API version
    let client = ACSClientBuilder::new()
        .connection_string(&connection_str)
        .api_version(ACSApiVersion::V20250901)
        .timeout(Duration::from_secs(30))
        .max_retries(3)
        .build()
        .expect("Failed to build ACSClient");

    let email = SentEmailBuilder::new()
        .sender(sender)
        .content(EmailContent {
            subject: Some("Error handling example".to_string()),
            plain_text: Some("Sent via azure-ecs-rs with typed error handling.".to_string()),
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
        Ok(id) => {
            info!(message_id = %id, "Email accepted by ACS");

            // Poll status and match on each possible terminal state
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                match client.get_email_status(&id).await {
                    Ok(status) => {
                        info!(%status, "Status update");
                        use azure_ecs_rs::domain::entities::models::EmailSendStatusType::*;
                        match status {
                            Succeeded => {
                                info!("Delivery confirmed");
                                break;
                            }
                            Failed | Canceled => {
                                error!(%status, "Delivery failed");
                                break;
                            }
                            Unknown => {
                                warn!("Unknown status — stopping poll");
                                break;
                            }
                            NotStarted | Running => {} // keep polling
                        }
                    }
                    Err(e) => {
                        handle_error(e);
                        break;
                    }
                }
            }
        }
        Err(e) => handle_error(e),
    }

    Ok(())
}
