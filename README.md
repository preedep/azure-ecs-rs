# Azure Email Communication Service for Rust (azure-ecs-rs)

[![Crates.io](https://img.shields.io/crates/v/azure-ecs-rs.svg)](https://crates.io/crates/azure-ecs-rs)
[![docs.rs](https://img.shields.io/docsrs/azure-ecs-rs)](https://docs.rs/azure-ecs-rs)
[![CI](https://github.com/preedep/azure-ecs-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/preedep/azure-ecs-rs/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Rust SDK for the [Azure Email Communication Service](https://learn.microsoft.com/en-us/azure/communication-services/) data-plane REST API.

## Features

- **Send email** — single send, batch concurrent send, callback-based polling, stream-based polling
- **Cancellation** — `CancellationToken` support on stream and callback variants
- **Three auth methods** — Shared Key, Service Principal, Managed Identity
- **Connection reuse** — `reqwest::Client` built once and shared across all requests and clones
- **Pool-friendly** — `ACSClient` is `Clone + Send + Sync`; share across tasks without locking
- **Typed errors** — `ACSError` enum with variants for network, auth, API, rate-limit, and more
- **Retry with backoff** — automatic retry on `429`/`503` with exponential backoff and `Retry-After` support
- **Configurable timeout** — per-request HTTP timeout via `.timeout(Duration)`
- **Attachment support** — sync (`build`) and async (`build_async`) paths; MIME type auto-detected
- **`tracing` integration** — structured spans on all public methods; bridges to `log`-based subscribers
- **Two API versions** — `2023-03-31` (default) and `2025-09-01` (opt-in)

## Authentication

| Method | Builder |
|---|---|
| Shared Key | `.connection_string("endpoint=https://…;accesskey=…")` |
| Service Principal | `.host("https://…").service_principal(&tenant, &client_id, &secret)` |
| Managed Identity | `.host("https://…").managed_identity()` |

## API Versions

| Constant | Version | Status |
|---|---|---|
| `ACSApiVersion::V20230331` | `2023-03-31` | Default — backward compatible |
| `ACSApiVersion::V20250901` | `2025-09-01` | Latest stable — opt-in |

## Choosing a Send Method

The SDK exposes several send methods. Use this table to pick the right one for your situation.

| Method | Use when |
|---|---|
| [`send_email`](#send-email) | You queue the email and track delivery yourself later (fire-and-queue). |
| [`send_email_idempotent`](#idempotent-send) | Same as above, but your code may retry on network failure — the key prevents double-sends. |
| [`send_emails_batch`](#batch-send) | You need to dispatch many emails concurrently in one call. |
| [`send_email_and_wait`](#wait-for-terminal-status) | You want a single `await` that returns the final delivery status. Simplest option when you can afford to block the task. |
| [`send_email_and_wait_cancellable`](#wait-with-cancellation) | Same as above, but inside a request handler or task that may be shut down early (e.g. axum, gRPC, CLI with Ctrl-C). |
| [`send_email_stream`](#stream-based-status-polling) | You need to react to *each* status transition — progress UI, per-step logging, custom retry logic. |
| [`send_email_stream_cancellable`](#stream-with-cancellation) | Same as above, but the consumer may abandon the stream before delivery completes. |
| [`send_email_with_callback`](#callback-original-api-backward-compatible) | You want a background task to fire a closure on each status update (metrics, audit log). |
| [`send_email_with_callback_cancellable`](#callback-with-cancellation) | Same as above, but the background task must stop cleanly on a shutdown signal. |

**Decision flowchart**

```
Do you need delivery confirmation?
├── No  → send_email (or send_email_idempotent if retries are needed)
│
└── Yes
    │
    ├── Many emails at once?
    │   └── Yes → send_emails_batch
    │
    ├── Need to react to each status step?
    │   ├── Yes → send_email_stream / send_email_stream_cancellable
    │   └── No
    │
    ├── Need a side-effect callback per update (e.g. metrics)?
    │   ├── Yes → send_email_with_callback / send_email_with_callback_cancellable
    │   └── No
    │
    └── Just want a single await for the final status?
        ├── No cancellation needed → send_email_and_wait
        └── Must be stoppable (request context / shutdown) → send_email_and_wait_cancellable
```

> **Cancellation note:** cancellation only stops the *local wait/poll loop* — the email remains queued in ACS and may still be delivered. `ACSError::Canceled` distinguishes early abort from `ACSError::Timeout` (deadline elapsed) and from a real API error.

## Quick Start

### Build the client

```rust
use std::time::Duration;
use azure_ecs_rs::adapters::gateways::acs_email::{ACSApiVersion, ACSClientBuilder};

// Shared Key
let client = ACSClientBuilder::new()
    .connection_string(&connection_str)
    .timeout(Duration::from_secs(30))
    .max_retries(3)
    .build()?;

// Service Principal
let client = ACSClientBuilder::new()
    .host(&asc_url)
    .service_principal(&tenant_id, &client_id, &client_secret)
    .build()?;

// Managed Identity
let client = ACSClientBuilder::new()
    .host(&asc_url)
    .managed_identity()
    .build()?;
```

### Build an email

```rust
use azure_ecs_rs::domain::entities::models::{
    EmailAddress, EmailContent, Recipients, SentEmailBuilder,
};

let email = SentEmailBuilder::new()
    .sender("noreply@yourdomain.azurecomm.net".to_string())
    .content(EmailContent {
        subject: Some("Hello".to_string()),
        plain_text: Some("Plain text body.".to_string()),
        html: Some("<p>HTML body.</p>".to_string()),
    })
    .recipients(Recipients {
        to: Some(vec![EmailAddress {
            email: Some("recipient@example.com".to_string()),
            display_name: Some("Recipient".to_string()),
        }]),
        cc: None,
        b_cc: None,
    })
    .build()?;
```

### Send email

```rust
let operation_id = client.send_email(&email).await?;
println!("queued: {operation_id}");
```

### Batch send

Send multiple emails concurrently. Results are returned in input order; a failed
send is captured as `Err` in its slot and does not abort the others.

```rust
let emails = vec![email1, email2, email3];
let results = client.send_emails_batch(&emails).await;

for (i, result) in results.iter().enumerate() {
    match result {
        Ok(id) => println!("[{i}] queued: {id}"),
        Err(e)  => eprintln!("[{i}] failed: {e}"),
    }
}
```

All sends share the same connection pool — no extra TLS handshakes beyond the first request.

### Stream-based status polling

```rust
use futures::StreamExt;
use azure_ecs_rs::domain::entities::models::EmailSendStatusType;

let (operation_id, stream) = client.send_email_stream(&email).await?;
tokio::pin!(stream);

while let Some(item) = stream.next().await {
    match item {
        Ok(EmailSendStatusType::Succeeded) => println!("delivered!"),
        Ok(EmailSendStatusType::Failed | EmailSendStatusType::Canceled) => {
            eprintln!("delivery failed");
            break;
        }
        Ok(status) => println!("status: {status}"),
        Err(e) => { eprintln!("poll error: {e}"); break; }
    }
}
```

### Stream with cancellation

```rust
use tokio_util::sync::CancellationToken;
use futures::StreamExt;

let token = CancellationToken::new();

let (operation_id, stream) = client
    .send_email_stream_cancellable(&email, token.clone())
    .await?;

// Cancel from another task at any time:
// token.cancel();

tokio::pin!(stream);
while let Some(item) = stream.next().await {
    println!("status: {:?}", item);
}
// Stream exits cleanly when the token is cancelled.
```

### Callback with cancellation

```rust
use tokio_util::sync::CancellationToken;

let token = CancellationToken::new();

let (operation_id, done_rx) = client
    .send_email_with_callback_cancellable(
        &email,
        token.clone(),
        |id, status, err| {
            if let Some(e) = err {
                eprintln!("id={id} error={e}");
            } else {
                println!("id={id} status={status}");
            }
        },
    )
    .await?;

// token.cancel() stops the background task early.
let _ = done_rx.await; // resolves on terminal status or cancellation
```

### Callback (original API, backward compatible)

```rust
let (operation_id, done_rx) = client
    .send_email_with_callback(&email, |id, status, err| {
        println!("id={id} status={status}");
    })
    .await?;

let _ = done_rx.await;
```

### Wait for terminal status

Send and block until `Succeeded`, `Failed`, `Canceled`, or `Unknown` — no stream
or callback needed. `ACSError::Timeout` is returned if no terminal state is
observed within the deadline; the email may still be in transit.

```rust
use azure_ecs_rs::domain::entities::models::{ACSError, EmailSendStatusType};
use std::time::Duration;

match client.send_email_and_wait(&email, Duration::from_secs(60)).await {
    Ok(EmailSendStatusType::Succeeded) => println!("delivered"),
    Ok(EmailSendStatusType::Failed)    => eprintln!("delivery failed"),
    Ok(status)                         => println!("terminal: {status}"),
    Err(ACSError::Timeout)             => eprintln!("timed out — email may still be in transit"),
    Err(e)                             => eprintln!("error: {e}"),
}
```

Use `.poll_interval(Duration)` on the builder to control how often status is
polled (default: 5 s).

### Wait with cancellation

`send_email_and_wait_cancellable` adds a [`CancellationToken`] so an external
signal — a shutdown handler, an HTTP request context, a test harness — can abort
the wait without leaking the poll loop. The email stays queued in ACS and may
still be delivered.

```rust
use tokio_util::sync::CancellationToken;
use azure_ecs_rs::domain::entities::models::{ACSError, EmailSendStatusType};
use std::time::Duration;

let token = CancellationToken::new();

// Cancel from another task at any time: token.cancel()

match client
    .send_email_and_wait_cancellable(&email, Duration::from_secs(60), token)
    .await
{
    Ok(EmailSendStatusType::Succeeded) => println!("delivered"),
    Err(ACSError::Canceled)            => eprintln!("wait cancelled — email may still deliver"),
    Err(ACSError::Timeout)             => eprintln!("timed out"),
    Err(e)                             => eprintln!("error: {e}"),
    Ok(status)                         => println!("terminal: {status}"),
}
```

[`CancellationToken`]: https://docs.rs/tokio-util/latest/tokio_util/sync/struct.CancellationToken.html

### Poll status manually

```rust
let status = client.get_email_status(&operation_id).await?;
println!("status: {status}");
```

### Typed error handling

```rust
use azure_ecs_rs::domain::entities::models::ACSError;

match client.send_email(&email).await {
    Ok(id) => println!("queued: {id}"),
    Err(ACSError::RateLimitExceeded { retries }) => eprintln!("rate limit after {retries} retries"),
    Err(ACSError::Auth(msg))                     => eprintln!("auth failed: {msg}"),
    Err(ACSError::Api { code, message })         => eprintln!("API error {code:?}: {message}"),
    Err(ACSError::Network(msg))                  => eprintln!("network: {msg}"),
    Err(e)                                       => eprintln!("other: {e}"),
}
```

### Pool-friendly usage

`ACSClient` is cheap to clone — all clones share the same `reqwest::Client`
connection pool. Build once and distribute across tasks:

```rust
// Build once at startup.
let client = ACSClientBuilder::new()
    .connection_string(&conn_str)
    .build()?;

// Clone into each task — no extra TLS handshake per clone.
let handles: Vec<_> = emails.iter().map(|email| {
    let c = client.clone();
    let e = email.clone();
    tokio::spawn(async move { c.send_email(&e).await })
}).collect();

for h in handles { let _ = h.await; }
```

Or use `send_emails_batch` to let the client manage concurrency for you.

### Attachments

```rust
use azure_ecs_rs::domain::entities::models::EmailAttachmentBuilder;

// Async (preferred in async contexts — non-blocking I/O)
let attachment = EmailAttachmentBuilder::new()
    .file_to_base64("report.pdf")
    .build_async()
    .await?;

// Sync (only use outside async runtimes — blocks the thread)
let attachment = EmailAttachmentBuilder::new()
    .file_to_base64("report.pdf")
    .build()?;
```

MIME type is detected automatically from the file contents and falls back to
`application/octet-stream` for unknown types.

### Observability

```rust
tracing_subscriber::fmt()
    .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
    .init();
```

Run with `RUST_LOG=azure_ecs_rs=debug` to see spans for every request,
retry, and status poll. The library also bridges to `log`-based subscribers
via `tracing-log`, so no changes are needed if you already use `pretty_env_logger`.

## Environment Variables

| Variable | Used by |
|---|---|
| `CONNECTION_STR` | `endpoint=https://…;accesskey=…` — SharedKey auth |
| `TENANT_ID`, `CLIENT_ID`, `CLIENT_SECRET` | ServicePrincipal auth |
| `ASC_URL` | ServicePrincipal / ManagedIdentity host |
| `SENDER` | From address (e.g. `DoNotReply@yourdomain.azurecomm.net`) |
| `REPLY_EMAIL`, `REPLY_EMAIL_DISPLAY` | Reply-to address used in examples |

## Examples

| Command | Description |
|---|---|
| `cargo run --example mail` | Basic send — Shared Key |
| `cargo run --example mail_async` | Callback-based polling |
| `cargo run --example mail_attach` | File attachment (sync build) |
| `cargo run --example mail_attach_async` | File attachment (async build) + tracing |
| `cargo run --example mail_error_handling` | Typed `ACSError` matching |
| `cargo run --example mail_retry_timeout` | Retry and timeout configuration |
| `cargo run --example mail_stream` | Stream-based status polling |
| `cargo run --example mail_wait` | `send_email_and_wait` + `send_email_and_wait_cancellable` |

Prefix any example with `RUST_LOG=debug` to enable tracing output.

## Testing

```bash
cargo test                          # 157 unit + wiremock tests — no credentials needed
cargo test --test azure_e2e -- --include-ignored   # real Azure E2E (credentials required)
```

The test suite has three tiers:

| Tier | Location | Credentials | What it covers |
|---|---|---|---|
| Unit | `src/**` `#[cfg(test)]` | None | Logic, builders, error variants, serialization |
| Wiremock integration | `mod integration_tests` in `acs_email.rs` | None | HTTP paths, retry, backoff, stream, batch, cancellation |
| Azure E2E | `tests/azure_e2e.rs` (`#[ignore]`) | Required | Auth validity, schema acceptance, real operation IDs |

See `docs/adr/ADR-002` and `docs/adr/ADR-003` for design rationale.

To run E2E tests locally:

```bash
CONNECTION_STR="endpoint=https://…;accesskey=…" \
SENDER="DoNotReply@yourdomain.azurecomm.net" \
TO_EMAIL="recipient@example.com" \
cargo test --test azure_e2e -- --include-ignored --nocapture
```

## Get credentials from Azure Portal

- **Connection String**
  ![Connection String](https://github.com/preedep/rust_azure_email_communication/blob/develop/images/image2.png)

- **Sender address**
  ![Sender](https://github.com/preedep/rust_azure_email_communication/blob/develop/images/image1.png)

## Changelog

See [CHANGELOG.md](CHANGELOG.md) for release history.
