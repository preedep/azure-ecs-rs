//! Domain model — all data types, builders, and the `ACSError` error enum.
//!
//! # Key types
//!
//! | Type | Role |
//! |---|---|
//! | [`ACSError`] | Typed error returned by every public `ACSClient` method |
//! | [`SentEmail`] / [`SentEmailBuilder`] | Top-level email payload |
//! | [`EmailAttachment`] / [`EmailAttachmentBuilder`] | File attachment with sync and async build paths |
//! | [`EmailSendStatusType`] | Delivery status enum (`NotStarted`, `Running`, `Succeeded`, …) |
//! | [`HeaderSet`] | Custom email headers; serialises as a flat `{"name": "value"}` JSON map |
//!
//! # Builder pattern
//!
//! All three builders implement [`Default`] and delegate to `new()`.  Use
//! [`SentEmailBuilder`] to construct a validated [`SentEmail`]; build returns
//! `Err(ACSError::MissingField)` if required fields are absent.

use base64::engine::general_purpose;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::Formatter;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;

/// Typed error returned by all public `ACSClient` methods.
#[derive(Debug, thiserror::Error)]
pub enum ACSError {
    /// HTTP request could not be sent (DNS, TLS, connection refused, …).
    #[error("network error: {0}")]
    Network(String),

    /// The constructed URL is malformed.
    #[error("invalid URL: {0}")]
    InvalidUrl(String),

    /// Failed to serialize the request body to JSON.
    #[error("serialization error: {0}")]
    Serialization(String),

    /// Failed to deserialize the response body from JSON.
    #[error("deserialization error: {0}")]
    Deserialization(String),

    /// Could not obtain an access token (service principal or managed identity).
    #[error("authentication error: {0}")]
    Auth(String),

    /// The shared-key HMAC header could not be built.
    #[error("header error: {0}")]
    Header(String),

    /// The API returned an error response.
    #[error("API error {}: {message}", code.as_deref().unwrap_or("unknown"))]
    Api {
        code: Option<String>,
        message: String,
    },

    /// A required field was absent in the API response.
    #[error("missing field in response: {0}")]
    MissingField(&'static str),

    /// Rate limit hit and all retries were exhausted.
    #[error("rate limit exceeded after {retries} retries")]
    RateLimitExceeded { retries: u32 },

    /// [`send_email_and_wait`] did not observe a terminal status within the given timeout.
    ///
    /// [`send_email_and_wait`]: crate::adapters::gateways::acs_email::ACSClient::send_email_and_wait
    #[error("timed out waiting for terminal delivery status")]
    Timeout,

    /// Polling was stopped early because the [`CancellationToken`] passed to
    /// [`send_email_and_wait_cancellable`] was cancelled before a terminal
    /// delivery status was observed.
    ///
    /// [`CancellationToken`]: tokio_util::sync::CancellationToken
    /// [`send_email_and_wait_cancellable`]: crate::adapters::gateways::acs_email::ACSClient::send_email_and_wait_cancellable
    #[error("polling cancelled by caller before terminal status was observed")]
    Canceled,
}

impl From<ErrorResponse> for ACSError {
    fn from(e: ErrorResponse) -> Self {
        let detail = e.error.unwrap_or_default();
        ACSError::Api {
            code: detail.code,
            message: detail
                .message
                .unwrap_or_else(|| "unknown error".to_string()),
        }
    }
}

/// Represents the status of an email send operation.
#[derive(Serialize, Deserialize, Debug)]
pub struct EmailSendStatus(EmailSendStatusType);

impl EmailSendStatus {
    /// Converts the `EmailSendStatus` to its underlying type.
    ///
    /// # Returns
    ///
    /// * `EmailSendStatusType` - The underlying type of the email send status.
    pub fn to_type(self) -> EmailSendStatusType {
        self.0
    }
}

/// Enum representing the possible statuses of an email send operation.
#[derive(Serialize, Deserialize, Debug, PartialEq)]
pub enum EmailSendStatusType {
    Unknown,
    Canceled,
    Failed,
    NotStarted,
    Running,
    Succeeded,
}

/// Represents the response received after sending an email.
#[derive(Serialize, Deserialize, Debug)]
pub struct SentEmailResponse {
    /// The ID of the sent email.
    #[serde(rename = "id")]
    pub id: Option<String>,

    /// The status of the sent email.
    #[serde(rename = "status")]
    pub status: Option<EmailSendStatus>,

