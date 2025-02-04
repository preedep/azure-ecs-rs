use base64::engine::general_purpose;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::fmt::Formatter;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::str::FromStr;

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

    /// Builds the `EmailAttachment` instance.
    ///
    /// # Returns
    ///
    /// * `Result<EmailAttachment, &\`static str\>` - The built `EmailAttachment` instance or an error message.
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
                self.name = Some(name.to_string_lossy().to_string());
                // Open the file
                let mut file =
                    File::open(file_path).map_err(|e| format!("Failed to open file {:?}", e))?;
                // Read the file contents into a byte vector
                let mut buffer = Vec::new();
                file.read_to_end(&mut buffer)
                    .map_err(|e| format!("Failed to read file {:?}", e))?;
                // Infer the content type of the file
                let content_type = infer::Infer::new()
                    .get(&buffer)
                    .map(|info| info.mime_type().to_string())
                    .unwrap_or_else(|| "application/octet-stream".to_string());

                self.attachment_type = Some(content_type.to_string());
                // Encode the byte vector to a Base64 string
                let encoded = general_purpose::STANDARD.encode(&buffer);
                encoded
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
        let mut headers_map = std::collections::BTreeMap::new();
        for header in self.0.iter().filter(|header| header.name.is_some() && header.value.is_some()) {
            headers_map.insert(header.name.as_ref().unwrap(), header.value.as_ref().unwrap());
        }
        headers_map.serialize(serializer)
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
    use serde_json::json;
    use super::*;

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
}
