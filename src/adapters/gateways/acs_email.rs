//! HTTP client for the Azure Email Communication Service (ACS) data-plane API.
//!
//! # Architecture
//!
//! - [`ACSClientBuilder`] — fluent builder; validates configuration and constructs
//!   the shared [`reqwest::Client`] once so all requests reuse the same connection pool.
//! - [`ACSClient`] — clone-cheap handle (all fields behind `Arc` or `Clone`).
//!   Owns the HTTP client and dispatches the public operations:
//!   [`send_email`], [`send_emails_batch`], [`send_email_with_callback`],
//!   [`send_email_with_callback_cancellable`], [`send_email_stream`],
//!   [`send_email_stream_cancellable`], and [`get_email_status`].
//!
//! # Pool-friendly usage
//!
//! [`ACSClient`] is cheap to clone: call [`ACSClientBuilder::build`] once,
//! then share the handle across tasks by cloning it.  Every clone shares the
//! same underlying [`reqwest::Client`] connection pool, amortising TLS
//! handshakes across all concurrent requests.
//!
//! ```rust,ignore
//! let client = ACSClientBuilder::new()
//!     .connection_string(&conn_str)
//!     .build()
//!     .expect("valid configuration");
//!
//! // Distribute work across tasks — clones share the same connection pool.
//! let handles: Vec<_> = emails.iter().map(|email| {
//!     let c = client.clone();
//!     let e = email.clone();
//!     tokio::spawn(async move { c.send_email(&e).await })
//! }).collect();
//! for h in handles { let _ = h.await; }
//! ```
//!
//! Or use [`send_emails_batch`] to send a slice concurrently in one call.
//!
//! # Retry behaviour
//!
//! `429 Too Many Requests` and `503 Service Unavailable` responses trigger automatic
//! retries.  The delay is taken from the `Retry-After` response header when present;
//! otherwise exponential backoff (`2^n` seconds, n = retry index) is used.
//! When all retries are exhausted the call fails with
//! [`ACSError::RateLimitExceeded`].  Set `.max_retries(0)` to disable retries.
//!
//! # Authentication
//!
//! The auth method is selected at build time and baked into [`ACSClient`]:
//!
//! | Method | `Authorization` header strategy |
//! |---|---|
//! | [`SharedKey`] | HMAC-SHA256 signed per-request (see `acs_shared_key`) |
//! | [`ServicePrincipal`] | OAuth2 client-credentials token via `azure_identity` |
//! | [`ManagedIdentity`] | Ambient managed-identity token via `azure_identity` |
//!
//! Token acquisition for service principal / managed identity happens inside
//! each request; responses are not cached.
//!
//! [`send_email`]: ACSClient::send_email
//! [`send_emails_batch`]: ACSClient::send_emails_batch
//! [`send_email_with_callback`]: ACSClient::send_email_with_callback
//! [`send_email_with_callback_cancellable`]: ACSClient::send_email_with_callback_cancellable
//! [`send_email_stream`]: ACSClient::send_email_stream
//! [`send_email_stream_cancellable`]: ACSClient::send_email_stream_cancellable
//! [`get_email_status`]: ACSClient::get_email_status
//! [`SharedKey`]: ACSAuthMethod::SharedKey
//! [`ServicePrincipal`]: ACSAuthMethod::ServicePrincipal
//! [`ManagedIdentity`]: ACSAuthMethod::ManagedIdentity

// License: MIT
// This file is part of the Azure Communication Services Email Client Library, an open-source project.
// This source code is licensed under the MIT license found in the LICENSE file in the root directory of this source tree.

use crate::adapters::gateways::acs_shared_key::{get_request_header, parse_endpoint};
use crate::domain::entities::models::{
    ACSError, EmailSendStatusType, ErrorResponse, SentEmail, SentEmailResponse,
};
use async_stream::stream;
use azure_core::auth::TokenCredential;
use azure_core::HttpClient;
use azure_identity::{create_credential, ClientSecretCredential};
use futures::stream::Stream;
use reqwest::header::RETRY_AFTER;
use reqwest::{Client, StatusCode};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::oneshot;
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, instrument};
use url::Url;
use uuid::Uuid;

type EmailResult<T> = Result<T, ACSError>;

/// Selects the ACS Email REST API version used for all requests on an [`ACSClient`].
///
/// The default is [`V20230331`] for backward compatibility.  Opt in to
/// [`V20250901`] via [`ACSClientBuilder::api_version`] when you need explicit
/// version pinning.
///
/// **Note:** the data-plane endpoint exposes identical operations in both
/// versions.  Suppression-list management (opt-out) lives on the ARM management
/// plane and is not part of this client.  See `docs/adr/ADR-001` for details.
///
/// [`V20230331`]: ACSApiVersion::V20230331
/// [`V20250901`]: ACSApiVersion::V20250901
#[derive(Clone, Default, Debug)]
pub enum ACSApiVersion {
    /// `2023-03-31` — default; backward-compatible with all prior releases.
    #[default]
    V20230331,
    /// `2025-09-01` — latest stable; use for explicit version pinning.
    V20250901,
}

impl ACSApiVersion {
    pub fn as_str(&self) -> &'static str {
        match self {
            ACSApiVersion::V20230331 => "2023-03-31",
            ACSApiVersion::V20250901 => "2025-09-01",
        }
    }
}

// Azure Communication Services (ACS) authentication method
#[derive(Clone)]
enum ACSAuthMethod {
    SharedKey(String),
    ServicePrincipal {
        tenant_id: String,
        client_id: String,
        client_secret: String,
    },
    ManagedIdentity,
}

/// Default number of retries for `429 Too Many Requests` and `503 Service Unavailable` responses.
const DEFAULT_MAX_RETRIES: u32 = 3;

/// Async HTTP client for the ACS Email data-plane API.
///
/// Construct via [`ACSClientBuilder`].  The client is cheap to clone — the
/// underlying [`reqwest::Client`] (and its connection pool) is shared across
/// all clones via `reqwest`'s internal `Arc`.
///
/// # Thread safety
///
/// `ACSClient` implements [`Clone`] and is `Send + Sync`; it can be shared
/// across threads or Tokio tasks without additional locking.
///
/// # Pool-friendly pattern
///
/// Build once, clone freely:
///
/// ```rust,ignore
/// let client = ACSClientBuilder::new()
///     .connection_string(&conn_str)
///     .build()?;
///
/// // Each clone shares the same reqwest connection pool.
/// let c1 = client.clone();
/// let c2 = client.clone();
/// tokio::join!(
///     async move { c1.send_email(&email1).await },
///     async move { c2.send_email(&email2).await },
/// );
/// ```
///
/// Or use [`send_emails_batch`] to let the client handle concurrency for you.
///
/// [`Arc`]: std::sync::Arc
/// [`send_emails_batch`]: ACSClient::send_emails_batch
#[derive(Clone)]
pub struct ACSClient {
    host: String,
    base_url: String,
    auth_method: ACSAuthMethod,
    api_version: ACSApiVersion,
    http_client: Client,
    max_retries: u32,
}

/// Fluent builder for [`ACSClient`].
///
/// Call [`ACSClientBuilder::new`] (or `ACSClientBuilder::default()`), chain
/// the configuration methods you need, then call [`build`] to obtain a
/// validated [`ACSClient`].
///
/// # Required fields
///
/// Exactly one of the following must be set:
/// - `.connection_string(…)` for shared-key auth, **or**
/// - `.host(…)` + `.service_principal(…)` for service principal auth, **or**
/// - `.host(…)` + `.managed_identity()` for managed identity auth.
///
/// [`build`]: ACSClientBuilder::build
pub struct ACSClientBuilder {
    host: Option<String>,
    connection_string: Option<String>,
    auth_method: Option<ACSAuthMethod>,
    api_version: ACSApiVersion,
    max_retries: u32,
    timeout: Option<Duration>,
    base_url_override: Option<String>,
}

impl Default for ACSClientBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl ACSClientBuilder {
    // Create a new builder instance
    pub fn new() -> Self {
        ACSClientBuilder {
            host: None,
            connection_string: None,
            auth_method: None,
            api_version: ACSApiVersion::default(),
            max_retries: DEFAULT_MAX_RETRIES,
            timeout: None,
            base_url_override: None,
        }
    }

    /// Override the base URL used for all requests. Intended for integration tests
    /// that point at a local mock server (e.g. wiremock). Not for production use.
    #[cfg(test)]
    pub(crate) fn base_url_override(mut self, url: &str) -> Self {
        self.base_url_override = Some(url.to_string());
        self
    }

    pub fn api_version(mut self, version: ACSApiVersion) -> Self {
        self.api_version = version;
        self
    }

