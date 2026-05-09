//! # azure-ecs-rs
//!
//! Rust client library for the [Azure Email Communication Service (ACS)] REST API.
//!
//! Supports sending emails and polling delivery status with three authentication
//! methods, configurable retry/timeout, async attachment loading, and structured
//! `tracing` telemetry — all over the ACS data-plane API.
//!
//! [Azure Email Communication Service (ACS)]: https://learn.microsoft.com/en-us/azure/communication-services/concepts/email/email-overview
//!
//! ## Quick start
//!
//! ```rust,no_run
//! use std::time::Duration;
//! use azure_ecs_rs::adapters::gateways::acs_email::{ACSApiVersion, ACSClientBuilder};
//! use azure_ecs_rs::domain::entities::models::{
//!     EmailAddress, EmailContent, Recipients, SentEmailBuilder,
//! };
//!
//! # #[tokio::main]
//! # async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let client = ACSClientBuilder::new()
//!     .connection_string("endpoint=https://...;accesskey=...")
//!     .timeout(Duration::from_secs(30))
//!     .max_retries(3)
//!     .build()?;
//!
//! let email = SentEmailBuilder::new()
//!     .sender("noreply@example.com".to_string())
//!     .content(EmailContent {
//!         subject: Some("Hello".to_string()),
//!         plain_text: Some("World".to_string()),
//!         html: None,
//!     })
//!     .recipients(Recipients {
//!         to: Some(vec![EmailAddress {
//!             email: Some("recipient@example.com".to_string()),
//!             display_name: Some("Recipient".to_string()),
//!         }]),
//!         cc: None,
//!         b_cc: None,
//!     })
//!     .build()?;
//!
//! let message_id = client.send_email(&email).await?;
//! println!("Accepted: {message_id}");
//! # Ok(())
//! # }
//! ```
//!
//! ## Authentication
//!
//! | Method | Builder call |
//! |---|---|
//! | Shared Key | `.connection_string("endpoint=...;accesskey=...")` |
//! | Service Principal | `.host(url).service_principal(tenant, client_id, secret)` |
//! | Managed Identity | `.managed_identity().host(url)` |
//!
//! ## Feature flags
//!
//! This crate has no optional Cargo features. All functionality is always available.
//!
//! ## API versions
//!
//! Select a version with `.api_version(ACSApiVersion::V20250901)` on the builder.
//! The default is `V20230331` for backward compatibility.

pub mod domain;

pub mod adapters {
    pub mod gateways {
        pub mod acs_email;
        mod acs_shared_key;
    }
}