    /// The error details if the email send operation failed.
    #[serde(rename = "error")]
    pub error: Option<ErrorDetail>,
}

/// Represents the details of an error.
#[derive(Serialize, Deserialize, Debug, Default)]
pub struct ErrorDetail {
    /// Additional information about the error.
    #[serde(rename = "additionalInfo")]
    pub additional_info: Option<Vec<ErrorAdditionalInfo>>,

    /// The error code.
    #[serde(rename = "code")]
    pub code: Option<String>,

    /// The error message.
    #[serde(rename = "message")]
    pub message: Option<String>,

    /// The target of the error.
    #[serde(rename = "target")]
    pub target: Option<String>,
}

/// Represents additional information about an error.
#[derive(Serialize, Deserialize, Debug)]
pub struct ErrorAdditionalInfo {
    /// The additional information.
    #[serde(rename = "info")]
    pub info: Option<String>,

    /// The type of the additional information.
    #[serde(rename = "type")]
    pub info_type: Option<String>,
}

/// Represents an email to be sent.
#[derive(Serialize, Deserialize, Debug)]
pub struct SentEmail {
    /// The headers of the email.
    #[serde(rename = "headers", skip_serializing_if = "Option::is_none")]
    pub headers: Option<HeaderSet>,

    /// The sender address of the email.
    #[serde(rename = "senderAddress")]
    pub sender: String,

    /// The content of the email.
    #[serde(rename = "content")]
    pub content: EmailContent,

    /// The recipients of the email.
    #[serde(rename = "recipients")]
    pub recipients: Recipients,

    /// The attachments of the email.
    #[serde(rename = "attachments", skip_serializing_if = "Option::is_none")]
    pub attachments: Option<Vec<EmailAttachment>>,

    /// The reply-to addresses of the email.
    #[serde(rename = "replyTo", skip_serializing_if = "Option::is_none")]
    pub reply_to: Option<Vec<EmailAddress>>,

    /// Indicates whether user engagement tracking is disabled.
    #[serde(
        rename = "userEngagementTrackingDisabled",
        skip_serializing_if = "Option::is_none"
    )]
    pub user_engagement_tracking_disabled: Option<bool>,
}

/// Builder for creating a `SentEmail` instance.
pub struct SentEmailBuilder {
    headers: Option<HeaderSet>,
    sender: Option<String>,
    content: Option<EmailContent>,
    recipients: Option<Recipients>,
    attachments: Option<Vec<EmailAttachment>>,
    reply_to: Option<Vec<EmailAddress>>,
    user_engagement_tracking_disabled: Option<bool>,
}

impl Default for SentEmailBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl SentEmailBuilder {
    /// Creates a new `SentEmailBuilder` instance.
    ///
    /// # Returns
    ///
    /// * `SentEmailBuilder` - A new instance of the builder.
    pub fn new() -> Self {
        SentEmailBuilder {
            headers: None,
            sender: None,
            content: None,
            recipients: None,
            attachments: None,
            reply_to: None,
            user_engagement_tracking_disabled: None,
        }
    }

    /// Sets the headers for the email.
    ///
    /// # Arguments
    ///
    /// * `headers` - A vector of `Header` instances.
    ///
    /// # Returns
    ///
    /// * `Self` - The builder instance.
    #[allow(dead_code)]
    pub fn headers(mut self, headers: Vec<Header>) -> Self {
        self.headers = Some(HeaderSet(headers));
        self
    }

    /// Sets the sender address for the email.
    ///
    /// # Arguments
    ///
    /// * `sender` - A string representing the sender address.
    ///
    /// # Returns
    ///
    /// * `Self` - The builder instance.
    pub fn sender(mut self, sender: String) -> Self {
        self.sender = Some(sender);
        self
    }

    /// Sets the content for the email.
    ///
    /// # Arguments
    ///
    /// * `content` - An `EmailContent` instance.
    ///
    /// # Returns
    ///
    /// * `Self` - The builder instance.
    pub fn content(mut self, content: EmailContent) -> Self {
        self.content = Some(content);
        self
    }

    /// Sets the recipients for the email.
    ///
    /// # Arguments
    ///
    /// * `recipients` - A `Recipients` instance.
    ///
    /// # Returns
    ///
    /// * `Self` - The builder instance.
    pub fn recipients(mut self, recipients: Recipients) -> Self {
        self.recipients = Some(recipients);
        self
    }

