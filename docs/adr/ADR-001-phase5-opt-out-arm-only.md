# ADR-001 — Phase 5 Opt-Out / Unsubscribe Management

**Status:** Accepted  
**Date:** 2026-05-09

## Context

The roadmap listed Phase 5 as adding `add_unsubscribe`, `remove_unsubscribe`, and `check_unsubscribe` operations to `ACSClient`, citing the 2025-09-01 API version as the enabler.

Investigation of the ACS Email REST API specifications revealed:

- The **data-plane API** (`https://<resource>.communication.azure.com`) at version `2025-09-01` exposes exactly **two endpoints**, unchanged from `2023-03-31`:
  - `POST /emails:send`
  - `GET /emails/operations/{operationId}`
- Suppression list / unsubscribe management is available exclusively on the **Azure Resource Manager management plane** (`https://management.azure.com`), under paths such as:
  - `PUT /subscriptions/{sub}/resourceGroups/{rg}/providers/Microsoft.Communication/emailServices/{svc}/domains/{domain}/suppressionLists/{list}/suppressionListAddresses/{id}`
  - `GET` and `DELETE` variants of the same path

The management plane requires:
- Different base URL and completely different URL path structure
- Azure AD authentication only (no shared-key support)
- Additional required parameters: subscription ID, resource group, email service name, domain name, suppression list name

This is a fundamentally different API tier from the data-plane client this SDK implements.

## Decision

**Revise Phase 5 scope.** We will not implement opt-out management in `ACSClient`.

Reasons:
1. Adding ARM management-plane operations would require a second client type (`ACSManagementClient`) with a materially different construction surface (subscription ID, resource group, etc.), blurring the purpose of the library.
2. The current `ACSClient` is a focused data-plane client. Mixing planes would violate the single-responsibility principle and confuse callers about which operations require which credentials.
3. Callers who need suppression list management can use the official Azure SDK for Rust (`azure_mgmt_communication`) or the Azure Portal / CLI.

The roadmap Phase 5 entry will be updated to document this constraint rather than the original feature plan.

## Consequences

- No new public API is added for suppression list management.
- `ACSApiVersion::V20250901` remains available for callers who want explicit version pinning or to benefit from future data-plane additions in that version.
- If Azure adds opt-out operations to the data-plane API in a future spec revision, Phase 5 can be revisited without breaking changes.
- The ROADMAP.md Phase 5 entry is updated to reflect this decision.
