# Changelog

All notable changes are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

---

## [0.2.0] — 2026-05-09

### Added
- **Typed errors** — `ACSError` enum (`Network`, `InvalidUrl`, `Serialization`, `Deserialization`, `Auth`, `Header`, `Api`, `MissingField`, `RateLimitExceeded`) via `thiserror`. Public API return type changes from `ErrorResponse` to `ACSError`. ([ROADMAP Phase 1])
- **`Default` impls** — `SentEmailBuilder`, `EmailAttachmentBuilder`, and `ACSClientBuilder` all implement `Default`. ([ROADMAP Phase 1])
- **Configurable timeout** — `.timeout(Duration)` on `ACSClientBuilder`; previously hardcoded to none. ([ROADMAP Phase 2])
- **Configurable retry** — `.max_retries(u32)` on `ACSClientBuilder` with exponential backoff and `Retry-After` header support; previously hardcoded to 3. ([ROADMAP Phase 2])
- **`tracing` integration** — structured spans on all public and key private methods via `#[instrument]`; `tracing-log` bridge included so `log`-based subscribers receive events automatically. ([ROADMAP Phase 3])
- **Async attachment I/O** — `EmailAttachmentBuilder::build_async()` uses `tokio::fs::read`; sync `build()` retained for backward compatibility. ([ROADMAP Phase 3])
- **Stream-based status polling** — `ACSClient::send_email_stream()` returns `impl Stream<Item = Result<EmailSendStatusType, ACSError>>`; terminates automatically on a terminal status or error. ([ROADMAP Phase 4])
- **`ACSApiVersion::V20250901`** — opt-in API version constant for explicit pinning.
- **Header public fields** — `Header.name` and `Header.value` are now `pub`, enabling callers to construct and inspect headers directly.
- **`HeaderSet` builder** — `HeaderSet(Vec<Header>)` newtype with custom serde: serializes as a flat `{"name": "value"}` map, skipping entries with `None` name or value.
- **CI/CD workflows** — GitHub Actions for CI (build, test, clippy, fmt) and release (tag-triggered publish to crates.io + GitHub Release).
- **Integration tests** — wiremock-based tests covering all HTTP paths; no Azure credentials required.
- **Pre-push hook** — `hooks/pre-push` runs fmt and clippy locally before every push.

### Breaking changes
- `EmailResult<T>` error type changed from `ErrorResponse` to `ACSError`. Match arms must be updated.

---

## [0.1.0] — initial release

- Basic `send_email` and `get_email_status` via ACS data-plane REST API.
- Shared Key, Service Principal, and Managed Identity authentication.
- Attachment support with base64 encoding and MIME detection.
- Callback-based status polling via `send_email_with_callback`.