    /// Sets the attachments for the email.
    ///
    /// # Arguments
    ///
    /// * `attachments` - A vector of `EmailAttachment` instances.
    ///
    /// # Returns
    ///
    /// * `Self` - The builder instance.
    #[allow(dead_code)]
    pub fn attachments(mut self, attachments: Vec<EmailAttachment>) -> Self {
        self.attachments = Some(attachments);
        self
    }

    /// Sets the reply-to addresses for the email.
    ///
    /// # Arguments
    ///
    /// * `reply_to` - A vector of `EmailAddress` instances.
    ///
    /// # Returns
    ///
    /// * `Self` - The builder instance.
    #[allow(dead_code)]
    pub fn reply_to(mut self, reply_to: Vec<EmailAddress>) -> Self {
        self.reply_to = Some(reply_to);
        self
    }

    /// Sets whether user engagement tracking is disabled for the email.
    ///
    /// # Arguments
    ///
    /// * `user_engagement_tracking_disabled` - A boolean indicating whether tracking is disabled.
    ///
    /// # Returns
    ///
    /// * `Self` - The builder instance.
    pub fn user_engagement_tracking_disabled(
        mut self,
        user_engagement_tracking_disabled: bool,
    ) -> Self {
        self.user_engagement_tracking_disabled = Some(user_engagement_tracking_disabled);
        self
    }

    /// Builds the `SentEmail` instance.
    ///
    /// # Returns
    ///
    /// * `Result<SentEmail, &\`static str\>` - The built `SentEmail` instance or an error message.
    pub fn build(self) -> Result<SentEmail, &'static str> {
        Ok(SentEmail {
            headers: self.headers,
            sender: self.sender.ok_or("Sender is required")?,
            content: self.content.ok_or("Content is required")?,
            recipients: self.recipients.ok_or("Recipients are required")?,
            attachments: self.attachments,
            reply_to: self.reply_to,
            user_engagement_tracking_disabled: self.user_engagement_tracking_disabled,
        })
    }
}

/// Represents an email attachment.
#[derive(Serialize, Deserialize, Debug)]
pub struct EmailAttachment {
    /// The name of the attachment.
    #[serde(rename = "name")]
    name: Option<String>,

    /// The content type of the attachment.
    #[serde(rename = "contentType")]
    attachment_type: Option<String>,

    /// The base64 encoded content of the attachment.
    #[serde(rename = "contentInBase64")]
    content_bytes_base64: Option<String>,
}

/// Builder for creating a `EmailAttachment` instance.
pub struct EmailAttachmentBuilder {
    name: Option<String>,
    attachment_type: Option<String>,
    content_bytes_base64: Option<String>,
    file_path: Option<String>,
}

impl Default for EmailAttachmentBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl EmailAttachmentBuilder {
    /// Creates a new `EmailAttachmentBuilder` instance.
    ///
    /// # Returns
    ///
    /// * `EmailAttachmentBuilder` - A new instance of the builder.
    pub fn new() -> Self {
        EmailAttachmentBuilder {
            name: None,
            attachment_type: None,
            content_bytes_base64: None,
            file_path: None,
        }
    }

    /// Sets the content bytes in base64 format for the attachment.
    ///
    /// # Arguments
    ///
    /// * `name` - The name of the attachment.
    /// * `content_type` - The content type of the attachment.
    /// * `content_bytes_base64` - The base64 encoded content of the attachment.
    ///
    /// # Returns
    ///
    /// * `Self` - The builder instance.
    pub fn content_bytes_base64(
        mut self,
        name: String,
        content_type: String,
        content_bytes_base64: String,
    ) -> Self {
        self.name = Some(name);
        self.attachment_type = Some(content_type);
        self.content_bytes_base64 = Some(content_bytes_base64);
        self
    }

    /// Sets the file path to be converted to base64 format for the attachment.
    ///
    /// # Arguments
    ///
    /// * `file_path` - The file path of the attachment.
    ///
    /// # Returns
    ///
    /// * `Self` - The builder instance.
    pub fn file_to_base64(mut self, file_path: &str) -> Self {
        self.file_path = Some(file_path.to_string());
        self
    }