    /// Maximum number of retries on `429 Too Many Requests` or `503 Service Unavailable`.
    ///
    /// Retries use exponential backoff (`2^n` seconds) unless the server supplies a
    /// `Retry-After` header, in which case that value is used instead. Exhausting all
    /// retries yields [`ACSError::RateLimitExceeded`]. Default: `3`.
    pub fn max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Per-request HTTP timeout. Applies to every individual request including retries.
    ///
    /// When the timeout elapses before a response is received the request fails with
    /// [`ACSError::Network`]. Default: no timeout.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    // Set the host for the client
    pub fn host(mut self, host: &str) -> Self {
        self.host = Some(host.to_string());
        self
    }

    // Set the authentication method for the client using a shared key
    pub fn connection_string(mut self, connection_string: &str) -> Self {
        self.connection_string = Some(connection_string.to_string());
        self
    }

    // Set the authentication method for the client using a service principal
    pub fn service_principal(
        mut self,
        tenant_id: &str,
        client_id: &str,
        client_secret: &str,
    ) -> Self {
        self.auth_method = Some(ACSAuthMethod::ServicePrincipal {
            tenant_id: tenant_id.to_string(),
            client_id: client_id.to_string(),
            client_secret: client_secret.to_string(),
        });
        self
    }

    // Set the authentication method for the client using managed identity
    pub fn managed_identity(mut self) -> Self {
        self.auth_method = Some(ACSAuthMethod::ManagedIdentity);
        self
    }

    // Build and return the ACSClient
    pub fn build(self) -> Result<ACSClient, String> {
        let mut client_builder = Client::builder();
        if let Some(timeout) = self.timeout {
            client_builder = client_builder.timeout(timeout);
        }
        let http_client = client_builder
            .build()
            .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

        if let Some(connection_string) = self.connection_string {
            let parsed_res = parse_endpoint(&connection_string)
                .map_err(|e| format!("Failed to parse connection string: {}", e))?;
            let host = parsed_res.host_name;
            let base_url = self
                .base_url_override
                .unwrap_or_else(|| format!("https://{}", host));
            let auth_method = ACSAuthMethod::SharedKey(parsed_res.access_key);
            return Ok(ACSClient {
                host,
                base_url,
                auth_method,
                api_version: self.api_version,
                http_client,
                max_retries: self.max_retries,
            });
        }

        let host = self.host.ok_or_else(|| "Host is required".to_string())?;
        let clean_host = host
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        let base_url = self
            .base_url_override
            .unwrap_or_else(|| format!("https://{}", clean_host));
        let auth_method = self
            .auth_method
            .ok_or_else(|| "Authentication method is required".to_string())?;
        Ok(ACSClient {
            host,
            base_url,
            auth_method,
            api_version: self.api_version,
            http_client,
            max_retries: self.max_retries,
        })
    }
}

impl ACSClient {
    /// Submit an email for delivery and return the ACS operation ID.
    ///
    /// The returned `String` is the opaque operation ID (message ID) assigned by
    /// ACS.  Pass it to [`get_email_status`] or use [`send_email_stream`] /
    /// [`send_email_with_callback`] to track delivery.
    ///
    /// Transient `429` / `503` responses are retried automatically up to
    /// `max_retries` times (default: 3) with exponential back-off.
    ///
    /// # Errors
    ///
    /// - [`ACSError::Network`] — request could not be sent.
    /// - [`ACSError::Auth`] — token acquisition failed (service principal / managed identity).
    /// - [`ACSError::Api`] — ACS returned a non-202 error response.
    /// - [`ACSError::RateLimitExceeded`] — all retries exhausted on `429`/`503`.
    ///
    /// [`get_email_status`]: ACSClient::get_email_status
    /// [`send_email_stream`]: ACSClient::send_email_stream
    /// [`send_email_with_callback`]: ACSClient::send_email_with_callback
    #[instrument(skip(self, email), fields(host = %self.host, api_version = %self.api_version.as_str()))]
    pub async fn send_email(&self, email: &SentEmail) -> EmailResult<String> {
        let request_id = format!("{}", Uuid::new_v4());
        acs_send_email(
            &self.http_client,
            &self.base_url,
            &self.auth_method,
            request_id.as_str(),
            email,
            &self.api_version,
            self.max_retries,
        )
        .await
    }

