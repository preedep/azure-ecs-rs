//! Shared-key (HMAC-SHA256) request signing for the ACS data-plane API.
//!
//! The ACS shared-key authentication scheme works as follows:
//!
//! 1. Hash the request body with SHA-256 and base64-encode the digest
//!    (`x-ms-content-sha256` header).
//! 2. Build a canonical signing string from the HTTP method, path+query,
//!    date, host, and content hash.
//! 3. Sign the string with HMAC-SHA256 using the base64-decoded access key.
//! 4. Attach the signature in an `Authorization: HMAC-SHA256 …` header.
//!
//! All signing happens synchronously in the calling thread; no I/O is performed.

use crate::domain::entities::models::EndPointParams;
use base64::{engine::general_purpose, Engine as _};
use hmac::{Hmac, Mac};
use httpdate::fmt_http_date;
use reqwest::header::HeaderMap;
use sha2::{Digest, Sha256};
use std::time::SystemTime;
use tracing::debug;
use url::Url;

type HmacSha256 = Hmac<Sha256>;

/// Computes the SHA-256 hash of the given content and encodes it in base64.
///
/// # Arguments
///
/// * `content` - A string slice that holds the content to be hashed.
///
/// # Returns
///
/// * `String` - The base64 encoded SHA-256 hash of the content.
pub fn compute_content_sha256(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let result = hasher.finalize();
    general_purpose::STANDARD.encode(result)
}

/// Computes the HMAC-SHA256 signature for the given string using the provided secret.
///
/// # Arguments
///
/// * `string_to_sign` - A string slice that holds the string to be signed.
/// * `secret` - A string slice that holds the secret key.
///
/// # Returns
///
/// * `Result<String, String>` - The base64 encoded HMAC-SHA256 signature or an error message.
pub fn compute_signature(string_to_sign: &str, secret: &str) -> Result<String, String> {
    let decoded_secret = general_purpose::STANDARD
        .decode(secret)
        .map_err(|e| format!("Failed to decode secret: {}", e))?;
    let mut mac = HmacSha256::new_from_slice(&decoded_secret)
        .map_err(|e| format!("Failed to create HMAC instance: {}", e))?;
    mac.update(string_to_sign.as_bytes());
    let result = mac.finalize();
    let code_bytes = result.into_bytes();
    Ok(general_purpose::STANDARD.encode(code_bytes))
}

/// Parses the endpoint string and extracts the host name and access key.
///
/// # Arguments
///
/// * `endpoint` - A string slice that holds the endpoint string.
///
/// # Returns
///
/// * `Result<EndPointParams, String>` - The parsed endpoint parameters or an error message.
pub fn parse_endpoint(endpoint: &str) -> Result<EndPointParams, String> {
    debug!("Parsing endpoint");
    let parameters: Vec<&str> = endpoint.split(';').collect();
    if parameters.len() != 2 {
        return Err("Connection string must contain exactly two parameters".to_string());
    }

    let mut end_point_params = EndPointParams {
        host_name: String::new(),
        access_key: String::new(),
    };

    for param in parameters {
        if let Some(host) = param.strip_prefix("endpoint=") {
            let parsed_url =
                Url::parse(host).map_err(|e| format!("Invalid endpoint URL: {}", e))?;
            end_point_params.host_name = parsed_url
                .host_str()
                .ok_or_else(|| "Missing host in endpoint URL".to_string())?
                .to_string();
            debug!("Host name: {}", end_point_params.host_name);
        } else if let Some(key) = param.strip_prefix("accesskey=") {
            end_point_params.access_key = key.to_string();
            debug!("Access key: {}", end_point_params.access_key);
        } else {
            return Err("Invalid parameter in connection string".to_string());
        }
    }

    Ok(end_point_params)
}

