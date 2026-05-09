# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Overview

`azure-ecs-rs` is a Rust SDK for Azure Email Communication Service (ACS). It provides sync and async clients for sending emails and polling send status via the ACS REST API.

> **API versions:** `2023-03-31` (default, backward-compatible) and `2025-09-01` (opt-in via `ACSApiVersion::V20250901`). The data-plane spec is identical between the two versions; `V20250901` is available for explicit pinning only. See ADR-001 for why opt-out management is not in this SDK.

## Commands

```bash
cargo build                                          # build
cargo test                                           # run all 110 tests (unit + integration)
cargo clippy -- -D warnings                          # lint (must pass clean for CI)
cargo fmt --check                                    # format check (must pass clean for CI)
cargo test <name>                                    # run a single test by name substring
cargo clippy                                         # lint
cargo run --example mail                             # sync example (shared key auth)
cargo run --example mail_async                       # async example (service principal auth)
cargo run --example mail_attach                      # async example with attachments
RUST_LOG=debug cargo run --example mail              # with debug logging
```

## Git hooks

A pre-push hook lives in `hooks/pre-push`. It runs `cargo fmt --check` and `cargo clippy -- -D warnings` before every push, catching CI failures locally. Install it once per clone:

```bash
cp hooks/pre-push .git/hooks/pre-push
chmod +x .git/hooks/pre-push
```

## Testing

All tests run with `cargo test` — no Azure credentials or network access required.

### Unit tests

Inline in each module under `#[cfg(test)]`:

| Module | What is tested |
|---|---|
| `acs_email.rs` | `ACSApiVersion` variants/default, `ACSClientBuilder` all construction paths + `host()` base-URL formation + error messages + `max_retries` + `timeout`, `parse_url`, `serialize_body`, all error helpers, `ACSError` Display/From/RateLimitExceeded, `wrap_http_client`, `is_terminal_status` all variants |
| `acs_shared_key.rs` | `compute_content_sha256` known SHA-256 vectors, `compute_signature` (valid, invalid, determinism, divergence), `parse_endpoint` all error cases, `get_request_header` all required headers + HMAC-SHA256 Authorization scheme |
| `models.rs` | `SentEmailBuilder` (success, all missing-field errors, optional fields, `reply_to` with real addresses + JSON serialization), `EmailAttachmentBuilder` sync + async (`build_async`: valid file, missing file, MIME detection for PNG/octet-stream, filename extraction), `EmailSendStatusType` Display + `FromStr` all variants, `EmailSendStatus` Display + `to_type`, `HeaderSet` serde round-trip, `SentEmail` JSON serialization, tracing subscriber smoke test |

### Integration tests (wiremock)

Live in `mod integration_tests` inside `acs_email.rs`. Use `wiremock` to start a real local HTTP server per test — no Azure credentials needed. See **ADR-002** for design rationale.

| Test | What it covers |
|---|---|
| `send_email_accepted_returns_message_id` | 202 response → `Ok(message_id)` |
| `send_email_500_returns_api_error` | 500 response → `ACSError::Api` |
| `send_email_retries_on_429_then_succeeds` | First request 429, second 202 → retry works |
| `send_email_exhausts_retries_returns_rate_limit_error` | Persistent 429 → `ACSError::RateLimitExceeded { retries }` |
| `get_email_status_succeeded` | 200 `"Succeeded"` → `EmailSendStatusType::Succeeded` |
| `get_email_status_running` | 200 `"Running"` → `EmailSendStatusType::Running` |
| `get_email_status_api_error_on_404` | 404 → `ACSError::Api` |
| `send_email_stream_returns_error_when_send_fails` | Send 500 → stream itself returns `Err` before yielding |
| `send_email_stream_yields_terminal_status_and_stops` | Send 202, status Succeeded → stream yields one item then terminates |
| `send_email_stream_stops_on_status_error` | Send 202, status 404 → stream yields `Err` then terminates |

**How it works:** `ACSClientBuilder` has a `#[cfg(test)] pub(crate) fn base_url_override(url: &str)` method that replaces the computed `https://<host>` with the wiremock server URI. Auth headers are computed with a dummy shared key; the mock server ignores them.

## Architecture