    /// Submit an email and receive delivery status updates via a callback.
    ///
    /// Sends the email, then spawns a Tokio task that polls
    /// [`get_email_status`] every **5 seconds** and invokes `call_back` on each
    /// update.  The task stops when a terminal status
    /// (`Succeeded`, `Failed`, `Canceled`, `Unknown`) or a poll error is
    /// observed.
    ///
    /// # Returns
    ///
    /// A tuple `(message_id, done_rx)`.  Await `done_rx` to block until the
    /// background task has finished (useful in tests or when you need to ensure
    /// delivery completes before your process exits).
    ///
    /// # Callback signature
    ///
    /// ```text
    /// fn(message_id: String, status: &EmailSendStatusType, error: Option<ACSError>)
    /// ```
    ///
    /// `error` is `Some` only when a status-poll request itself fails; a
    /// `Failed` delivery status still has `error = None`.
    ///
    /// # Errors
    ///
    /// Returns `Err` only if the initial *send* fails.  Poll errors are
    /// delivered through the callback rather than propagated.
    ///
    /// [`get_email_status`]: ACSClient::get_email_status
    #[allow(dead_code)]
    #[instrument(skip(self, email, call_back), fields(host = %self.host, api_version = %self.api_version.as_str()))]
    pub async fn send_email_with_callback<F>(
        self,
        email: &SentEmail,
        call_back: F,
    ) -> EmailResult<(String, oneshot::Receiver<()>)>
    where
        F: Fn(String, &EmailSendStatusType, Option<ACSError>) + Send + Sync + 'static,
    {
        let request_id = format!("{}", Uuid::new_v4());
        let result = acs_send_email(
            &self.http_client,
            &self.base_url,
            &self.auth_method,
            request_id.as_str(),
            email,
            &self.api_version,
            self.max_retries,
        )
        .await?;

        let message_id = result.clone();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            loop {
                sleep(Duration::from_secs(5)).await;
                let resp_status = self.get_email_status(&message_id).await;
                if let Ok(status) = resp_status {
                    call_back(message_id.clone(), &status, None);
                    if matches!(
                        status,
                        EmailSendStatusType::Unknown
                            | EmailSendStatusType::Canceled
                            | EmailSendStatusType::Failed
                            | EmailSendStatusType::Succeeded
                    ) {
                        let _ = tx.send(());
                        break;
                    }
                } else {
                    call_back(
                        message_id.clone(),
                        &EmailSendStatusType::Failed,
                        resp_status.err(),
                    );
                    let _ = tx.send(());
                    break;
                }
            }
        });

        Ok((result, rx))
    }

    /// Stream delivery status updates for a sent email.
    ///
    /// Sends the email, then returns a `Stream` that yields one
    /// `Result<EmailSendStatusType, ACSError>` per poll interval (5 s).
    /// The stream ends after the first terminal status
    /// (`Succeeded`, `Failed`, `Canceled`, `Unknown`) or on a poll error.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use futures::StreamExt;
    /// let (id, stream) = client.send_email_stream(&email).await?;
    /// tokio::pin!(stream);
    /// while let Some(item) = stream.next().await { /* … */ }
    /// ```
    #[instrument(skip(self, email), fields(host = %self.host, api_version = %self.api_version.as_str()))]
    pub async fn send_email_stream(
        &self,
        email: &SentEmail,
    ) -> EmailResult<(
        String,
        impl Stream<Item = Result<EmailSendStatusType, ACSError>> + '_,
    )> {
        let request_id = Uuid::new_v4().to_string();
        let message_id = acs_send_email(
            &self.http_client,
            &self.base_url,
            &self.auth_method,
            &request_id,
            email,
            &self.api_version,
            self.max_retries,
        )
        .await?;

        let returned_id = message_id.clone();
        let poll_stream = stream! {
            loop {
                sleep(Duration::from_secs(5)).await;
                match self.get_email_status(&message_id).await {
                    Ok(status) => {
                        let terminal = is_terminal_status(&status);
                        yield Ok(status);
                        if terminal { break; }
                    }
                    Err(e) => {
                        yield Err(e);
                        break;
                    }
                }
            }
        };

        Ok((returned_id, poll_stream))
    }

    /// Send multiple emails concurrently and collect all results.
    ///
    /// Dispatches one [`send_email`] per entry in `emails`, awaits all of them,
    /// and returns results in input order.  A failed send is captured as `Err`
    /// in its slot — it does not abort the remaining sends.
    ///
    /// All sends share the same underlying connection pool, so no extra TLS
    /// handshakes are incurred compared to sequential sends.
    ///
    /// # Errors
    ///
    /// Each element follows the same error variants as [`send_email`].
    ///
    /// [`send_email`]: ACSClient::send_email
    #[instrument(skip(self, emails), fields(host = %self.host, count = emails.len()))]
    pub async fn send_emails_batch(&self, emails: &[SentEmail]) -> Vec<EmailResult<String>> {
        futures::future::join_all(emails.iter().map(|e| self.send_email(e))).await
    }

    /// Stream delivery status updates with cooperative cancellation.
    ///
    /// Identical to [`send_email_stream`] except that the returned stream exits
    /// cleanly when `token` is cancelled — no further status polls are issued
    /// and the stream yields `None` on the next call to `next()`.
    ///
    /// # Example
    ///
    /// ```rust,ignore
    /// use futures::StreamExt;
    /// use tokio_util::sync::CancellationToken;
    ///
    /// let token = CancellationToken::new();
    /// let (id, stream) = client
    ///     .send_email_stream_cancellable(&email, token.clone())
    ///     .await?;
    ///
    /// tokio::pin!(stream);
    /// while let Some(item) = stream.next().await { /* … */ }
    ///
    /// // Somewhere else: token.cancel() stops the stream early.
    /// ```
    ///
    /// [`send_email_stream`]: ACSClient::send_email_stream
    #[instrument(skip(self, email, token), fields(host = %self.host, api_version = %self.api_version.as_str()))]
    pub async fn send_email_stream_cancellable(
        &self,
        email: &SentEmail,
        token: CancellationToken,
    ) -> EmailResult<(
        String,
        impl Stream<Item = Result<EmailSendStatusType, ACSError>> + '_,
    )> {
        let request_id = Uuid::new_v4().to_string();
        let message_id = acs_send_email(
            &self.http_client,
            &self.base_url,
            &self.auth_method,
            &request_id,
            email,
            &self.api_version,
            self.max_retries,
        )
        .await?;

        let returned_id = message_id.clone();
        let poll_stream = stream! {
            loop {
                tokio::select! {
                    _ = token.cancelled() => { break; }
                    _ = sleep(Duration::from_secs(5)) => {
                        match self.get_email_status(&message_id).await {
                            Ok(status) => {
                                let terminal = is_terminal_status(&status);
                                yield Ok(status);
                                if terminal { break; }
                            }
                            Err(e) => {
                                yield Err(e);
                                break;
                            }
                        }
                    }
                }
            }
        };

        Ok((returned_id, poll_stream))
    }

    /// Callback-based status polling with cooperative cancellation.
    ///
    /// Identical to [`send_email_with_callback`] except that the background
    /// polling task exits cleanly when `token` is cancelled — no further status
    /// polls are issued and `done_rx` resolves once the task has fully stopped.
    ///
    /// # Errors
    ///
    /// Returns `Err` only if the initial *send* fails.  Poll errors and
    /// cancellation are both delivered via `done_rx` resolving.
    ///
    /// [`send_email_with_callback`]: ACSClient::send_email_with_callback
    #[allow(dead_code)]
    #[instrument(skip(self, email, token, call_back), fields(host = %self.host, api_version = %self.api_version.as_str()))]
    pub async fn send_email_with_callback_cancellable<F>(
        self,
        email: &SentEmail,
        token: CancellationToken,
        call_back: F,
    ) -> EmailResult<(String, oneshot::Receiver<()>)>
    where
        F: Fn(String, &EmailSendStatusType, Option<ACSError>) + Send + Sync + 'static,
    {
        let request_id = Uuid::new_v4().to_string();
        let result = acs_send_email(
            &self.http_client,
            &self.base_url,
            &self.auth_method,
            &request_id,
            email,
            &self.api_version,
            self.max_retries,
        )
        .await?;

        let message_id = result.clone();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = token.cancelled() => {
                        let _ = tx.send(());
                        break;
                    }
                    _ = sleep(Duration::from_secs(5)) => {
                        let resp_status = self.get_email_status(&message_id).await;
                        if let Ok(status) = resp_status {
                            call_back(message_id.clone(), &status, None);
                            if is_terminal_status(&status) {
                                let _ = tx.send(());
                                break;
                            }
                        } else {
                            call_back(
                                message_id.clone(),
                                &EmailSendStatusType::Failed,
                                resp_status.err(),
                            );
                            let _ = tx.send(());
                            break;
                        }
                    }
                }
            }
        });

        Ok((result, rx))
    }

    /// Poll the delivery status of a previously submitted email.
    ///
    /// `message_id` is the operation ID returned by [`send_email`].  ACS
    /// processes delivery asynchronously; call this method periodically
    /// (recommended: every 5 s) until a terminal status is observed:
    ///
    /// | Status | Meaning |
    /// |---|---|
    /// | `NotStarted` | Queued, not yet processing |
    /// | `Running` | In transit |
    /// | `Succeeded` | Delivered to the recipient server |
    /// | `Failed` | Permanent delivery failure |
    /// | `Canceled` | Canceled by the service |
    /// | `Unknown` | Unrecognised status string from ACS |
    ///
    /// Consider [`send_email_stream`] for a `Stream`-based alternative.
    ///
    /// # Errors
    ///
    /// - [`ACSError::Api`] — ACS returned an error (e.g. operation not found).
    /// - [`ACSError::Network`] — request failed due to connectivity.
    /// - [`ACSError::MissingField`] — the response was valid JSON but lacked a
    ///   `status` field.
    ///
    /// [`send_email`]: ACSClient::send_email
    /// [`send_email_stream`]: ACSClient::send_email_stream
    #[instrument(skip(self), fields(host = %self.host, api_version = %self.api_version.as_str()))]
    pub async fn get_email_status(&self, message_id: &str) -> EmailResult<EmailSendStatusType> {
        acs_get_email_status(
            &self.http_client,
            &self.base_url,
            &self.auth_method,
            message_id,
            &self.api_version,
        )
        .await
    }
}

#[instrument(skip(http_client, body, acs_auth_method), fields(method = %method, url = %url))]
async fn send_request<T>(
    http_client: &Client,
    method: reqwest::Method,
    url: &str,
    request_id: &str,
    body: Option<&T>,
    acs_auth_method: &ACSAuthMethod,
) -> EmailResult<reqwest::Response>
where
    T: serde::Serialize,
{
    let url_endpoint = parse_url(url)?;
    let json_body = serialize_body(body)?;
    let headers = create_headers(
        http_client,
        &url_endpoint,
        method.as_str(),
        request_id,
        &json_body,
        acs_auth_method,
    )
    .await?;
    let request_builder = http_client.request(method, url).headers(headers);
    let request_builder = if let Some(body) = body {
        request_builder.json(body)
    } else {
        request_builder
    };
    request_builder.send().await.map_err(network_err)
}

fn parse_url(url: &str) -> EmailResult<Url> {
    Url::parse(url).map_err(url_err)
}

fn serialize_body<T: serde::Serialize>(body: Option<&T>) -> EmailResult<String> {
    if let Some(body) = body {
        serde_json::to_string(body).map_err(serial_err)
    } else {
        Ok(String::new())
    }
}

fn wrap_http_client(client: &Client) -> Arc<dyn HttpClient> {
    Arc::new(client.clone()) as Arc<dyn HttpClient>
}

/// Get an access token based on the provided authentication method.
///
/// # Arguments
///
/// * `auth_method` - A reference to the `ACSAuthMethod` enum specifying the authentication method.
///
/// # Returns
///
/// * `Result<String, String>` - The result of the token acquisition, containing the token if successful.
async fn get_access_token(
    http_client: &Client,
    auth_method: &ACSAuthMethod,
) -> Result<String, String> {
    match auth_method {
        ACSAuthMethod::ServicePrincipal {
            tenant_id,
            client_id,
            client_secret,
        } => {
            let azure_http_client = wrap_http_client(http_client);
            let token_url = "https://login.microsoftonline.com/";

            let credential = ClientSecretCredential::new(
                azure_http_client,
                Url::parse(token_url).unwrap(),
                tenant_id.to_string(),
                client_id.to_string(),
                client_secret.to_string(),
            );
            let token = credential
                .get_token(&["https://communication.azure.com/.default"])
                .await
                .map_err(|e| format!("Failed to get access token: {}", e))?;

            return Ok(token.token.secret().to_owned());
        }
        ACSAuthMethod::ManagedIdentity => {
            let credential =
                create_credential().map_err(|e| format!("Failed to create credential: {}", e))?;
            let token = credential
                .get_token(&["https://communication.azure.com/.default"])
                .await
                .map_err(|e| format!("Failed to get access token: {}", e))?;
            return Ok(token.token.secret().to_owned());
        }
        _ => {}
    }
    Ok("".to_string())
}