/// Creates the request headers for the given parameters.
///
/// # Arguments
///
/// * `url_endpoint` - A reference to the `Url` struct representing the endpoint URL.
/// * `http_method` - A string slice that holds the HTTP method.
/// * `request_id` - A string slice that holds the request ID.
/// * `json_payload` - A string slice that holds the JSON payload.
/// * `access_key` - A string slice that holds the access key.
///
/// # Returns
///
/// * `Result<HeaderMap, String>` - The created request headers or an error message.
pub fn get_request_header(
    url_endpoint: &Url,
    http_method: &str,
    request_id: &str,
    json_payload: &str,
    access_key: &str,
) -> Result<HeaderMap, String> {
    let mut headers = HeaderMap::new();
    let content_hash = compute_content_sha256(json_payload);
    let now = SystemTime::now();
    let http_date = fmt_http_date(now);

    headers.insert("Content-Type", "application/json".parse().unwrap());
    headers.insert("repeatability-request-id", request_id.parse().unwrap());
    headers.insert("repeatability-first-sent", http_date.parse().unwrap());
    headers.insert("x-ms-date", http_date.parse().unwrap());
    headers.insert("x-ms-content-sha256", content_hash.parse().unwrap());

    let host_authority = url_endpoint
        .host_str()
        .ok_or_else(|| "Missing host in URL".to_string())?;
    let path_and_query = match url_endpoint.query() {
        Some(query) => format!("{}?{}", url_endpoint.path(), query),
        None => url_endpoint.path().to_string(),
    };
    let string_to_sign = format!(
        "{}\n{}\n{};{};{}",
        http_method, path_and_query, http_date, host_authority, content_hash
    );
    debug!("String to sign:\n{}", string_to_sign);

    let signature = compute_signature(&string_to_sign, access_key)?;
    let authorization = format!(
        "HMAC-SHA256 SignedHeaders=x-ms-date;host;x-ms-content-sha256&Signature={}",
        signature
    );
    headers.insert("Authorization", authorization.parse().unwrap());

    Ok(headers)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests that `compute_signature` returns an error for an invalid secret.
    #[test]
    fn compute_signature_invalid_secret() {
        let string_to_sign = "string to sign";
        let secret = "invalid base64";
        let result = compute_signature(string_to_sign, secret);
        assert!(result.is_err());
    }

    /// Tests that `parse_endpoint` correctly parses a valid endpoint string.
    #[test]
    fn parse_endpoint_valid_input() {
        let endpoint = "endpoint=https://example.com;accesskey=key";
        let result = parse_endpoint(endpoint).unwrap();
        assert_eq!(result.host_name, "example.com");
        assert_eq!(result.access_key, "key");
    }

    /// Tests that `parse_endpoint` returns an error for an invalid endpoint string.
    #[test]
    fn parse_endpoint_invalid_input() {
        let endpoint = "invalid_endpoint_string";
        let result = parse_endpoint(endpoint);
        assert!(result.is_err());
    }

    /// Tests that `get_request_header` returns the correct headers for valid input.
    #[test]
    fn get_request_header_valid_input() {
        let url = Url::parse("https://example.com/path").unwrap();
        let http_method = "GET";
        let request_id = "request-id";
        let json_payload = "{}";
        let access_key = "c2VjcmV0"; // base64 for "secret"
        let headers =
            get_request_header(&url, http_method, request_id, json_payload, access_key).unwrap();
        assert!(headers.contains_key("Authorization"));
    }

    /// Tests that `get_request_header` returns an error for an invalid access key.
    #[test]
    fn get_request_header_invalid_access_key() {
        let url = Url::parse("https://example.com/path").unwrap();
        let http_method = "GET";
        let request_id = "request-id";
        let json_payload = "{}";
        let access_key = "invalid base64";
        let result = get_request_header(&url, http_method, request_id, json_payload, access_key);
        assert!(result.is_err());
    }

    // ── compute_content_sha256 ────────────────────────────────────────────────

    #[test]
    fn compute_content_sha256_empty_string() {
        // SHA-256("") = e3b0c44298fc1c149afb...  base64 = 47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=
        let hash = compute_content_sha256("");
        assert_eq!(hash, "47DEQpj8HBSa+/TImW+5JCeuQeRkm5NMpJWZG3hSuFU=");
    }

    #[test]
    fn compute_content_sha256_known_value() {
        // SHA-256("{}") base64
        let hash = compute_content_sha256("{}");
        assert!(!hash.is_empty());
        // deterministic — same input always yields same output
        assert_eq!(hash, compute_content_sha256("{}"));
    }

    // ── compute_signature ────────────────────────────────────────────────────

    #[test]
    fn compute_signature_valid_secret_returns_ok() {
        let secret = "c2VjcmV0"; // base64("secret")
        let result = compute_signature("string-to-sign", secret);
        assert!(result.is_ok());
        assert!(!result.unwrap().is_empty());
    }

    #[test]
    fn compute_signature_is_deterministic() {
        let secret = "c2VjcmV0";
        let a = compute_signature("msg", secret).unwrap();
        let b = compute_signature("msg", secret).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn compute_signature_differs_for_different_messages() {
        let secret = "c2VjcmV0";
        let a = compute_signature("msg1", secret).unwrap();
        let b = compute_signature("msg2", secret).unwrap();
        assert_ne!(a, b);
    }

    // ── parse_endpoint ────────────────────────────────────────────────────────

    #[test]
    fn parse_endpoint_strips_https_from_host() {
        let result = parse_endpoint("endpoint=https://my.host.com;accesskey=key").unwrap();
        assert_eq!(result.host_name, "my.host.com");
    }

    #[test]
    fn parse_endpoint_missing_accesskey_prefix() {
        let result = parse_endpoint("endpoint=https://example.com;key=value");
        assert!(result.is_err());
    }

    #[test]
    fn parse_endpoint_missing_endpoint_prefix() {
        let result = parse_endpoint("host=https://example.com;accesskey=key");
        assert!(result.is_err());
    }

    #[test]
    fn parse_endpoint_too_few_parts() {
        let result = parse_endpoint("endpoint=https://example.com");
        assert!(result.is_err());
    }

    #[test]
    fn parse_endpoint_too_many_parts() {
        let result = parse_endpoint("endpoint=https://example.com;accesskey=key;extra=val");
        assert!(result.is_err());
    }

    // ── get_request_header – header presence ─────────────────────────────────

    #[test]
    fn get_request_header_contains_all_required_headers() {
        let url = Url::parse("https://example.com/emails:send?api-version=2023-03-31").unwrap();
        let headers = get_request_header(&url, "POST", "req-id", "{}", "c2VjcmV0").unwrap();
        assert!(headers.contains_key("authorization"));
        assert!(headers.contains_key("content-type"));
        assert!(headers.contains_key("x-ms-date"));
        assert!(headers.contains_key("x-ms-content-sha256"));
        assert!(headers.contains_key("repeatability-request-id"));
        assert!(headers.contains_key("repeatability-first-sent"));
    }

    #[test]
    fn get_request_header_authorization_uses_hmac_sha256() {
        let url = Url::parse("https://example.com/path").unwrap();
        let headers = get_request_header(&url, "GET", "id", "{}", "c2VjcmV0").unwrap();
        let auth = headers.get("authorization").unwrap().to_str().unwrap();
        assert!(auth.starts_with("HMAC-SHA256"));
    }
}