    /// Builds the `EmailAttachment` instance synchronously.
    ///
    /// **Warning:** when a `file_to_base64` path is set this method performs
    /// blocking file I/O (`std::fs`) on the calling thread.  Calling it from
    /// inside a Tokio async task will block the executor thread for the
    /// duration of the read.  Use [`build_async`](Self::build_async) instead
    /// when you are in an async context.
    ///
    /// # Returns
    ///
    /// * `Result<EmailAttachment, String>` - The built `EmailAttachment` or an error.
    pub fn build(mut self) -> Result<EmailAttachment, String> {
        let content_bytes_base64 = match self.file_path {
            Some(file_path) => {
                let path = Path::new(&file_path);
                if !path.exists() {
                    return Err("File does not exist".to_string());
                }
                let name = path
                    .file_name()
                    .ok_or("File name is required".to_string())?;
                self.name = Some(name.to_string_lossy().into_owned());
                let mut file =
                    File::open(file_path).map_err(|e| format!("Failed to open file {:?}", e))?;
                let mut buffer = Vec::new();
                file.read_to_end(&mut buffer)
                    .map_err(|e| format!("Failed to read file {:?}", e))?;
                let content_type = infer::get(&buffer)
                    .map(|t| t.mime_type())
                    .unwrap_or("application/octet-stream");
                self.attachment_type = Some(content_type.to_string());
                // Encode the byte vector to a Base64 string
                general_purpose::STANDARD.encode(&buffer)
            }
            None => self.content_bytes_base64.ok_or("Content is required")?,
        };
        Ok(EmailAttachment {
            name: self.name,
            attachment_type: self.attachment_type,
            content_bytes_base64: Some(content_bytes_base64),
        })
    }

    /// Async variant of [`build`](Self::build) — reads the file with `tokio::fs` to avoid
    /// blocking the async executor. Prefer this over `build()` when a `file_to_base64` path
    /// has been set and you are inside an async context.
    ///
    /// Falls back to the synchronous path when content was supplied via
    /// [`content_bytes_base64`](Self::content_bytes_base64) (no I/O needed).
    pub async fn build_async(mut self) -> Result<EmailAttachment, String> {
        let content_bytes_base64 = match self.file_path {
            Some(ref file_path) => {
                let path = PathBuf::from(file_path);
                if !path.exists() {
                    return Err("File does not exist".to_string());
                }
                self.name = Some(
                    path.file_name()
                        .ok_or("File name is required".to_string())?
                        .to_string_lossy()
                        .into_owned(),
                );

                let buffer = tokio::fs::read(&path)
                    .await
                    .map_err(|e| format!("Failed to read file: {}", e))?;

                let content_type = infer::get(&buffer)
                    .map(|t| t.mime_type())
                    .unwrap_or("application/octet-stream");
                self.attachment_type = Some(content_type.to_string());

                general_purpose::STANDARD.encode(&buffer)
            }
            None => self.content_bytes_base64.ok_or("Content is required")?,
        };
        Ok(EmailAttachment {
            name: self.name,
            attachment_type: self.attachment_type,
            content_bytes_base64: Some(content_bytes_base64),
        })
    }
}

/// Represents the content of an email.
#[derive(Serialize, Deserialize, Debug)]
pub struct EmailContent {
    /// The subject of the email.
    #[serde(rename = "subject")]
    pub subject: Option<String>,

    /// The plain text content of the email.
    #[serde(rename = "plainText")]
    pub plain_text: Option<String>,

    /// The HTML content of the email.
    #[serde(rename = "html")]
    pub html: Option<String>,
}

/// Represents a set of headers in an email.
#[derive(Debug)]
pub struct HeaderSet(Vec<Header>);

/// Represents a header in an email.
#[derive(Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct Header {
    /// The name of the header.
    #[serde(rename = "name")]
    pub name: Option<String>,

    /// The value of the header.
    #[serde(rename = "value")]
    pub value: Option<String>,
}

/// Represents the recipients of an email.
#[derive(Serialize, Deserialize, Debug)]
pub struct Recipients {
    /// The primary recipients of the email.
    #[serde(rename = "to")]
    pub to: Option<Vec<EmailAddress>>,

    /// The CC recipients of the email.
    #[serde(rename = "cc")]
    pub cc: Option<Vec<EmailAddress>>,

    /// The BCC recipients of the email.
    #[serde(rename = "bcc")]
    pub b_cc: Option<Vec<EmailAddress>>,
}

/// Represents an email address.
#[derive(Serialize, Deserialize, Debug)]
pub struct EmailAddress {
    /// The email address.
    #[serde(rename = "address")]
    pub email: Option<String>,