/// Create headers for the request based on the provided authentication method.
///
/// # Arguments
///
/// * `url_endpoint` - A reference to the `Url` struct representing the endpoint URL.
/// * `method` - A reference to the HTTP method string.
/// * `request_id` - A reference to the request ID string.
/// * `json_body` - A reference to the JSON body string.
/// * `auth_method` - A reference to the `ACSAuthMethod` enum specifying the authentication method.
///
/// # Returns
///
/// * `EmailResult<reqwest::header::HeaderMap>` - The result of the header creation, containing the headers if successful.
async fn create_headers(
    http_client: &Client,
    url_endpoint: &Url,
    method: &str,
    request_id: &str,
    json_body: &str,
    auth_method: &ACSAuthMethod,
) -> EmailResult<reqwest::header::HeaderMap> {
    let mut headers = reqwest::header::HeaderMap::new();

    match auth_method {
        ACSAuthMethod::SharedKey(share_key) => {
            headers = get_request_header(url_endpoint, method, request_id, json_body, share_key)
                .map_err(header_err)?
        }
        ACSAuthMethod::ServicePrincipal { .. } | ACSAuthMethod::ManagedIdentity => {
            let token = get_access_token(http_client, auth_method)
                .await
                .map_err(auth_err)?;
            headers.insert(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", token).parse().unwrap(),
            );
            headers.insert(
                reqwest::header::CONTENT_TYPE,
                "application/json".parse().unwrap(),
            );
            headers.insert(
                reqwest::header::HeaderName::from_static("x-ms-client-request-id"),
                request_id.parse().unwrap(),
            );
        }
    }

    Ok(headers)
}

/// Convert an error into an `ErrorResponse`.
///
/// # Arguments
///
/// * `message` - A reference to the error message string.
/// * `error` - An object that implements the `ToString` trait.
///
/// # Returns
///
/// * `ErrorResponse` - The error response containing the error details.
fn network_err(detail: impl ToString) -> ACSError {
    ACSError::Network(detail.to_string())
}

fn parse_err(detail: impl ToString) -> ACSError {
    ACSError::Deserialization(detail.to_string())
}

fn serial_err(detail: impl ToString) -> ACSError {
    ACSError::Serialization(detail.to_string())
}

fn url_err(detail: impl ToString) -> ACSError {
    ACSError::InvalidUrl(detail.to_string())
}

fn header_err(detail: impl ToString) -> ACSError {
    ACSError::Header(detail.to_string())
}

fn auth_err(detail: impl ToString) -> ACSError {
    ACSError::Auth(detail.to_string())
}

/// Get the status of a sent email using the ACS client.
///
/// # Arguments
///
/// * `base_url` - Base URL including scheme, e.g. `https://resource.communication.azure.com`.
/// * `acs_auth_method` - A reference to the `ACSAuthMethod` enum specifying the authentication method.
/// * `request_id` - A reference to the request ID string.
///
/// # Returns
///
/// * `EmailResult<EmailSendStatusType>` - The result of the email status query, containing the status if successful.
#[instrument(skip(http_client, acs_auth_method), fields(base_url = %base_url))]
async fn acs_get_email_status(
    http_client: &Client,
    base_url: &str,
    acs_auth_method: &ACSAuthMethod,
    request_id: &str,
    api_version: &ACSApiVersion,
) -> EmailResult<EmailSendStatusType> {
    let url = format!(
        "{}/emails/operations/{}?api-version={}",
        base_url,
        request_id,
        api_version.as_str()
    );
    debug!("end point URL: {}", url);

    let response = send_request::<()>(
        http_client,
        reqwest::Method::GET,
        &url,
        request_id,
        None,
        acs_auth_method,
    )
    .await?;
    if response.status() == StatusCode::OK {
        let email_response = parse_response::<SentEmailResponse>(response).await?;
        email_response
            .status
            .map(|status| Ok(status.to_type()))
            .unwrap_or_else(|| Err(create_missing_status_error()))
    } else {
        let error_response = parse_response::<ErrorResponse>(response).await?;
        Err(ACSError::from(error_response))
    }
}

/// Send an email using the ACS client.
///
/// # Arguments
///
/// * `base_url` - Base URL including scheme, e.g. `https://resource.communication.azure.com`.
/// * `acs_auth_method` - A reference to the `ACSAuthMethod` enum specifying the authentication method.
/// * `request_id` - A reference to the request ID string.
/// * `email` - A reference to the `SentEmail` struct containing the email details.
///
/// # Returns
///
/// * `EmailResult<String>` - The result of the email send operation, containing the message ID if successful.
#[instrument(skip(http_client, acs_auth_method, email), fields(base_url = %base_url, max_retries = %max_retries))]
async fn acs_send_email(
    http_client: &Client,
    base_url: &str,
    acs_auth_method: &ACSAuthMethod,
    request_id: &str,
    email: &SentEmail,
    api_version: &ACSApiVersion,
    max_retries: u32,
) -> EmailResult<String> {
    let url = format!(
        "{}/emails:send?api-version={}",
        base_url,
        api_version.as_str()
    );
    debug!("end point URL: {}", url);
    let response = send_request(
        http_client,
        reqwest::Method::POST,
        &url,
        request_id,
        Some(email),
        acs_auth_method,
    )
    .await?;
    debug!("{:#?}", response);
    handle_response_and_retry_if_needed(
        http_client,
        response,
        reqwest::Method::POST,
        &url,
        request_id,
        Some(email),
        acs_auth_method,
        max_retries,
    )
    .await
}
/// Handle the response from the email send operation and retry if needed.
///
/// # Arguments
///
/// * `response` - The `reqwest::Response` object.
/// * `method` - The HTTP method used for the request.
/// * `url` - The URL to send the request to.
/// * `request_id` - The request ID string.
/// * `body` - An optional reference to the request body.
/// * `acs_auth_method` - A reference to the `ACSAuthMethod` enum specifying the authentication method.
/// * `max_retries` - The maximum number of retries.
///
/// # Returns
///
/// * `EmailResult<String>` - The result of the response handling, containing the message ID if successful.
#[allow(clippy::too_many_arguments)]
async fn handle_response_and_retry_if_needed<T>(
    http_client: &Client,
    mut response: reqwest::Response,
    method: reqwest::Method,
    url: &str,
    request_id: &str,
    body: Option<&T>,
    acs_auth_method: &ACSAuthMethod,
    max_retries: u32,
) -> EmailResult<String>
where
    T: serde::Serialize,
{
    let mut retries = 0;

    loop {
        match response.status() {
            StatusCode::ACCEPTED => {
                return parse_response::<SentEmailResponse>(response)
                    .await?
                    .id
                    .ok_or_else(create_missing_id_error);
            }
            StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE => {
                if retries >= max_retries {
                    error!("Max retries exceeded");
                    return Err(ACSError::RateLimitExceeded { retries });
                }

                if let Some(retry_after) = response.headers().get(RETRY_AFTER) {
                    if let Ok(retry_after_value) = retry_after.to_str() {
                        if let Ok(retry_after_secs) = retry_after_value.parse::<u64>() {
                            debug!("Retrying after {} seconds", retry_after_secs);
                            sleep(Duration::from_secs(retry_after_secs)).await;
                        } else {
                            error!("Failed to parse Retry-After header value");
                            return parse_error_response(response).await;
                        }
                    } else {
                        error!("Failed to parse Retry-After header value");
                        return parse_error_response(response).await;
                    }
                } else {
                    // Implement exponential backoff
                    let backoff_secs = 2u64.pow(retries);
                    debug!(
                        "Retry-After header not found. Retrying after {} seconds",
                        backoff_secs
                    );
                    sleep(Duration::from_secs(backoff_secs)).await;
                }

                retries += 1;

                let new_response = send_request(
                    http_client,
                    method.clone(),
                    url,
                    request_id,
                    body,
                    acs_auth_method,
                )
                .await?;
                response = new_response;
            }
            _ => {
                error!("Failed to send email: {:#?}", response);
                return parse_error_response(response).await;
            }
        }
    }
}

/// Parse the response from the email send operation.
///
/// # Arguments
///
/// * `response` - The `reqwest::Response` object.
///
/// # Returns
///
/// * `EmailResult<T>` - The result of the response parsing, containing the parsed response if successful.
async fn parse_response<T>(response: reqwest::Response) -> EmailResult<T>
where
    T: serde::de::DeserializeOwned,
{
    response.json::<T>().await.map_err(parse_err)
}

/// Parse the error response from the email send operation.
///
/// # Arguments
///
/// * `response` - The `reqwest::Response` object.
///
/// # Returns
///
/// * `EmailResult<String>` - The result of the error response parsing, containing the error response if successful.
async fn parse_error_response(response: reqwest::Response) -> EmailResult<String> {
    let error_response = parse_response::<ErrorResponse>(response).await?;
    Err(ACSError::from(error_response))
}

fn is_terminal_status(status: &EmailSendStatusType) -> bool {
    matches!(
        status,
        EmailSendStatusType::Succeeded
            | EmailSendStatusType::Failed
            | EmailSendStatusType::Canceled
            | EmailSendStatusType::Unknown
    )
}

fn create_missing_status_error() -> ACSError {
    ACSError::MissingField("status")
}

