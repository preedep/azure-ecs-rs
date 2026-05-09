# Azure Email Communication Service for Rust (azure-ecs-rs)

Azure Email Communication Service is part of the Azure Communication Services. It provides a REST API to send emails.
For more information, see the [Azure Communication Services documentation](https://learn.microsoft.com/en-us/azure/communication-services/).

[Azure Communication Service - Email - Rest API](https://learn.microsoft.com/en-us/rest/api/communication/email/send?tabs=HTTP)

## Features

- Send email (simple and with callback)
- Stream-based status polling (`send_email_stream`)
- Get email status
- Attachment support with auto MIME type detection (sync and async I/O)
- Connection reuse (`reqwest::Client` shared across all requests)
- Typed error handling (`ACSError` enum)
- Configurable retry with exponential backoff
- Configurable per-request timeout
- `tracing` integration — structured spans on all public methods

## Authentication

| Method | Builder call |
|---|---|
| Shared Key | `.connection_string(&str)` |
| Service Principal | `.host(&str).service_principal(&tenant, &client_id, &secret)` |
| Managed Identity | `.managed_identity().host(&str)` |

## API Versions

| Version | Constant | Status |
|---|---|---|
| `2023-03-31` | `ACSApiVersion::V20230331` | Default (backward compatible) |
| `2025-09-01` | `ACSApiVersion::V20250901` | Latest stable — opt-in |

## Environment Variables

```sh
# Common
SENDER="noreply@yourdomain.azurecomm.net"
REPLY_EMAIL="recipient@example.com"
REPLY_EMAIL_DISPLAY="Recipient Name"

# Shared Key
CONNECTION_STR="endpoint=https://...;accesskey=..."

# Service Principal
CLIENT_ID="..."
CLIENT_SECRET="..."
TENANT_ID="..."
ASC_URL="https://yourresource.asiapacific.communication.azure.com"
```

## Quick Start

### Build the client

```rust
use std::time::Duration;
use azure_ecs_rs::adapters::gateways::acs_email::{ACSApiVersion, ACSClientBuilder};

// Shared Key
let client = ACSClientBuilder::new()
    .connection_string(&connection_str)
    .api_version(ACSApiVersion::V20250901) // optional; default is 2023-03-31
    .timeout(Duration::from_secs(30))
    .max_retries(3)
    .build()
    .expect("Failed to build ACSClient");

// Service Principal
let client = ACSClientBuilder::new()
    .host(&host_name)
    .service_principal(&tenant_id, &client_id, &client_secret)
    .timeout(Duration::from_secs(30))
    .build()
    .expect("Failed to build ACSClient");

// Managed Identity
let client = ACSClientBuilder::new()
    .managed_identity()
    .host(&host_name)
    .build()
    .expect("Failed to build ACSClient");
```

### Send email

```rust
use azure_ecs_rs::domain::entities::models::{
    EmailAddress, EmailContent, Recipients, SentEmailBuilder,
};

let email = SentEmailBuilder::new()
    .sender(sender)
    .content(EmailContent {
        subject: Some("Hello".to_string()),
        plain_text: Some("Plain text body.".to_string()),
        html: Some("<p>HTML body.</p>".to_string()),
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

let message_id = client.send_email(&email).await?;
```

### Typed error handling

```rust
use azure_ecs_rs::domain::entities::models::ACSError;

match client.send_email(&email).await {
    Ok(id) => println!("Accepted: {id}"),
    Err(ACSError::RateLimitExceeded { retries }) => {
        eprintln!("Rate limit after {retries} retries");
    }
    Err(ACSError::Auth(msg)) => {
        eprintln!("Auth failed: {msg}");
    }
    Err(ACSError::Api { code, message }) => {
        eprintln!("API error {:?}: {message}", code);
    }
    Err(ACSError::Network(msg)) if msg.contains("timed out") => {
        eprintln!("Timed out — increase .timeout() or check endpoint");
    }
    Err(e) => eprintln!("Other error: {e}"),
}
```

### Stream-based status polling

`send_email_stream` returns an `impl Stream<Item = Result<EmailSendStatusType, ACSError>>`.
Each item is a polled status; the stream ends when a terminal state is reached
(`Succeeded`, `Failed`, `Canceled`, `Unknown`) or an error occurs.

```rust
use futures::StreamExt;
use azure_ecs_rs::domain::entities::models::EmailSendStatusType;

let (message_id, stream) = client.send_email_stream(&email).await?;
tokio::pin!(stream);

while let Some(item) = stream.next().await {
    match item {
        Ok(EmailSendStatusType::Succeeded) => {
            println!("Delivered!");
            break;
        }
        Ok(EmailSendStatusType::Failed | EmailSendStatusType::Canceled) => {
            eprintln!("Delivery failed");
            break;
        }
        Ok(status) => println!("Status: {status}"),
        Err(e) => {
            eprintln!("Poll error: {e}");
            break;
        }
    }
}
```

### Send with callback

```rust
let (message_id, done_rx) = client
    .send_email_with_callback(&email, |msg_id, status, err| {
        if let Some(e) = err {
            eprintln!("id={msg_id} error={e}");
        } else {
            println!("id={msg_id} status={status}");
        }
    })
    .await?;

let _ = done_rx.await; // wait for terminal state
```

### Attachments

```rust
use azure_ecs_rs::domain::entities::models::EmailAttachmentBuilder;

// Sync (blocks current thread)
let attachment = EmailAttachmentBuilder::new()
    .file_to_base64("report.pdf")
    .build()
    .expect("Failed to load attachment");

// Async (non-blocking, preferred in async contexts)
let attachment = EmailAttachmentBuilder::new()
    .file_to_base64("report.pdf")
    .build_async()
    .await
    .expect("Failed to load attachment");
```

MIME type is detected automatically from the file contents; falls back to `application/octet-stream`.

### Observability

The library emits `tracing` spans and events. Wire up any `tracing` subscriber in your application:

```rust
tracing_subscriber::fmt()
    .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
    .init();
```

Then run with `RUST_LOG=debug` to see library spans alongside application logs.

## Examples

| Example | Description |
|---|---|
| `cargo run --example mail` | Basic send with Shared Key |
| `cargo run --example mail_async` | Send with callback |
| `cargo run --example mail_attach` | Send with file attachment (sync) |
| `cargo run --example mail_error_handling` | Typed `ACSError` matching + status poll loop |
| `cargo run --example mail_retry_timeout` | Retry/timeout configuration |
| `cargo run --example mail_attach_async` | Non-blocking attachment + full tracing setup |
| `cargo run --example mail_stream` | Stream-based status polling with `send_email_stream` |

Prefix any example with `RUST_LOG=debug` to enable tracing output.

## Get email credentials from Azure Portal

- **Connection String**
  ![Connection String](https://github.com/preedep/rust_azure_email_communication/blob/develop/images/image2.png)

- **Sender address**
  ![Sender](https://github.com/preedep/rust_azure_email_communication/blob/develop/images/image1.png)