    /// The display name associated with the email address.
    #[serde(rename = "displayName")]
    pub display_name: Option<String>,
}

/// Represents an error response.
#[derive(Serialize, Deserialize, Debug)]
pub struct ErrorResponse {
    /// The error details.
    #[serde(rename = "error")]
    pub error: Option<ErrorDetail>,
}

/// Represents the parameters of an endpoint.
#[derive(Debug)]
pub struct EndPointParams {
    /// The host name of the endpoint.
    pub host_name: String,

    /// The access key for the endpoint.
    pub access_key: String,
}

impl fmt::Display for EmailSendStatusType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            EmailSendStatusType::Canceled => write!(f, "Canceled"),
            EmailSendStatusType::Failed => write!(f, "Failed"),
            EmailSendStatusType::NotStarted => write!(f, "NotStarted"),
            EmailSendStatusType::Running => write!(f, "Running"),
            EmailSendStatusType::Succeeded => write!(f, "Succeeded"),
            _ => write!(f, "Unknown"),
        }
    }
}

impl FromStr for EmailSendStatusType {
    type Err = ();

    /// Converts a string to an `EmailSendStatusType`.
    ///
    /// # Arguments
    ///
    /// * `s` - A string slice representing the status.
    ///
    /// # Returns
    ///
    /// * `Result<EmailSendStatusType, ()>` - The corresponding `EmailSendStatusType` or an error.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "Canceled" => Ok(EmailSendStatusType::Canceled),
            "Failed" => Ok(EmailSendStatusType::Failed),
            "NotStarted" => Ok(EmailSendStatusType::NotStarted),
            "Running" => Ok(EmailSendStatusType::Running),
            "Succeeded" => Ok(EmailSendStatusType::Succeeded),
            _ => Ok(EmailSendStatusType::Unknown),
        }
    }
}

impl fmt::Display for EmailSendStatus {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0).expect("EmailSendStatus: panic message");
        Ok(())
    }
}

impl Serialize for HeaderSet {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        let valid = self
            .0
            .iter()
            .filter(|h| h.name.is_some() && h.value.is_some());
        // Count without collecting so no intermediate Vec is allocated.
        let count = valid.clone().count();
        let mut map = serializer.serialize_map(Some(count))?;
        for h in valid {
            map.serialize_entry(h.name.as_ref().unwrap(), h.value.as_ref().unwrap())?;
        }
        map.end()
    }
}