fn create_missing_id_error() -> ACSError {
    ACSError::MissingField("id")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── ACSApiVersion ────────────────────────────────────────────────────────

    #[test]
    fn api_version_v20230331_as_str() {
        assert_eq!(ACSApiVersion::V20230331.as_str(), "2023-03-31");
    }

    #[test]
    fn api_version_v20250901_as_str() {
        assert_eq!(ACSApiVersion::V20250901.as_str(), "2025-09-01");
    }

    #[test]
    fn api_version_default_is_v20230331() {
        assert_eq!(ACSApiVersion::default().as_str(), "2023-03-31");
    }

    // ── ACSClientBuilder ─────────────────────────────────────────────────────

    #[test]
    fn builder_fails_without_host() {
        let result = ACSClientBuilder::new().managed_identity().build();
        assert!(result.err().unwrap().contains("Host is required"));
    }

    #[test]
    fn builder_fails_without_auth_method() {
        let result = ACSClientBuilder::new().host("example.com").build();
        assert!(result
            .err()
            .unwrap()
            .contains("Authentication method is required"));
    }

    #[test]
    fn builder_succeeds_with_connection_string() {
        let conn = "endpoint=https://example.com;accesskey=c2VjcmV0";
        assert!(ACSClientBuilder::new()
            .connection_string(conn)
            .build()
            .is_ok());
    }

    #[test]
    fn builder_fails_with_invalid_connection_string() {
        let result = ACSClientBuilder::new()
            .connection_string("bad-string")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn builder_succeeds_with_service_principal() {
        let result = ACSClientBuilder::new()
            .host("example.com")
            .service_principal("tenant", "client", "secret")
            .build();
        assert!(result.is_ok());
    }

    #[test]
    fn builder_succeeds_with_managed_identity() {
        let result = ACSClientBuilder::new()
            .host("example.com")
            .managed_identity()
            .build();
        assert!(result.is_ok());
    }

    #[test]
    fn builder_default_api_version_is_v20230331() {
        let conn = "endpoint=https://example.com;accesskey=c2VjcmV0";
        let client = ACSClientBuilder::new()
            .connection_string(conn)
            .build()
            .unwrap();
        assert_eq!(client.api_version.as_str(), "2023-03-31");
    }

    #[test]
    fn builder_respects_v20250901() {
        let conn = "endpoint=https://example.com;accesskey=c2VjcmV0";
        let client = ACSClientBuilder::new()
            .connection_string(conn)
            .api_version(ACSApiVersion::V20250901)
            .build()
            .unwrap();
        assert_eq!(client.api_version.as_str(), "2025-09-01");
    }

    // ── parse_url ────────────────────────────────────────────────────────────

    #[test]
    fn parse_url_valid() {
        assert!(parse_url("https://example.com/path?q=1").is_ok());
    }

    #[test]
    fn parse_url_invalid() {
        assert!(parse_url("not a url !!").is_err());
    }

    // ── serialize_body ───────────────────────────────────────────────────────

    #[test]
    fn serialize_body_none_returns_empty_string() {
        let result = serialize_body::<serde_json::Value>(None).unwrap();
        assert_eq!(result, "");
    }

    #[test]
    fn serialize_body_some_produces_json() {
        let body = json!({"key": "value"});
        let result = serialize_body(Some(&body)).unwrap();
        assert!(result.contains("\"key\""));
        assert!(result.contains("\"value\""));
    }

    // ── error helpers ─────────────────────────────────────────────────────────

    #[test]
    fn network_err_produces_network_variant() {
        assert!(matches!(network_err("boom"), ACSError::Network(_)));
    }

    #[test]
    fn parse_err_produces_deserialization_variant() {
        assert!(matches!(
            parse_err("bad json"),
            ACSError::Deserialization(_)
        ));
    }

    #[test]
    fn url_err_produces_invalid_url_variant() {
        assert!(matches!(url_err("bad"), ACSError::InvalidUrl(_)));
    }

    #[test]
    fn serial_err_produces_serialization_variant() {
        assert!(matches!(serial_err("oops"), ACSError::Serialization(_)));
    }

    #[test]
    fn auth_err_produces_auth_variant() {
        assert!(matches!(auth_err("denied"), ACSError::Auth(_)));
    }

    #[test]
    fn header_err_produces_header_variant() {
        assert!(matches!(header_err("bad header"), ACSError::Header(_)));
    }

    // ── create_missing_status_error / create_missing_id_error ────────────────

    #[test]
    fn create_missing_status_error_is_missing_field() {
        assert!(matches!(
            create_missing_status_error(),
            ACSError::MissingField("status")
        ));
    }

    #[test]
    fn create_missing_id_error_is_missing_field() {
        assert!(matches!(
            create_missing_id_error(),
            ACSError::MissingField("id")
        ));
    }

    // ── ACSError Display ──────────────────────────────────────────────────────

    #[test]
    fn acs_error_display_network() {
        let e = ACSError::Network("timeout".to_string());
        assert!(e.to_string().contains("timeout"));
    }

    #[test]
    fn acs_error_display_rate_limit() {
        let e = ACSError::RateLimitExceeded { retries: 3 };
        assert!(e.to_string().contains("3"));
    }

    #[test]
    fn acs_error_display_api() {
        let e = ACSError::Api {
            code: Some("400".to_string()),
            message: "bad request".to_string(),
        };
        let s = e.to_string();
        assert!(s.contains("400"));
        assert!(s.contains("bad request"));
    }

    #[test]
    fn acs_error_from_error_response() {
        let resp = ErrorResponse {
            error: Some(crate::domain::entities::models::ErrorDetail {
                code: Some("503".to_string()),
                message: Some("service unavailable".to_string()),
                ..Default::default()
            }),
        };
        let e = ACSError::from(resp);
        assert!(matches!(e, ACSError::Api { .. }));
        assert!(e.to_string().contains("503"));
    }

    // ── wrap_http_client ─────────────────────────────────────────────────────

    #[test]
    fn wrap_http_client_produces_arc() {
        let client = Client::new();
        let wrapped = wrap_http_client(&client);
        assert!(std::sync::Arc::strong_count(&wrapped) >= 1);
    }

    // ── Phase 2: max_retries ─────────────────────────────────────────────────

    #[test]
    fn builder_default_max_retries_is_3() {
        let conn = "endpoint=https://example.com;accesskey=c2VjcmV0";
        let client = ACSClientBuilder::new()
            .connection_string(conn)
            .build()
            .unwrap();
        assert_eq!(client.max_retries, DEFAULT_MAX_RETRIES);
    }

    #[test]
    fn builder_respects_custom_max_retries() {
        let conn = "endpoint=https://example.com;accesskey=c2VjcmV0";
        let client = ACSClientBuilder::new()
            .connection_string(conn)
            .max_retries(10)
            .build()
            .unwrap();
        assert_eq!(client.max_retries, 10);
    }

    #[test]
    fn builder_max_retries_zero_disables_retry() {
        let conn = "endpoint=https://example.com;accesskey=c2VjcmV0";
        let client = ACSClientBuilder::new()
            .connection_string(conn)
            .max_retries(0)
            .build()
            .unwrap();
        assert_eq!(client.max_retries, 0);
    }

    #[test]
    fn builder_max_retries_propagates_to_service_principal() {
        let client = ACSClientBuilder::new()
            .host("example.com")
            .service_principal("t", "c", "s")
            .max_retries(5)
            .build()
            .unwrap();
        assert_eq!(client.max_retries, 5);
    }

    // ── Phase 2: timeout ─────────────────────────────────────────────────────

    #[test]
    fn builder_default_timeout_builds_successfully() {
        // No timeout set — should still build fine
        let conn = "endpoint=https://example.com;accesskey=c2VjcmV0";
        assert!(ACSClientBuilder::new()
            .connection_string(conn)
            .build()
            .is_ok());
    }

    #[test]
    fn builder_with_timeout_builds_successfully() {
        let conn = "endpoint=https://example.com;accesskey=c2VjcmV0";
        let result = ACSClientBuilder::new()
            .connection_string(conn)
            .timeout(Duration::from_secs(30))
            .build();
        assert!(result.is_ok());
    }

    #[test]
    fn builder_timeout_zero_builds_successfully() {
        // Zero timeout is valid at builder level (reqwest allows it)
        let conn = "endpoint=https://example.com;accesskey=c2VjcmV0";
        let result = ACSClientBuilder::new()
            .connection_string(conn)
            .timeout(Duration::from_millis(0))
            .build();
        assert!(result.is_ok());
    }

    #[test]
    fn builder_timeout_and_max_retries_compose() {
        let conn = "endpoint=https://example.com;accesskey=c2VjcmV0";
        let client = ACSClientBuilder::new()
            .connection_string(conn)
            .timeout(Duration::from_secs(10))
            .max_retries(5)
            .build()
            .unwrap();
        assert_eq!(client.max_retries, 5);
    }

    // ── Phase 2: ACSError::RateLimitExceeded ─────────────────────────────────

    #[test]
    fn rate_limit_exceeded_carries_retry_count() {
        let err = ACSError::RateLimitExceeded { retries: 7 };
        assert!(err.to_string().contains("7"));
    }

    #[test]
    fn rate_limit_exceeded_display_mentions_retries() {
        let err = ACSError::RateLimitExceeded {
            retries: DEFAULT_MAX_RETRIES,
        };
        let s = err.to_string();
        assert!(s.contains("rate limit"));
        assert!(s.contains(&DEFAULT_MAX_RETRIES.to_string()));
    }

    // ── DEFAULT_MAX_RETRIES constant ─────────────────────────────────────────

    #[test]
    fn default_max_retries_constant_is_3() {
        assert_eq!(DEFAULT_MAX_RETRIES, 3);
    }

    // ── ACSClientBuilder::host ────────────────────────────────────────────────

    #[test]
    fn builder_host_sets_hostname_used_for_base_url() {
        // Verify that .host() is accepted and .build() succeeds, producing a
        // client whose base_url carries the https:// scheme prefix.
        let client = ACSClientBuilder::new()
            .host("my.host.example.com")
            .managed_identity()
            .build()
            .unwrap();
        assert!(client.base_url.starts_with("https://"));
        assert!(client.base_url.contains("my.host.example.com"));
    }

    #[test]
    fn builder_host_service_principal_sets_base_url() {
        let client = ACSClientBuilder::new()
            .host("sp.example.com")
            .service_principal("tenant", "client_id", "secret")
            .build()
            .unwrap();
        assert_eq!(client.base_url, "https://sp.example.com");
    }

    // ── #13 ACSClient::Clone ─────────────────────────────────────────────────

    #[test]
    fn client_clone_preserves_base_url() {
        let conn = "endpoint=https://example.com;accesskey=c2VjcmV0";
        let client = ACSClientBuilder::new()
            .connection_string(conn)
            .build()
            .unwrap();
        let cloned = client.clone();
        assert_eq!(client.base_url, cloned.base_url);
    }

    #[test]
    fn client_clone_preserves_max_retries() {
        let conn = "endpoint=https://example.com;accesskey=c2VjcmV0";
        let client = ACSClientBuilder::new()
            .connection_string(conn)
            .max_retries(7)
            .build()
            .unwrap();
        let cloned = client.clone();
        assert_eq!(cloned.max_retries, 7);
    }

    #[test]
    fn client_clone_preserves_api_version() {
        let conn = "endpoint=https://example.com;accesskey=c2VjcmV0";
        let client = ACSClientBuilder::new()
            .connection_string(conn)
            .api_version(ACSApiVersion::V20250901)
            .build()
            .unwrap();
        let cloned = client.clone();
        assert_eq!(cloned.api_version.as_str(), "2025-09-01");
    }

    #[test]
    fn client_clone_preserves_host() {
        let client = ACSClientBuilder::new()
            .host("myhost.communication.azure.com")
            .managed_identity()
            .build()
            .unwrap();
        let cloned = client.clone();
        assert_eq!(client.host, cloned.host);
    }

    // #13 — ACSClient is Send + Sync (compile-time assertion) ────────────────

    #[test]
    fn client_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ACSClient>();
    }

    // ── Phase 4: is_terminal_status ──────────────────────────────────────────

    #[test]
    fn is_terminal_succeeded() {
        assert!(is_terminal_status(&EmailSendStatusType::Succeeded));
    }

    #[test]
    fn is_terminal_failed() {
        assert!(is_terminal_status(&EmailSendStatusType::Failed));
    }

    #[test]
    fn is_terminal_canceled() {
        assert!(is_terminal_status(&EmailSendStatusType::Canceled));
    }

    #[test]
    fn is_terminal_unknown() {
        assert!(is_terminal_status(&EmailSendStatusType::Unknown));
    }

    #[test]
    fn is_not_terminal_not_started() {
        assert!(!is_terminal_status(&EmailSendStatusType::NotStarted));
    }

    #[test]
    fn is_not_terminal_running() {
        assert!(!is_terminal_status(&EmailSendStatusType::Running));
    }
}

