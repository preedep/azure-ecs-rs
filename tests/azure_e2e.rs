//! End-to-end tests against a real Azure Email Communication Service resource.
//!
//! # Running
//!
//! These tests are marked `#[ignore]` and do **not** run during `cargo test`.
//! They require live Azure credentials and a provisioned ACS resource.
//!
//! ```bash
//! # Shared Key auth
//! CONNECTION_STR="endpoint=https://…;accesskey=…" \
//! SENDER="DoNotReply@yourdomain.com" \
//! TO_EMAIL="recipient@example.com" \
//! cargo test --test azure_e2e -- --include-ignored
//!
//! # Service Principal auth (additional vars)
//! TENANT_ID=… CLIENT_ID=… CLIENT_SECRET=… ASC_URL=https://… \
//! SENDER=… TO_EMAIL=… \
//! cargo test --test azure_e2e -- --include-ignored
//! ```
//!
//! # CI
//!
//! A dedicated workflow (`.github/workflows/azure_e2e.yml`) runs these on
//! `develop` and `main` only, gated behind GitHub Actions secrets:
//! `CONNECTION_STR`, `SENDER`, `TO_EMAIL`, `TENANT_ID`, `CLIENT_ID`,
//! `CLIENT_SECRET`, `ASC_URL`.
//!
//! # What these tests prove that wiremock cannot
//!
//! - HMAC-SHA256 shared-key signature is accepted by Azure (correct header format)
//! - ServicePrincipal OAuth2 token acquisition and bearer auth work end-to-end
//! - The ACS JSON request schema matches what Azure actually expects
//! - Operation IDs returned by ACS are real UUIDs that can be polled
//! - Both API versions (`2023-03-31`, `2025-09-01`) are accepted by the endpoint
//! - `send_emails_batch` concurrent dispatch is accepted by Azure
//! - `send_email_stream_cancellable` terminates cleanly against a real server

use azure_ecs_rs::adapters::gateways::acs_email::{ACSApiVersion, ACSClientBuilder};
use azure_ecs_rs::domain::entities::models::{
    EmailAddress, EmailContent, Recipients, SentEmailBuilder,
};
use futures::StreamExt;
use tokio_util::sync::CancellationToken;

// ── helpers ──────────────────────────────────────────────────────────────────

/// Read a required environment variable; panic with a clear message if absent.
fn require_env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| {
        panic!(
            "E2E test requires env var `{}`. \
             Run: {}=<value> cargo test --test azure_e2e -- --include-ignored",
            name, name
        )
    })
}

fn minimal_email(sender: &str, to: &str) -> azure_ecs_rs::domain::entities::models::SentEmail {
    SentEmailBuilder::new()
        .sender(sender.to_string())
        .content(EmailContent {
            subject: Some("Azure ECS SDK e2e test".to_string()),
            plain_text: Some("This is an automated e2e test from azure-ecs-rs.".to_string()),
            html: None,
        })
        .recipients(Recipients {
            to: Some(vec![EmailAddress {
                email: Some(to.to_string()),
                display_name: Some("E2E Test Recipient".to_string()),
            }]),
            cc: None,
            b_cc: None,
        })
        .build()
        .expect("minimal_email: all required fields are set")
}

// ── Shared Key auth ───────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Azure: set CONNECTION_STR, SENDER, TO_EMAIL"]
async fn shared_key_send_email_accepted() {
    let conn = require_env("CONNECTION_STR");
    let sender = require_env("SENDER");
    let to = require_env("TO_EMAIL");

    let client = ACSClientBuilder::new()
        .connection_string(&conn)
        .max_retries(1)
        .build()
        .expect("ACSClientBuilder::build");

    let email = minimal_email(&sender, &to);
    let result = client.send_email(&email).await;

    assert!(
        result.is_ok(),
        "send_email failed: {:?}",
        result.unwrap_err()
    );
    let operation_id = result.unwrap();
    assert!(!operation_id.is_empty(), "operation_id should not be empty");
    println!("operation_id: {}", operation_id);
}

