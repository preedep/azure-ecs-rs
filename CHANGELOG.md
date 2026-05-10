# Changelog

All notable changes are documented here.
Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

---

## [0.4.0] — unreleased

### Added

- **Configurable poll interval** — `.poll_interval(Duration)` on `ACSClientBuilder` controls the delay between status polls in `send_email_and_wait*`, `send_email_stream*`, and the callback variants. Default: 5 s (same as the previous hardcoded value — no behaviour change for existing callers).
- **`send_email_and_wait`** — `ACSClient::send_email_and_wait(email, timeout: Duration) -> Result<EmailSendStatusType, ACSError>` sends an email and blocks until a terminal status is observed or the deadline elapses. Returns `ACSError::Timeout` when the deadline expires; the email may still be delivered.
- **`send_email_and_wait_cancellable`** — identical to `send_email_and_wait` but accepts a `CancellationToken`; returns `ACSError::Canceled` when the token is cancelled before a terminal status is observed. Cancellation only stops the local wait — the email remains queued in ACS.
- **`send_email_idempotent`** — `ACSClient::send_email_idempotent(email, idempotency_key: &str)` sets `repeatability-request-id` and `repeatability-first-sent` headers on the initial request and all retries, making application-level retries safe against double-sends.
- **`ACSError::Timeout`** — returned by `send_email_and_wait` and `send_email_and_wait_cancellable` when the deadline elapses before a terminal status.
- **`ACSError::Canceled`** — returned by `send_email_and_wait_cancellable` when the `CancellationToken` is cancelled before a terminal status.
- **`examples/mail_wait.rs`** — demonstrates `send_email_and_wait` and `send_email_and_wait_cancellable` with a simulated shutdown abort.

### Security

- **Access key no longer logged** — `acs_shared_key::parse_endpoint` previously emitted the raw Shared Key (HMAC secret) at `debug!` level. The log line has been removed.
- **OAuth client secret no longer logged** — `examples/mail.rs` previously emitted the Service Principal `client_secret` at `debug!` level. The log line has been removed.

### Documentation

- **README** — new "Choosing a Send Method" section with a decision table and flowchart to help callers pick the right API; added code examples for `send_email_and_wait` and `send_email_and_wait_cancellable`; updated examples table.
- **Rust doc comments** — all public `ACSClientBuilder` methods (`new`, `api_version`, `host`, `connection_string`, `service_principal`, `managed_identity`, `build`) upgraded from `//` inline comments to `///` doc comments, now visible on docs.rs. Private helper functions simplified from verbose `# Arguments` / `# Returns` blocks to concise summaries.
- **CLAUDE.md** — full public API list, all error variants, complete integration test table, `poll_interval` builder option documented.

### Tests

136 → 157 (+21 unit + integration tests covering `poll_interval` builder propagation, clone preservation, all `send_email_and_wait*` and `send_email_idempotent` paths including timeout, cancellation, send failure, poll error, and idempotency header presence on retries).

---

## [0.3.0] — 2026-05-10

### Added

- **Batch send** — `ACSClient::send_emails_batch(emails: &[SentEmail]) -> Vec<Result<String, ACSError>>` dispatches all emails concurrently via `join_all` and returns results in input order. Errors are captured per-slot; a failed send does not abort sibling sends. All sends share the same `reqwest::Client` connection pool. ([ROADMAP Phase 7 #11])
- **Cancellation support** — two new methods alongside the existing ones (backward-compatible):
  - `send_email_stream_cancellable(email, token: CancellationToken)` — stream exits cleanly when the token is cancelled.
  - `send_email_with_callback_cancellable(email, token, callback)` — background polling task exits cleanly; `done_rx` resolves once the task stops.
  Both use `tokio::select!` to race the poll sleep against `token.cancelled()`, so a pre-cancelled token stops immediately without issuing any status requests. ([ROADMAP Phase 7 #12])
- **`tokio-util`** added as a dependency (`CancellationToken` lives in `tokio_util::sync`).

### Changed

- **Pool-friendly `ACSClient::Clone`** — expanded module-level and struct-level documentation with build-once / clone-into-tasks examples. `ACSClient: Send + Sync` is now verified by a compile-time assertion test. ([ROADMAP Phase 7 #13])

### Performance

- **`HeaderSet` serialiser** — eliminated the intermediate `BTreeMap` allocation that occurred on every email send. The custom `Serialize` impl now uses `serialize_map` with a two-pass iterator count; no intermediate collection is allocated.
- **`infer::get()`** — replaced `infer::Infer::new().get(buf)` with the `infer::get(buf)` free function in both `build()` and `build_async()`.
- **Filename conversion** — replaced `.to_string_lossy().to_string()` (two steps) with `.to_string_lossy().into_owned()` (one step) in both attachment builders.
- **Redundant clone removed** — `content_type.to_string()` in `build()` was called on a value already typed as `String`; replaced with a direct move.
- **`String::new()`** — replaced `"".to_string()` in the SharedKey auth fallback path with the zero-allocation `String::new()`.
- **Blocking I/O warning** — `EmailAttachmentBuilder::build()` doc now explicitly warns that it performs blocking `std::fs` I/O and must not be called from an async task; use `build_async()` instead.

### Documentation

- **CLAUDE.md** — added "Rust performance patterns" section covering allocation rules, async/blocking constraints, serialisation conventions, concurrency patterns, and a detailed zero-copy / memory layout guide with Bad/Good examples for each rule.

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