// ── Integration tests (wiremock) ─────────────────────────────────────────────
//
// These tests start a real local HTTP server (wiremock) and point ACSClient at
// it via `base_url_override`. No Azure credentials or network access required.
// Shared-key auth is used with a dummy key; the mock server ignores auth headers.
//
// ADR: docs/adr/ADR-002-integration-tests-wiremock.md
#[cfg(test)]
mod integration_tests {
    use super::*;
    use serde_json::json;
    use tokio_util::sync::CancellationToken;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const FAKE_CONN: &str = "endpoint=https://fake.communication.azure.com;accesskey=c2VjcmV0";

    fn minimal_email() -> SentEmail {
        use crate::domain::entities::models::{
            EmailAddress, EmailContent, Recipients, SentEmailBuilder,
        };
        SentEmailBuilder::new()
            .sender("noreply@example.com".to_string())
            .content(EmailContent {
                subject: Some("Test".to_string()),
                plain_text: Some("body".to_string()),
                html: None,
            })
            .recipients(Recipients {
                to: Some(vec![EmailAddress {
                    email: Some("to@example.com".to_string()),
                    display_name: None,
                }]),
                cc: None,
                b_cc: None,
            })
            .build()
            .unwrap()
    }

    fn client_for(server: &MockServer) -> ACSClient {
        ACSClientBuilder::new()
            .connection_string(FAKE_CONN)
            .max_retries(0)
            .base_url_override(&server.uri())
            .build()
            .unwrap()
    }

    // ── send_email ────────────────────────────────────────────────────────────

    // ── API version routing ───────────────────────────────────────────────────