The codebase follows a hexagonal (ports-and-adapters) layout:

```
src/
  domain/entities/models.rs   — all data types, builders, and ACSError
  adapters/gateways/
    acs_email.rs              — ACSClient: main HTTP client, all public methods
    acs_shared_key.rs         — shared-key HMAC-SHA256 signing logic
```

### Key types

- **`ACSClient`** (`acs_email.rs`) — built via `ACSClientBuilder`. Holds auth method, base URL, shared `reqwest::Client`, and API version. Public API:
  - `send_email(email: &SentEmail) -> Result<String, ACSError>`
  - `send_email_with_callback(email, cb) -> Result<(String, Receiver<()>), ACSError>` — polls via tokio task, fires callback on each status update
  - `get_email_status(operation_id: &str) -> Result<EmailSendStatusType, ACSError>`

- **`ACSError`** (`models.rs`) — typed error enum via `thiserror`. Variants: `Network`, `InvalidUrl`, `Serialization`, `Deserialization`, `Auth`, `Header`, `Api { code, message }`, `MissingField`, `RateLimitExceeded { retries }`.

- **`ACSApiVersion`** — `V20230331` (default) / `V20250901`. Set via `.api_version()` on the builder.

- **`ACSClientBuilder`** — fluent builder implementing `Default`. Key options:
  - `.timeout(Duration)` — per-request HTTP timeout (default: none)
  - `.max_retries(u32)` — retries on 429/503 with exponential backoff (default: 3)
  - `.api_version(ACSApiVersion)` — API version (default: `V20230331`)

- **`SentEmail`** / **`SentEmailBuilder`** — top-level email object. Builder implements `Default`. Composes `EmailContent`, `Recipients`, optional `Vec<EmailAttachment>`.

- **`HeaderSet`** — `Vec<Header>` newtype with custom serde: serializes as `{"name": "value", …}` map, skipping entries with `None` name or value.

### Auth flow

| Method | How `Authorization` header is formed |
|---|---|
| SharedKey | HMAC-SHA256 over request line + headers, via `acs_shared_key.rs` |
| ServicePrincipal | OAuth2 client-credentials token fetched via `azure_identity` |
| ManagedIdentity | Token from ambient managed identity via `azure_identity` |

### Connection reuse

`reqwest::Client` is created once in `ACSClientBuilder::build()` and stored on `ACSClient`. All requests (`send_email`, `get_email_status`, retries, auth token fetches) share this client, eliminating per-request TLS handshakes.

### Attachment handling

`EmailAttachmentBuilder` offers two build paths:

| Method | I/O | Use when |
|---|---|---|
| `build()` | Synchronous (`std::fs`) | Content already in memory via `content_bytes_base64()` |
| `build_async().await` | Async (`tokio::fs::read`) | Loading from a file path inside an async context |

Both paths use the `infer` crate for MIME type detection and fall back to `application/octet-stream` for unknown types.

### Observability

The library emits `tracing` spans and events. All public methods (`send_email`, `get_email_status`, `send_email_with_callback`) and key private functions (`send_request`, `acs_send_email`, `acs_get_email_status`) are instrumented with `#[instrument]`. Fields recorded: `host`, `api_version`, `max_retries`, `url`, `method`.

Callers using a `log`-based setup (e.g. `pretty_env_logger`) get library events automatically via the `tracing-log` bridge included as a dependency. Callers using `tracing-subscriber` get full structured spans.

## Error handling

All public methods return `Result<T, ACSError>`. Match on variants for typed handling:

```rust
match client.send_email(&email).await {
    Ok(id) => { /* … */ }
    Err(ACSError::RateLimitExceeded { retries }) => { /* back off */ }
    Err(ACSError::Auth(msg)) => { /* credential issue */ }
    Err(ACSError::Api { code, message }) => { /* API-level error */ }
    Err(e) => { /* network, serialization, etc. */ }
}
```

## Performance notes