#[tokio::test]
#[ignore = "requires Azure: set CONNECTION_STR, SENDER, TO_EMAIL"]
async fn shared_key_get_email_status_after_send() {
    let conn = require_env("CONNECTION_STR");
    let sender = require_env("SENDER");
    let to = require_env("TO_EMAIL");

    let client = ACSClientBuilder::new()
        .connection_string(&conn)
        .max_retries(1)
        .build()
        .expect("ACSClientBuilder::build");

    let email = minimal_email(&sender, &to);
    let operation_id = client
        .send_email(&email)
        .await
        .expect("send_email should succeed");

    // Poll immediately — status will be NotStarted or Running, not an error.
    let status = client
        .get_email_status(&operation_id)
        .await
        .expect("get_email_status should succeed");
    println!("status after send: {}", status);
}

#[tokio::test]
#[ignore = "requires Azure: set CONNECTION_STR, SENDER, TO_EMAIL"]
async fn shared_key_send_email_v20250901() {
    let conn = require_env("CONNECTION_STR");
    let sender = require_env("SENDER");
    let to = require_env("TO_EMAIL");

    let client = ACSClientBuilder::new()
        .connection_string(&conn)
        .api_version(ACSApiVersion::V20250901)
        .max_retries(1)
        .build()
        .expect("ACSClientBuilder::build");

    let email = minimal_email(&sender, &to);
    let result = client.send_email(&email).await;

    assert!(
        result.is_ok(),
        "send_email (2025-09-01) failed: {:?}",
        result.unwrap_err()
    );
    println!("operation_id (v2025): {}", result.unwrap());
}

#[tokio::test]
#[ignore = "requires Azure: set CONNECTION_STR, SENDER, TO_EMAIL"]
async fn shared_key_send_email_stream_yields_at_least_one_status() {
    let conn = require_env("CONNECTION_STR");
    let sender = require_env("SENDER");
    let to = require_env("TO_EMAIL");

    let client = ACSClientBuilder::new()
        .connection_string(&conn)
        .max_retries(1)
        .build()
        .expect("ACSClientBuilder::build");

    let email = minimal_email(&sender, &to);
    let (operation_id, stream) = client
        .send_email_stream(&email)
        .await
        .expect("send_email_stream should succeed");

    println!("stream operation_id: {}", operation_id);

    tokio::pin!(stream);
    // Take only the first status poll — the test does not wait for terminal.
    let first = stream
        .next()
        .await
        .expect("stream should yield at least one status");

    let status = first.expect("first status should be Ok");
    println!("first stream status: {}", status);
}

#[tokio::test]
#[ignore = "requires Azure: set CONNECTION_STR, SENDER, TO_EMAIL"]
async fn shared_key_send_email_stream_cancellable_stops_cleanly() {
    let conn = require_env("CONNECTION_STR");
    let sender = require_env("SENDER");
    let to = require_env("TO_EMAIL");

    let client = ACSClientBuilder::new()
        .connection_string(&conn)
        .max_retries(1)
        .build()
        .expect("ACSClientBuilder::build");

    let email = minimal_email(&sender, &to);
    let token = CancellationToken::new();

    let (operation_id, stream) = client
        .send_email_stream_cancellable(&email, token.clone())
        .await
        .expect("send_email_stream_cancellable should succeed");

    println!("cancellable stream operation_id: {}", operation_id);

    // Cancel after send but before polling — stream must return None immediately.
    token.cancel();
    tokio::pin!(stream);
    assert!(
        stream.next().await.is_none(),
        "cancelled stream must yield None"
    );
}

#[tokio::test]
#[ignore = "requires Azure: set CONNECTION_STR, SENDER, TO_EMAIL"]
async fn shared_key_send_emails_batch_all_accepted() {
    let conn = require_env("CONNECTION_STR");
    let sender = require_env("SENDER");
    let to = require_env("TO_EMAIL");

    let client = ACSClientBuilder::new()
        .connection_string(&conn)
        .max_retries(1)
        .build()
        .expect("ACSClientBuilder::build");

    let emails = vec![
        minimal_email(&sender, &to),
        minimal_email(&sender, &to),
        minimal_email(&sender, &to),
    ];

    let results = client.send_emails_batch(&emails).await;

    assert_eq!(results.len(), 3);
    for (i, result) in results.iter().enumerate() {
        assert!(
            result.is_ok(),
            "batch email {} failed: {:?}",
            i,
            result.as_ref().unwrap_err()
        );
        println!("batch[{}] operation_id: {}", i, result.as_ref().unwrap());
    }
}