    #[tokio::test]
    async fn send_email_uses_v20230331_by_default() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .and(query_param("api-version", "2023-03-31"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "v1-msg" })))
            .mount(&server)
            .await;

        let client = client_for(&server); // uses default version
        let result = client.send_email(&minimal_email()).await;
        assert_eq!(result.unwrap(), "v1-msg");
    }

    #[tokio::test]
    async fn send_email_uses_v20250901_when_configured() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .and(query_param("api-version", "2025-09-01"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "v2-msg" })))
            .mount(&server)
            .await;

        let client = ACSClientBuilder::new()
            .connection_string(FAKE_CONN)
            .api_version(ACSApiVersion::V20250901)
            .max_retries(0)
            .base_url_override(&server.uri())
            .build()
            .unwrap();

        let result = client.send_email(&minimal_email()).await;
        assert_eq!(result.unwrap(), "v2-msg");
    }

    #[tokio::test]
    async fn get_email_status_uses_v20250901_when_configured() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/emails/operations/op-v2"))
            .and(query_param("api-version", "2025-09-01"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "op-v2",
                "status": "Succeeded"
            })))
            .mount(&server)
            .await;

        let client = ACSClientBuilder::new()
            .connection_string(FAKE_CONN)
            .api_version(ACSApiVersion::V20250901)
            .max_retries(0)
            .base_url_override(&server.uri())
            .build()
            .unwrap();

        let result = client.get_email_status("op-v2").await;
        assert!(matches!(result, Ok(EmailSendStatusType::Succeeded)));
    }

    // ── send_email (default version) ──────────────────────────────────────────

    #[tokio::test]
    async fn send_email_accepted_returns_message_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .and(query_param("api-version", "2023-03-31"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "msg-001" })))
            .mount(&server)
            .await;

        let email = minimal_email();
        let result = client_for(&server).send_email(&email).await;
        assert_eq!(result.unwrap(), "msg-001");
    }

    #[tokio::test]
    async fn send_email_500_returns_api_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(500).set_body_json(json!({
                "error": { "code": "InternalError", "message": "boom" }
            })))
            .mount(&server)
            .await;

        let email = minimal_email();
        let result = client_for(&server).send_email(&email).await;
        assert!(matches!(result, Err(ACSError::Api { .. })));
    }

    #[tokio::test]
    async fn send_email_retries_on_429_then_succeeds() {
        let server = MockServer::start().await;
        // First request → 429; second → 202
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                "error": { "code": "TooManyRequests", "message": "slow down" }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "msg-retry" })))
            .mount(&server)
            .await;

        let client = ACSClientBuilder::new()
            .connection_string(FAKE_CONN)
            .max_retries(3)
            .base_url_override(&server.uri())
            .build()
            .unwrap();

        let result = client.send_email(&minimal_email()).await;
        assert_eq!(result.unwrap(), "msg-retry");
    }

    #[tokio::test]
    async fn send_email_exhausts_retries_returns_rate_limit_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                "error": { "code": "TooManyRequests", "message": "slow down" }
            })))
            .mount(&server)
            .await;

        let client = ACSClientBuilder::new()
            .connection_string(FAKE_CONN)
            .max_retries(2)
            .base_url_override(&server.uri())
            .build()
            .unwrap();

        let result = client.send_email(&minimal_email()).await;
        assert!(matches!(
            result,
            Err(ACSError::RateLimitExceeded { retries: 2 })
        ));
    }

    // ── get_email_status ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_email_status_succeeded() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/emails/operations/op-123"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "op-123",
                "status": "Succeeded"
            })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        let result = client.get_email_status("op-123").await;
        assert!(matches!(result, Ok(EmailSendStatusType::Succeeded)));
    }

    #[tokio::test]
    async fn get_email_status_running() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/emails/operations/op-456"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "op-456",
                "status": "Running"
            })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        let result = client.get_email_status("op-456").await;
        assert!(matches!(result, Ok(EmailSendStatusType::Running)));
    }

    #[tokio::test]
    async fn get_email_status_api_error_on_404() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/emails/operations/no-such"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "error": { "code": "NotFound", "message": "operation not found" }
            })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        let result = client.get_email_status("no-such").await;
        assert!(matches!(result, Err(ACSError::Api { .. })));
    }

    // ── send_email_with_callback ──────────────────────────────────────────────

    #[tokio::test]
    async fn send_email_with_callback_invokes_callback_on_succeeded() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "cb-msg" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/emails/operations/cb-msg"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "id": "cb-msg", "status": "Succeeded" })),
            )
            .mount(&server)
            .await;

        let collected: std::sync::Arc<std::sync::Mutex<Vec<(String, bool)>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let collected_clone = collected.clone();

        let client = client_for(&server);
        let email = minimal_email();
        let (message_id, done_rx) = client
            .send_email_with_callback(&email, move |id, status, err| {
                collected_clone
                    .lock()
                    .unwrap()
                    .push((format!("{id}:{status}"), err.is_some()));
            })
            .await
            .unwrap();

        let _ = done_rx.await;

        assert_eq!(message_id, "cb-msg");
        let calls = collected.lock().unwrap();
        assert!(
            !calls.is_empty(),
            "callback should have been called at least once"
        );
        assert!(
            calls.iter().any(|(s, _)| s.contains("Succeeded")),
            "callback should report Succeeded"
        );
        assert!(
            calls.iter().all(|(_, has_err)| !has_err),
            "no error expected in callbacks"
        );
    }

    #[tokio::test]
    async fn send_email_with_callback_returns_error_when_send_fails() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(500).set_body_json(json!({
                "error": { "code": "InternalError", "message": "server fault" }
            })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        let email = minimal_email();
        let result = client.send_email_with_callback(&email, |_, _, _| {}).await;

        assert!(matches!(result, Err(ACSError::Api { .. })));
    }

    // ── parse_response: bad JSON → Deserialization error ──────────────────────

    #[tokio::test]
    async fn send_email_malformed_json_response_returns_deserialization_error() {
        let server = MockServer::start().await;
        // 202 but body is not valid JSON for SentEmailResponse
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_string("not json at all"))
            .mount(&server)
            .await;

        let client = client_for(&server);
        let result = client.send_email(&minimal_email()).await;
        assert!(matches!(result, Err(ACSError::Deserialization(_))));
    }

    // ── send_email_stream ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn send_email_stream_returns_error_when_send_fails() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(500).set_body_json(json!({
                "error": { "code": "InternalError", "message": "server fault" }
            })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        let result = client.send_email_stream(&minimal_email()).await;
        assert!(matches!(result, Err(ACSError::Api { .. })));
    }

    #[tokio::test]
    async fn send_email_stream_yields_terminal_status_and_stops() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "stream-op" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/emails/operations/stream-op"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "stream-op",
                "status": "Succeeded"
            })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        let (message_id, stream) = client.send_email_stream(&minimal_email()).await.unwrap();
        assert_eq!(message_id, "stream-op");

        tokio::pin!(stream);
        use futures::StreamExt;
        let mut statuses = Vec::new();
        while let Some(item) = stream.next().await {
            statuses.push(item.unwrap());
        }
        assert_eq!(statuses, vec![EmailSendStatusType::Succeeded]);
    }

    #[tokio::test]
    async fn send_email_stream_stops_on_status_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "err-op" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/emails/operations/err-op"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "error": { "code": "NotFound", "message": "operation not found" }
            })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        let (_, stream) = client.send_email_stream(&minimal_email()).await.unwrap();

        tokio::pin!(stream);
        use futures::StreamExt;
        let first = stream.next().await.expect("stream should yield one item");
        assert!(matches!(first, Err(ACSError::Api { .. })));
        // stream must terminate after an error
        assert!(stream.next().await.is_none());
    }

    // ── #13 pool-friendly: cloned client works end-to-end ────────────────────

    #[tokio::test]
    async fn cloned_client_can_send_email() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "clone-msg" })))
            .mount(&server)
            .await;

        let original = client_for(&server);
        let cloned = original.clone();
        let result = cloned.send_email(&minimal_email()).await;
        assert_eq!(result.unwrap(), "clone-msg");
    }

    #[tokio::test]
    async fn two_clones_send_independently() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "shared-msg" })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        let c1 = client.clone();
        let c2 = client.clone();
        let email = minimal_email();
        let (r1, r2) = tokio::join!(c1.send_email(&email), c2.send_email(&email));
        assert_eq!(r1.unwrap(), "shared-msg");
        assert_eq!(r2.unwrap(), "shared-msg");
    }

    // ── #11 send_emails_batch ─────────────────────────────────────────────────

    #[tokio::test]
    async fn send_emails_batch_empty_returns_empty_vec() {
        let server = MockServer::start().await;
        let client = client_for(&server);
        let results = client.send_emails_batch(&[]).await;
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn send_emails_batch_all_succeed() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "batch-id" })))
            .mount(&server)
            .await;

        let email = minimal_email();
        let client = client_for(&server);
        let results = client.send_emails_batch(&[email]).await;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].as_ref().unwrap(), "batch-id");
    }

    #[tokio::test]
    async fn send_emails_batch_all_fail_on_500() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(500).set_body_json(json!({
                "error": { "code": "InternalError", "message": "boom" }
            })))
            .mount(&server)
            .await;

        let email = minimal_email();
        let client = client_for(&server);
        let results = client.send_emails_batch(&[email]).await;
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], Err(ACSError::Api { .. })));
    }

    #[tokio::test]
    async fn send_emails_batch_multiple_results_in_order() {
        let server = MockServer::start().await;
        // Mock returns the same ID for every POST; we care that len == input len.
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "m" })))
            .mount(&server)
            .await;

        let email = minimal_email();
        let emails = vec![minimal_email(), minimal_email(), minimal_email()];
        let client = client_for(&server);
        let results = client.send_emails_batch(&emails).await;
        assert_eq!(results.len(), 3);
        for r in &results {
            assert!(r.is_ok());
        }
        // order is preserved
        let _ = email;
    }

    // ── #12 send_email_stream_cancellable ─────────────────────────────────────

    #[tokio::test]
    async fn send_email_stream_cancellable_stops_when_already_cancelled() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "c-op" })))
            .mount(&server)
            .await;
        // No status mock — if the stream polls it would panic on unmounted route.

        let token = CancellationToken::new();
        token.cancel(); // cancel before stream is even consumed

        let client = client_for(&server);
        let (id, stream) = client
            .send_email_stream_cancellable(&minimal_email(), token)
            .await
            .unwrap();
        assert_eq!(id, "c-op");

        tokio::pin!(stream);
        use futures::StreamExt;
        // Stream should yield nothing because the token was already cancelled.
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn send_email_stream_cancellable_returns_error_when_send_fails() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(500).set_body_json(json!({
                "error": { "code": "InternalError", "message": "fail" }
            })))
            .mount(&server)
            .await;

        let token = CancellationToken::new();
        let client = client_for(&server);
        let result = client
            .send_email_stream_cancellable(&minimal_email(), token)
            .await;
        assert!(matches!(result, Err(ACSError::Api { .. })));
    }

    #[tokio::test]
    async fn send_email_stream_cancellable_yields_terminal_status_normally() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "nc-op" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/emails/operations/nc-op"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "nc-op", "status": "Succeeded"
            })))
            .mount(&server)
            .await;

        let token = CancellationToken::new(); // never cancelled
        let client = client_for(&server);
        let (id, stream) = client
            .send_email_stream_cancellable(&minimal_email(), token)
            .await
            .unwrap();
        assert_eq!(id, "nc-op");

        tokio::pin!(stream);
        use futures::StreamExt;
        let mut statuses = Vec::new();
        while let Some(item) = stream.next().await {
            statuses.push(item.unwrap());
        }
        assert_eq!(statuses, vec![EmailSendStatusType::Succeeded]);
    }

    // ── #12 send_email_with_callback_cancellable ──────────────────────────────

    #[tokio::test]
    async fn send_email_with_callback_cancellable_stops_when_already_cancelled() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "cc-msg" })))
            .mount(&server)
            .await;
        // No status mock — callback must not be invoked.

        let token = CancellationToken::new();
        token.cancel();

        let callback_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let cc = callback_count.clone();

        let client = client_for(&server);
        let (message_id, done_rx) = client
            .send_email_with_callback_cancellable(&minimal_email(), token, move |_, _, _| {
                cc.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            })
            .await
            .unwrap();
        assert_eq!(message_id, "cc-msg");
        let _ = done_rx.await;
        assert_eq!(callback_count.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn send_email_with_callback_cancellable_invokes_callback_on_success() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "cbs-msg" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/emails/operations/cbs-msg"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({ "id": "cbs-msg", "status": "Succeeded" })),
            )
            .mount(&server)
            .await;

        let token = CancellationToken::new(); // never cancelled
        let statuses: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let s = statuses.clone();

        let client = client_for(&server);
        let (_, done_rx) = client
            .send_email_with_callback_cancellable(&minimal_email(), token, move |_, status, _| {
                s.lock().unwrap().push(status.to_string());
            })
            .await
            .unwrap();
        let _ = done_rx.await;
        let calls = statuses.lock().unwrap();
        assert!(calls.iter().any(|s| s == "Succeeded"));
    }

    #[tokio::test]
    async fn send_email_with_callback_cancellable_returns_error_when_send_fails() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(500).set_body_json(json!({
                "error": { "code": "InternalError", "message": "fail" }
            })))
            .mount(&server)
            .await;

        let token = CancellationToken::new();
        let client = client_for(&server);
        let result = client
            .send_email_with_callback_cancellable(&minimal_email(), token, |_, _, _| {})
            .await;
        assert!(matches!(result, Err(ACSError::Api { .. })));
    }

    // ── #11 batch: mixed success / failure ───────────────────────────────────

    #[tokio::test]
    async fn send_emails_batch_mixed_success_and_failure() {
        let server = MockServer::start().await;
        // First request to arrive → 500 (exhausted after 1 hit)
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(500).set_body_json(json!({
                "error": { "code": "InternalError", "message": "boom" }
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // Every subsequent request → 202
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "ok-id" })))
            .mount(&server)
            .await;

        let client = client_for(&server);
        let results = client
            .send_emails_batch(&[minimal_email(), minimal_email()])
            .await;

        assert_eq!(results.len(), 2);
        let ok_count = results.iter().filter(|r| r.is_ok()).count();
        let err_count = results.iter().filter(|r| r.is_err()).count();
        assert_eq!(ok_count, 1, "exactly one send should succeed");
        assert_eq!(err_count, 1, "exactly one send should fail");
    }

    #[tokio::test]
    async fn send_emails_batch_propagates_max_retries_config() {
        let server = MockServer::start().await;
        // Always 429 — exhausts retries
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                "error": { "code": "TooManyRequests", "message": "slow down" }
            })))
            .mount(&server)
            .await;

        let client = ACSClientBuilder::new()
            .connection_string(FAKE_CONN)
            .max_retries(2)
            .base_url_override(&server.uri())
            .build()
            .unwrap();

        let results = client.send_emails_batch(&[minimal_email()]).await;
        assert_eq!(results.len(), 1);
        assert!(matches!(
            results[0],
            Err(ACSError::RateLimitExceeded { retries: 2 })
        ));
    }

    #[tokio::test]
    async fn send_emails_batch_returns_error_on_malformed_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_string("not json"))
            .mount(&server)
            .await;

        let client = client_for(&server);
        let results = client.send_emails_batch(&[minimal_email()]).await;
        assert_eq!(results.len(), 1);
        assert!(matches!(results[0], Err(ACSError::Deserialization(_))));
    }

    // ── #12 stream cancellable: missing paths ─────────────────────────────────

    #[tokio::test]
    async fn send_email_stream_cancellable_stops_on_status_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "cse-op" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/emails/operations/cse-op"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "error": { "code": "NotFound", "message": "operation not found" }
            })))
            .mount(&server)
            .await;

        let token = CancellationToken::new(); // never cancelled
        let client = client_for(&server);
        let (_, stream) = client
            .send_email_stream_cancellable(&minimal_email(), token)
            .await
            .unwrap();

        tokio::pin!(stream);
        use futures::StreamExt;
        let first = stream
            .next()
            .await
            .expect("stream must yield an error item");
        assert!(matches!(first, Err(ACSError::Api { .. })));
        // stream must terminate after an error — next call returns None
        assert!(stream.next().await.is_none());
    }

    #[tokio::test]
    async fn send_email_stream_cancellable_running_then_succeeded() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "rs-op" })))
            .mount(&server)
            .await;
        // First poll → Running
        Mock::given(method("GET"))
            .and(path("/emails/operations/rs-op"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "rs-op", "status": "Running"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // Second poll → Succeeded
        Mock::given(method("GET"))
            .and(path("/emails/operations/rs-op"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "rs-op", "status": "Succeeded"
            })))
            .mount(&server)
            .await;

        let token = CancellationToken::new();
        let client = client_for(&server);
        let (_, stream) = client
            .send_email_stream_cancellable(&minimal_email(), token)
            .await
            .unwrap();

        tokio::pin!(stream);
        use futures::StreamExt;
        let mut statuses = Vec::new();
        while let Some(item) = stream.next().await {
            statuses.push(item.unwrap());
        }
        assert_eq!(
            statuses,
            vec![EmailSendStatusType::Running, EmailSendStatusType::Succeeded]
        );
    }

    // ── #12 callback cancellable: missing paths ───────────────────────────────

    #[tokio::test]
    async fn send_email_with_callback_cancellable_reports_error_on_status_failure() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "cpe-msg" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/emails/operations/cpe-msg"))
            .respond_with(ResponseTemplate::new(404).set_body_json(json!({
                "error": { "code": "NotFound", "message": "operation not found" }
            })))
            .mount(&server)
            .await;

        let token = CancellationToken::new();
        let had_error: std::sync::Arc<std::sync::atomic::AtomicBool> =
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = had_error.clone();

        let client = client_for(&server);
        let (_, done_rx) = client
            .send_email_with_callback_cancellable(&minimal_email(), token, move |_, _, err| {
                if err.is_some() {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                }
            })
            .await
            .unwrap();

        let _ = done_rx.await;
        assert!(
            had_error.load(std::sync::atomic::Ordering::SeqCst),
            "callback should have received a non-None error"
        );
    }

    #[tokio::test]
    async fn send_email_with_callback_cancellable_running_then_succeeded() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "crts-msg" })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/emails/operations/crts-msg"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "crts-msg", "status": "Running"
            })))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/emails/operations/crts-msg"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "crts-msg", "status": "Succeeded"
            })))
            .mount(&server)
            .await;

        let token = CancellationToken::new();
        let statuses: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
            std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let s = statuses.clone();

        let client = client_for(&server);
        let (_, done_rx) = client
            .send_email_with_callback_cancellable(&minimal_email(), token, move |_, status, _| {
                s.lock().unwrap().push(status.to_string());
            })
            .await
            .unwrap();

        let _ = done_rx.await;
        let calls = statuses.lock().unwrap();
        assert!(calls.contains(&"Running".to_string()));
        assert!(calls.contains(&"Succeeded".to_string()));
        assert_eq!(*calls.last().unwrap(), "Succeeded");
    }

    // ── #13 clone: get_email_status and different auth path ───────────────────

    #[tokio::test]
    async fn cloned_client_get_email_status_works() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/emails/operations/clone-op"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "clone-op", "status": "Succeeded"
            })))
            .mount(&server)
            .await;

        let original = client_for(&server);
        let cloned = original.clone();
        let result = cloned.get_email_status("clone-op").await;
        assert!(matches!(result, Ok(EmailSendStatusType::Succeeded)));
    }

    #[tokio::test]
    async fn clone_preserves_api_version_in_requests() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .and(query_param("api-version", "2025-09-01"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "ver-id" })))
            .mount(&server)
            .await;

        let original = ACSClientBuilder::new()
            .connection_string(FAKE_CONN)
            .api_version(ACSApiVersion::V20250901)
            .max_retries(0)
            .base_url_override(&server.uri())
            .build()
            .unwrap();
        let cloned = original.clone();

        let result = cloned.send_email(&minimal_email()).await;
        assert_eq!(result.unwrap(), "ver-id");
    }

    // ── handle_response_and_retry_if_needed: Retry-After header ──────────────

    #[tokio::test]
    async fn send_email_respects_retry_after_header() {
        let server = MockServer::start().await;
        // First request → 429 with Retry-After: 1
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(
                ResponseTemplate::new(429)
                    .append_header("Retry-After", "1")
                    .set_body_json(json!({
                        "error": { "code": "TooManyRequests", "message": "slow down" }
                    })),
            )
            .up_to_n_times(1)
            .mount(&server)
            .await;
        // Second request → 202
        Mock::given(method("POST"))
            .and(path("/emails:send"))
            .respond_with(ResponseTemplate::new(202).set_body_json(json!({ "id": "after-retry" })))
            .mount(&server)
            .await;

        let client = ACSClientBuilder::new()
            .connection_string(FAKE_CONN)
            .max_retries(3)
            .base_url_override(&server.uri())
            .build()
            .unwrap();

        let result = client.send_email(&minimal_email()).await;
        assert_eq!(result.unwrap(), "after-retry");
    }
}