- **No per-request allocations in the hot path** — `reqwest::Client` is shared (see Connection reuse above).
- **Retry with exponential backoff** — `429 Too Many Requests` and `503 Service Unavailable` trigger automatic retries (configurable via `.max_retries(u32)`, default: 3). Respects `Retry-After` header when present; falls back to exponential backoff (`2^n` seconds). Exhausting retries yields `ACSError::RateLimitExceeded { retries }`. Set `.max_retries(0)` to disable retry entirely.
- **Attachment base64 encoding** — `Vec<u8>` → base64 is done in memory. For large files this allocates the full file buffer plus the base64 string. Use `build_async()` to avoid blocking the executor on file I/O; the memory allocation pattern is the same either way.
- **Connection string parsing** — `parse_endpoint` allocates on every `build()` call. Avoid calling `build()` inside hot loops; build once and clone `ACSClient` (it's `Clone`).

## Environment variables

Copy `.env.example` (or create `.env`) with values matching your Azure resource:

| Variable | Used by |
|---|---|
| `CONNECTION_STR` | SharedKey auth (`endpoint=https://…;accesskey=…`) |
| `TENANT_ID`, `CLIENT_ID`, `CLIENT_SECRET`, `ASC_URL` | ServicePrincipal auth |
| `ASC_URL` | ManagedIdentity auth |
| `SENDER` | From address (e.g. `DoNotReply@yourdomain.com`) |
| `REPLY_EMAIL`, `REPLY_EMAIL_DISPLAY` | Reply-to address used in examples |

## Rust reference docs

Curated references relevant to this codebase:

### Language & idioms
- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/) — builder pattern, `Default`, naming conventions, `From`/`Into`
- [The Rust Book](https://doc.rust-lang.org/book/) — ownership, traits, error handling
- [Rust by Example](https://doc.rust-lang.org/rust-by-example/) — concise idiom reference

### Async
- [Tokio docs](https://docs.rs/tokio) — runtime, `spawn`, `sleep`, `oneshot`, `select!`
- [Async Rust Book](https://rust-lang.github.io/async-book/) — `Future`, `Pin`, cancellation semantics

### Error handling
- [thiserror docs](https://docs.rs/thiserror) — `#[derive(Error)]`, `#[error("…")]`, `#[source]`, `#[from]`
- [Error handling in Rust (blog)](https://nick.groenen.me/posts/rust-error-handling/) — library vs application error strategies

### HTTP & serialization
- [reqwest docs](https://docs.rs/reqwest) — `Client`, `RequestBuilder`, connection pooling, TLS
- [serde docs](https://serde.rs/) — `Serialize`/`Deserialize`, custom implementations, `#[serde(rename)]`
- [serde_json docs](https://docs.rs/serde_json) — `Value`, `json!` macro, streaming

### Azure SDK
- [azure_core docs](https://docs.rs/azure_core) — `HttpClient`, `TokenCredential`
- [azure_identity docs](https://docs.rs/azure_identity) — `ClientSecretCredential`, `create_credential`
- [ACS Email REST API spec](https://learn.microsoft.com/en-us/rest/api/communication/email/email/send?view=rest-communication-email-2025-09-01) — official API reference
- [ACS REST spec on GitHub](https://github.com/Azure/azure-rest-api-specs/tree/main/specification/communication/data-plane/Email) — request/response schemas

## Architecture Decision Records

Significant design decisions are captured in `docs/adr/`:

| ADR | Decision |
|---|---|
| [ADR-001](docs/adr/ADR-001-phase5-opt-out-arm-only.md) | Opt-out / suppression list management is ARM management-plane only — not implemented in this data-plane SDK |
| [ADR-002](docs/adr/ADR-002-integration-tests-wiremock.md) | Integration tests use `wiremock` with a `#[cfg(test)]` `base_url_override` builder method; no Azure credentials required |

## GitHub Actions

| Workflow | File | Trigger |
|---|---|---|
| CI | `.github/workflows/ci.yml` | Push / PR to `main` or `develop` |
| Release | `.github/workflows/release.yml` | Push of tag matching `v*.*.*` |

**CI** runs two jobs in parallel: `test` (`cargo build` + `cargo test`) and `lint` (`cargo clippy -- -D warnings` + `cargo fmt --check`).

**Release** requires the git tag to match the version in `Cargo.toml`, then runs full CI, publishes to crates.io (`CARGO_REGISTRY_TOKEN` secret required), and creates a GitHub Release with auto-generated notes.