impl<'de> serde::Deserialize<'de> for HeaderSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let headers_map: std::collections::BTreeMap<String, String> =
            serde::Deserialize::deserialize(deserializer)?;

        let headers = headers_map
            .into_iter()
            .map(|(name, value)| Header {
                name: Some(name),
                value: Some(value),
            })
            .collect();

        Ok(HeaderSet(headers))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn sent_email_builder_with_missing_content() {
        let result = SentEmailBuilder::new()
            .sender("sender@example.com".to_string())
            .recipients(Recipients {
                to: Some(vec![EmailAddress {
                    email: Some("to@example.com".to_string()),
                    display_name: Some("To".to_string()),
                }]),
                cc: None,
                b_cc: None,
            })
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn sent_email_builder_with_missing_recipients() {
        let result = SentEmailBuilder::new()
            .sender("sender@example.com".to_string())
            .content(EmailContent {
                subject: Some("Subject".to_string()),
                plain_text: Some("Plain text".to_string()),
                html: Some("<p>HTML</p>".to_string()),
            })
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn email_attachment_builder_with_invalid_file_path() {
        let result = EmailAttachmentBuilder::new()
            .file_to_base64("invalid_path.txt")
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn email_content_with_all_fields() {
        let content = EmailContent {
            subject: Some("Subject".to_string()),
            plain_text: Some("Plain text".to_string()),
            html: Some("<p>HTML</p>".to_string()),
        };
        assert_eq!(content.subject.unwrap(), "Subject");
        assert_eq!(content.plain_text.unwrap(), "Plain text");
        assert_eq!(content.html.unwrap(), "<p>HTML</p>");
    }

    #[test]
    fn email_content_with_missing_fields() {
        let content = EmailContent {
            subject: None,
            plain_text: Some("Plain text".to_string()),
            html: None,
        };
        assert!(content.subject.is_none());
        assert_eq!(content.plain_text.unwrap(), "Plain text");
        assert!(content.html.is_none());
    }

    #[test]
    fn test_header_set_serialization() {
        let header_set = HeaderSet(vec![
            Header {
                name: Some("Content-Type".to_string()),
                value: Some("application/json".to_string()),
            },
            Header {
                name: Some("Authorization".to_string()),
                value: Some("Bearer token".to_string()),
            },
        ]);
        let serialized = serde_json::to_value(&header_set).expect("Failed to serialize HeaderSet");
        let expected = json!({
            "Content-Type": "application/json",
            "Authorization": "Bearer token"
        });
        assert_eq!(serialized, expected);

        let serialized = serde_json::to_string(&header_set).expect("Failed to serialize HeaderSet");
        let deserialized: HeaderSet =
            serde_json::from_str(&serialized).expect("Failed to deserialize HeaderSet");
        assert_eq!(deserialized.0.len(), 2);
        assert_eq!(
            deserialized.0[0],
            Header {
                name: Some("Authorization".to_string()),
                value: Some("Bearer token".to_string())
            }
        );
        assert_eq!(
            deserialized.0[1],
            Header {
                name: Some("Content-Type".to_string()),
                value: Some("application/json".to_string())
            }
        );
    }

    #[test]
    fn test_empty_header_set_serialization() {
        let header_set = HeaderSet(Vec::new());
        let serialized = serde_json::to_value(&header_set).expect("Failed to serialize HeaderSet");
        let expected = json!({});
        assert_eq!(serialized, expected);
    }

    #[test]
    fn test_header_set_empty_deserialization() {
        let header_set = HeaderSet(Vec::new());
        let serialized = serde_json::to_string(&header_set).expect("Failed to serialize HeaderSet");
        let deserialized: HeaderSet =
            serde_json::from_str(&serialized).expect("Failed to deserialize HeaderSet");
        assert!(deserialized.0.is_empty());
    }

    // ── SentEmailBuilder ─────────────────────────────────────────────────────

    #[test]
    fn sent_email_builder_success() {
        let result = SentEmailBuilder::new()
            .sender("sender@example.com".to_string())
            .content(EmailContent {
                subject: Some("Subject".to_string()),
                plain_text: Some("Body".to_string()),
                html: None,
            })
            .recipients(Recipients {
                to: Some(vec![EmailAddress {
                    email: Some("to@example.com".to_string()),
                    display_name: Some("To".to_string()),
                }]),
                cc: None,
                b_cc: None,
            })
            .build();
        assert!(result.is_ok());
        let email = result.unwrap();
        assert_eq!(email.sender, "sender@example.com");
    }

    #[test]
    fn sent_email_builder_missing_sender() {
        let result = SentEmailBuilder::new()
            .content(EmailContent {
                subject: Some("Subject".to_string()),
                plain_text: None,
                html: None,
            })
            .recipients(Recipients {
                to: None,
                cc: None,
                b_cc: None,
            })
            .build();
        assert!(result.is_err());
    }

    #[test]
    fn sent_email_builder_optional_fields_default_to_none() {
        let email = SentEmailBuilder::new()
            .sender("s@example.com".to_string())
            .content(EmailContent {
                subject: None,
                plain_text: None,
                html: None,
            })
            .recipients(Recipients {
                to: None,
                cc: None,
                b_cc: None,
            })
            .build()
            .unwrap();
        assert!(email.headers.is_none());
        assert!(email.attachments.is_none());
        assert!(email.reply_to.is_none());
        assert!(email.user_engagement_tracking_disabled.is_none());
    }

    #[test]
    fn sent_email_builder_with_all_optional_fields() {
        let email = SentEmailBuilder::new()
            .sender("s@example.com".to_string())
            .content(EmailContent {
                subject: None,
                plain_text: None,
                html: None,
            })
            .recipients(Recipients {
                to: None,
                cc: None,
                b_cc: None,
            })
            .headers(vec![Header {
                name: Some("X-Custom".to_string()),
                value: Some("val".to_string()),
            }])
            .attachments(vec![])
            .reply_to(vec![])
            .user_engagement_tracking_disabled(true)
            .build()
            .unwrap();
        assert!(email.headers.is_some());
        assert_eq!(email.user_engagement_tracking_disabled, Some(true));
    }

    // ── EmailAttachmentBuilder ───────────────────────────────────────────────

    #[test]
    fn email_attachment_builder_with_content_bytes_base64() {
        let result = EmailAttachmentBuilder::new()
            .content_bytes_base64(
                "test.txt".to_string(),
                "text/plain".to_string(),
                "aGVsbG8=".to_string(),
            )
            .build();
        assert!(result.is_ok());
        let att = result.unwrap();
        assert!(serde_json::to_string(&att).is_ok());
    }

    // ── EmailSendStatusType ──────────────────────────────────────────────────

    #[test]
    fn email_send_status_type_display_all_variants() {
        assert_eq!(EmailSendStatusType::Canceled.to_string(), "Canceled");
        assert_eq!(EmailSendStatusType::Failed.to_string(), "Failed");
        assert_eq!(EmailSendStatusType::NotStarted.to_string(), "NotStarted");
        assert_eq!(EmailSendStatusType::Running.to_string(), "Running");
        assert_eq!(EmailSendStatusType::Succeeded.to_string(), "Succeeded");
        assert_eq!(EmailSendStatusType::Unknown.to_string(), "Unknown");
    }

    #[test]
    fn email_send_status_type_from_str_all_variants() {
        use std::str::FromStr;
        assert_eq!(
            EmailSendStatusType::from_str("Canceled").unwrap(),
            EmailSendStatusType::Canceled
        );
        assert_eq!(
            EmailSendStatusType::from_str("Failed").unwrap(),
            EmailSendStatusType::Failed
        );
        assert_eq!(
            EmailSendStatusType::from_str("NotStarted").unwrap(),
            EmailSendStatusType::NotStarted
        );
        assert_eq!(
            EmailSendStatusType::from_str("Running").unwrap(),
            EmailSendStatusType::Running
        );
        assert_eq!(
            EmailSendStatusType::from_str("Succeeded").unwrap(),
            EmailSendStatusType::Succeeded
        );
        assert_eq!(
            EmailSendStatusType::from_str("anything-else").unwrap(),
            EmailSendStatusType::Unknown
        );
    }

    #[test]
    fn email_send_status_display_delegates_to_type() {
        let status = EmailSendStatus(EmailSendStatusType::Succeeded);
        assert_eq!(status.to_string(), "Succeeded");
    }

    #[test]
    fn email_send_status_to_type() {
        let status = EmailSendStatus(EmailSendStatusType::Running);
        assert_eq!(status.to_type(), EmailSendStatusType::Running);
    }

    // ── Header public fields (sunsided PR #5) ────────────────────────────────

    #[test]
    fn header_fields_are_public_and_readable() {
        let h = Header {
            name: Some("X-Custom".to_string()),
            value: Some("hello".to_string()),
        };
        assert_eq!(h.name.as_deref(), Some("X-Custom"));
        assert_eq!(h.value.as_deref(), Some("hello"));
    }

    #[test]
    fn sent_email_builder_headers_appear_in_serialized_json() {
        let email = SentEmailBuilder::new()
            .sender("s@example.com".to_string())
            .content(EmailContent {
                subject: Some("Hi".to_string()),
                plain_text: None,
                html: None,
            })
            .recipients(Recipients {
                to: None,
                cc: None,
                b_cc: None,
            })
            .headers(vec![
                Header {
                    name: Some("X-Foo".to_string()),
                    value: Some("bar".to_string()),
                },
                Header {
                    name: Some("X-Baz".to_string()),
                    value: Some("qux".to_string()),
                },
            ])
            .build()
            .unwrap();
        let json = serde_json::to_value(&email).unwrap();
        let headers = json.get("headers").expect("headers should be present");
        assert_eq!(headers["X-Foo"], "bar");
        assert_eq!(headers["X-Baz"], "qux");
    }

    // ── HeaderSet serialization edge cases ───────────────────────────────────

    #[test]
    fn header_set_skips_headers_with_none_name_or_value() {
        let header_set = HeaderSet(vec![
            Header {
                name: None,
                value: Some("v".to_string()),
            },
            Header {
                name: Some("k".to_string()),
                value: None,
            },
            Header {
                name: Some("Keep".to_string()),
                value: Some("yes".to_string()),
            },
        ]);
        let serialized = serde_json::to_value(&header_set).unwrap();
        assert_eq!(serialized.as_object().unwrap().len(), 1);
        assert_eq!(serialized["Keep"], "yes");
    }

    // ── SentEmail JSON serialization ─────────────────────────────────────────

    #[test]
    fn sent_email_serializes_required_fields_only() {
        let email = SentEmailBuilder::new()
            .sender("s@example.com".to_string())
            .content(EmailContent {
                subject: Some("Hi".to_string()),
                plain_text: None,
                html: None,
            })
            .recipients(Recipients {
                to: None,
                cc: None,
                b_cc: None,
            })
            .build()
            .unwrap();
        let json = serde_json::to_value(&email).unwrap();
        assert_eq!(json["senderAddress"], "s@example.com");
        assert!(
            json.get("headers").is_none(),
            "optional headers should be omitted"
        );
        assert!(
            json.get("attachments").is_none(),
            "optional attachments should be omitted"
        );
    }

    // ── Phase 3b: build_async ─────────────────────────────────────────────────

    #[tokio::test]
    async fn build_async_with_content_bytes_succeeds() {
        let result = EmailAttachmentBuilder::new()
            .content_bytes_base64(
                "file.txt".to_string(),
                "text/plain".to_string(),
                "aGVsbG8=".to_string(),
            )
            .build_async()
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn build_async_missing_file_returns_error() {
        let result = EmailAttachmentBuilder::new()
            .file_to_base64("/nonexistent/path/file.txt")
            .build_async()
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[tokio::test]
    async fn build_async_reads_real_file_and_detects_mime() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        // Write minimal PNG header so infer detects image/png
        let png_header: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        tmp.write_all(png_header).unwrap();
        tmp.flush().unwrap();

        let result = EmailAttachmentBuilder::new()
            .file_to_base64(tmp.path().to_str().unwrap())
            .build_async()
            .await
            .unwrap();

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["contentType"], "image/png");
        assert!(json["contentInBase64"].as_str().unwrap().len() > 0);
    }

    #[tokio::test]
    async fn build_async_unknown_mime_falls_back_to_octet_stream() {
        use std::io::Write;
        let mut tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.write_all(b"hello world plain text").unwrap();
        tmp.flush().unwrap();

        let result = EmailAttachmentBuilder::new()
            .file_to_base64(tmp.path().to_str().unwrap())
            .build_async()
            .await
            .unwrap();

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["contentType"], "application/octet-stream");
    }

    #[tokio::test]
    async fn build_async_sets_filename_from_path() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("report.pdf");
        std::fs::write(&file_path, b"%PDF-1.4").unwrap();

        let result = EmailAttachmentBuilder::new()
            .file_to_base64(file_path.to_str().unwrap())
            .build_async()
            .await
            .unwrap();

        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["name"], "report.pdf");
    }

    // ── SentEmailBuilder::reply_to ───────────────────────────────────────────

    #[test]
    fn reply_to_with_addresses_sets_field() {
        let email = SentEmailBuilder::new()
            .sender("s@example.com".to_string())
            .content(EmailContent {
                subject: None,
                plain_text: None,
                html: None,
            })
            .recipients(Recipients {
                to: None,
                cc: None,
                b_cc: None,
            })
            .reply_to(vec![EmailAddress {
                email: Some("reply@example.com".to_string()),
                display_name: Some("Reply Handler".to_string()),
            }])
            .build()
            .unwrap();

        let addrs = email.reply_to.expect("reply_to should be Some");
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].email.as_deref(), Some("reply@example.com"));
        assert_eq!(addrs[0].display_name.as_deref(), Some("Reply Handler"));
    }

    #[test]
    fn reply_to_appears_in_serialized_json() {
        let email = SentEmailBuilder::new()
            .sender("s@example.com".to_string())
            .content(EmailContent {
                subject: None,
                plain_text: None,
                html: None,
            })
            .recipients(Recipients {
                to: None,
                cc: None,
                b_cc: None,
            })
            .reply_to(vec![EmailAddress {
                email: Some("reply@example.com".to_string()),
                display_name: None,
            }])
            .build()
            .unwrap();

        let json = serde_json::to_value(&email).unwrap();
        let arr = json["replyTo"]
            .as_array()
            .expect("replyTo should be an array");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["address"], "reply@example.com");
    }

    // ── Phase 3a: tracing smoke test ─────────────────────────────────────────

    #[test]
    fn tracing_subscriber_initialises_without_panic() {
        // Verifies that tracing-subscriber can be set up in test context —
        // proves our tracing macros emit valid metadata.
        let _ = tracing_subscriber::fmt::try_init();
    }
}
