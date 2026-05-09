# ADR-003 — Azure End-to-End Tests

**Status:** Accepted  
**Date:** 2026-05-09

## Context

The wiremock-based integration tests (ADR-002) verify HTTP-layer behaviour — retry logic, error mapping, status polling — without Azure credentials. However, three concerns cannot be covered by a mock server:

1. **Auth credential validity** — the HMAC-SHA256 shared-key signature, ServicePrincipal OAuth2 token, and ManagedIdentity ambient credential must actually be accepted by Azure. Wiremock ignores auth headers entirely.
2. **JSON schema acceptance** — Azure rejects requests whose body does not match the ACS data-plane schema. Wiremock accepts any body.
3. **End-to-end delivery** — the only way to confirm an email is queued and a real operation ID is returned is to call the live API.

## Decision

Add `tests/azure_e2e.rs` containing `#[ignore]`-attributed `async` tests that call a real Azure ACS resource. The `#[ignore]` attribute means:

- `cargo test` (normal CI) skips them — no credentials required.
- `cargo test --test azure_e2e -- --include-ignored` runs them when credentials are available.

A separate GitHub Actions workflow (`.github/workflows/azure_e2e.yml`) runs on push to `develop` and `main`, supplies credentials via secrets, and executes the e2e suite.

### Test matrix

| Test | Auth | What it proves |
|---|---|---|
| `shared_key_send_email_accepted` | SharedKey | HMAC-SHA256 signature format accepted by Azure |
| `shared_key_get_email_status_after_send` | SharedKey | Operation ID is real and pollable |
| `shared_key_send_email_v20250901` | SharedKey | API version `2025-09-01` accepted |
| `shared_key_send_email_stream_yields_at_least_one_status` | SharedKey | Stream polling works against live server |
| `shared_key_send_email_stream_cancellable_stops_cleanly` | SharedKey | Cancelled stream does not hang against live server |
| `shared_key_send_emails_batch_all_accepted` | SharedKey | Concurrent batch dispatch accepted |
| `service_principal_send_email_accepted` | ServicePrincipal | OAuth2 token acquisition + bearer auth work |
| `service_principal_get_email_status_after_send` | ServicePrincipal | Status polling with bearer auth works |
| `managed_identity_send_email_accepted` | ManagedIdentity | Ambient credential works (cloud-only) |
| `cloned_client_sends_independently` | SharedKey | Both clones share pool and both succeed |

### Environment variables

| Variable | Required by |
|---|---|
| `CONNECTION_STR` | SharedKey tests |
| `SENDER` | All tests |
| `TO_EMAIL` | All tests |
| `TENANT_ID`, `CLIENT_ID`, `CLIENT_SECRET`, `ASC_URL` | ServicePrincipal tests |
| `ASC_URL` | ManagedIdentity tests |

### What is intentionally NOT tested here

- **Retry backoff timing** — covered by wiremock; real Azure does not reliably return `429` on demand.
- **Terminal delivery status (`Succeeded`)** — requires waiting up to minutes for SMTP delivery; not suitable for automated tests. Covered by wiremock's `send_email_stream_yields_terminal_status_and_stops`.
- **`send_email_with_callback`** — the 5-second poll interval makes a full terminal-status test too slow. Covered by wiremock; auth is the only gap, and `send_email` + `get_email_status` already cover auth for that code path.

## Consequences

- `tests/azure_e2e.rs` is compiled as part of the crate but tests are skipped by default — no build overhead in the normal flow.
- `dotenv` is already a dev-dependency; `load_dotenv()` helper in the test file allows loading `.env` for local runs.
- The e2e workflow requires `CONNECTION_STR`, `SENDER`, `TO_EMAIL` secrets in GitHub Actions. ManagedIdentity tests additionally require a cloud-hosted runner.
- Tests make real network calls and send real emails to `TO_EMAIL`. Use a dedicated test mailbox.
