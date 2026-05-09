# ADR-002 — Integration Testing with Wiremock

**Status:** Accepted  
**Date:** 2026-05-09

## Context

The HTTP-layer behaviour of `ACSClient` — retry logic, exponential backoff, error variant mapping, status polling — cannot be exercised by pure unit tests because it depends on live HTTP round-trips. Previously, this behaviour was only testable against a real Azure ACS endpoint, requiring valid credentials and network access.

A mechanism was needed to run these tests in CI and local development without Azure credentials.

## Decision

Use the [`wiremock`](https://docs.rs/wiremock) crate (`v0.6`) to run a real local HTTP server in test processes.

Key design choices:

**`base_url_override` on `ACSClientBuilder`** — a `#[cfg(test)] pub(crate)` builder method that replaces the computed `https://<host>` base URL with an arbitrary URL (e.g. `http://127.0.0.1:<port>`). This allows tests to point `ACSClient` at a wiremock server without any public API surface changes.

**Shared-key auth with a dummy key** — integration tests build the client with `connection_string("endpoint=https://fake.communication.azure.com;accesskey=c2VjcmV0")`. The HMAC-SHA256 auth header is computed and sent, but the wiremock server accepts any request without validating it. This avoids adding a `no_auth` bypass or making auth optional.

**Tests live in `src/adapters/gateways/acs_email.rs`** under `mod integration_tests` — keeping them in the same crate gives access to the `pub(crate)` builder method. Scenarios covered:

| Test | What it proves |
|---|---|
| `send_email_accepted_returns_message_id` | Happy path: 202 → `Ok(message_id)` |
| `send_email_500_returns_api_error` | Server error → `ACSError::Api` |
| `send_email_retries_on_429_then_succeeds` | 429 once → retry → 202 succeeds |
| `send_email_exhausts_retries_returns_rate_limit_error` | Persistent 429 → `ACSError::RateLimitExceeded { retries }` |
| `get_email_status_succeeded` | Status polling 200 `"Succeeded"` → `EmailSendStatusType::Succeeded` |
| `get_email_status_running` | Status polling 200 `"Running"` → `EmailSendStatusType::Running` |
| `get_email_status_api_error_on_404` | 404 → `ACSError::Api` |

## Consequences

- `ACSClient` stores `base_url: String` (computed from host at build time) instead of using `host` directly in URL construction. This is a purely internal refactor with no public API change.
- `ACSClientBuilder` has a `#[cfg(test)] pub(crate)` field `base_url_override` and method. It is invisible outside test compilation and does not appear in published docs.
- `wiremock = "0.6"` added to `[dev-dependencies]`. It is not a runtime dependency.
- Integration tests run as part of `cargo test` with no additional setup or environment variables.
- Retry backoff sleeps are real (`tokio::time::sleep`) but `max_retries` is kept low (2–3) in tests so wall-clock time is bounded.