// ── Service Principal auth ────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Azure: set TENANT_ID, CLIENT_ID, CLIENT_SECRET, ASC_URL, SENDER, TO_EMAIL"]
async fn service_principal_send_email_accepted() {
    let tenant_id = require_env("TENANT_ID");
    let client_id = require_env("CLIENT_ID");
    let client_secret = require_env("CLIENT_SECRET");
    let asc_url = require_env("ASC_URL");
    let sender = require_env("SENDER");
    let to = require_env("TO_EMAIL");

    let client = ACSClientBuilder::new()
        .host(&asc_url)
        .service_principal(&tenant_id, &client_id, &client_secret)
        .max_retries(1)
        .build()
        .expect("ACSClientBuilder::build");

    let email = minimal_email(&sender, &to);
    let result = client.send_email(&email).await;

    assert!(
        result.is_ok(),
        "service_principal send_email failed: {:?}",
        result.unwrap_err()
    );
    println!("sp operation_id: {}", result.unwrap());
}

#[tokio::test]
#[ignore = "requires Azure: set TENANT_ID, CLIENT_ID, CLIENT_SECRET, ASC_URL, SENDER, TO_EMAIL"]
async fn service_principal_get_email_status_after_send() {
    let tenant_id = require_env("TENANT_ID");
    let client_id = require_env("CLIENT_ID");
    let client_secret = require_env("CLIENT_SECRET");
    let asc_url = require_env("ASC_URL");
    let sender = require_env("SENDER");
    let to = require_env("TO_EMAIL");

    let client = ACSClientBuilder::new()
        .host(&asc_url)
        .service_principal(&tenant_id, &client_id, &client_secret)
        .max_retries(1)
        .build()
        .expect("ACSClientBuilder::build");

    let email = minimal_email(&sender, &to);
    let operation_id = client
        .send_email(&email)
        .await
        .expect("send_email should succeed");

    let status = client
        .get_email_status(&operation_id)
        .await
        .expect("get_email_status should succeed");
    println!("sp status after send: {}", status);
}

// ── Managed Identity auth (cloud-only) ───────────────────────────────────────

#[tokio::test]
#[ignore = "requires cloud environment with managed identity: set ASC_URL, SENDER, TO_EMAIL"]
async fn managed_identity_send_email_accepted() {
    let asc_url = require_env("ASC_URL");
    let sender = require_env("SENDER");
    let to = require_env("TO_EMAIL");

    let client = ACSClientBuilder::new()
        .host(&asc_url)
        .managed_identity()
        .max_retries(1)
        .build()
        .expect("ACSClientBuilder::build");

    let email = minimal_email(&sender, &to);
    let result = client.send_email(&email).await;

    assert!(
        result.is_ok(),
        "managed_identity send_email failed: {:?}",
        result.unwrap_err()
    );
    println!("mi operation_id: {}", result.unwrap());
}

// ── Clone / pool-friendly ─────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Azure: set CONNECTION_STR, SENDER, TO_EMAIL"]
async fn cloned_client_sends_independently() {
    let conn = require_env("CONNECTION_STR");
    let sender = require_env("SENDER");
    let to = require_env("TO_EMAIL");

    let client = ACSClientBuilder::new()
        .connection_string(&conn)
        .max_retries(1)
        .build()
        .expect("ACSClientBuilder::build");

    let c1 = client.clone();
    let c2 = client.clone();
    let email1 = minimal_email(&sender, &to);
    let email2 = minimal_email(&sender, &to);

    let (r1, r2) = tokio::join!(c1.send_email(&email1), c2.send_email(&email2));

    assert!(r1.is_ok(), "clone 1 send failed: {:?}", r1.unwrap_err());
    assert!(r2.is_ok(), "clone 2 send failed: {:?}", r2.unwrap_err());
    println!("clone1 op: {}, clone2 op: {}", r1.unwrap(), r2.unwrap());
}

// ── Optional: dotenv support for local runs ───────────────────────────────────

/// Call this at the top of any test that wants to load `.env` automatically.
/// It is a no-op if the file does not exist.
#[allow(dead_code)]
fn load_dotenv() {
    let _ = dotenv::dotenv();
}
