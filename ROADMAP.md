# Roadmap

## Phase 1 — Foundation (breaking → v0.2.0) ✅ complete

| # | Feature | Status |
|---|---|---|
| 1 | Structured errors with `thiserror` — typed `ACSError` enum (`Network`, `InvalidUrl`, `Serialization`, `Deserialization`, `Auth`, `Header`, `Api`, `MissingField`, `RateLimitExceeded`) | ✅ |
| 2 | `Default` impls for all builders — `SentEmailBuilder`, `EmailAttachmentBuilder`, `ACSClientBuilder` | ✅ |

**Why breaking:** `EmailResult<T>` error type changes from `ErrorResponse` to `ACSError`. Callers matching on the error must update.

---

## Phase 2 — Client configuration (v0.2.x) ✅ complete

| # | Feature | Status |
|---|---|---|
| 3 | Configurable timeout — `.timeout(Duration)` on `ACSClientBuilder` | ✅ |
| 4 | Configurable retry policy — `.max_retries(u32)` on `ACSClientBuilder` | ✅ |

**Why:** Both are previously hardcoded (`no timeout`, `max_retries = 3`). Exposing them lets callers tune for their SLA.

---

## Phase 3 — Observability & async hygiene (v0.2.0) ✅ complete

| # | Feature | Status |
|---|---|---|
| 5 | `tracing` integration — library uses `tracing` macros with `#[instrument]` spans on all public and key private methods; `tracing-log` bridges to existing `log` subscribers | ✅ |
| 6 | Async file I/O for attachments — `build_async()` added to `EmailAttachmentBuilder` using `tokio::fs::read`; sync `build()` kept for backward compat | ✅ |

**Why:** `tracing` gives structured, correlated telemetry with zero setup for callers. Blocking file I/O on an async executor can starve the thread pool on large attachments.

---

## Phase 4 — Ergonomics (v0.3.0) ✅ complete

| # | Feature | Status |
|---|---|---|
| 7 | `Stream`-based status polling — `send_email_stream() -> impl Stream<Item = Result<EmailSendStatusType, ACSError>>` | ✅ |

**Why:** More composable and cancellable than the current callback API. The callback form stays for backward compat.

---

## Phase 5 — New API surface (v0.3.x, requires `ACSApiVersion::V20250901`) — revised

| # | Feature | Status |
|---|---|---|
| 8 | Opt-out / Unsubscribe management — `add_unsubscribe`, `remove_unsubscribe`, `check_unsubscribe` | ❌ Not applicable |

**Decision (ADR-001):** Suppression list management is available only on the **Azure Resource Manager management plane** (`management.azure.com`), not the data-plane API this SDK targets. The `2025-09-01` data-plane spec adds no new endpoints. Implementing this feature would require a separate `ACSManagementClient` with a different credential model (Azure AD only, no shared key) and different path parameters (subscription ID, resource group, etc.), which is out of scope for a data-plane SDK. Callers who need suppression list management should use the Azure Portal, CLI, or `azure_mgmt_communication` SDK.

`ACSApiVersion::V20250901` remains available for explicit version pinning.

---

## Phase 6 — CI/CD (runs in parallel with other phases) ✅ complete

| # | Feature | Status |
|---|---|---|
| 9 | CI workflow — `cargo build`, `cargo test`, `cargo clippy -- -D warnings`, `cargo fmt --check` on push/PR to `main` and `develop` | ✅ |
| 10 | Release workflow — triggered on `v*.*.*` tags: verifies tag matches `Cargo.toml` version, runs full CI, publishes to crates.io, creates GitHub Release with auto-generated notes | ✅ |

---

## Status legend

| Symbol | Meaning |
|---|---|
| ✅ | Completed |
| 🔄 | In progress |
| ⬜ | Pending |
