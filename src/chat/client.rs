//! Anthropic Messages API client.
//!
//! `reqwest` (already in deps) — no new dependency. Key handling, request
//! body assembly, retry policy, and response parsing live here. The rest
//! of the chat module never talks to `reqwest` directly.
//!
//! ## Secret hygiene (PLAN §7 S13)
//!
//! API keys are wrapped in [`ApiKey`], a hand-rolled newtype whose
//! `Debug` impl prints `***` and which is never serialized. The request
//! URL is logged but not the headers; on error paths only `status` and a
//! sanitized message reach the log.
//!
//! ## Retry policy (PLAN §7)
//!
//! Exponential backoff on `429`, `500`, `502`, `503`, `504`, capped at
//! 3 attempts, total wall-clock budget 30 s. Other errors fail fast.
//! Model-deprecation (404) is non-retryable: log + self-disable composer
//! for 1 hour, then retry once (the 1-hour timer lives in
//! [`ChatState::model_404_backoff_until`](crate::chat::state::ChatState)).
//!
//! ## Cache TTL (PLAN §7 P3)
//!
//! Use the **1-hour ephemeral cache TTL** (beta header
//! `extended-cache-ttl-2025-04-11`) on the cached blocks. The default
//! 5-min TTL would force cache writes on every quiet-period composer
//! call with no hit; the 1-hour variant costs 2× write but amortizes
//! 12× longer. If the beta is unavailable (a future API change), the
//! caller should fall back to 5-min — handled via [`CacheTtl`].
//!
//! Phase 3 lands the types and retry decision logic. The actual
//! `send_message_request` function is thin and untested — its dependents
//! (composer in Phase 4, classifier-call in Phase 4) are where
//! integration coverage will land.

use std::fmt;

use serde::{Deserialize, Serialize};

/// API key wrapper. `Debug` and `Display` both redact.
#[derive(Clone)]
pub struct ApiKey(String);

impl ApiKey {
    /// Construct from the value of the env var named in
    /// `chat.api_key_env`. Empty / whitespace-only keys are rejected
    /// loud — caller logs at error and self-disables.
    pub fn from_env(env_var: &str) -> Result<Self, String> {
        match std::env::var(env_var) {
            Ok(v) if !v.trim().is_empty() => Ok(ApiKey(v)),
            Ok(_) => Err(format!("env var {env_var} is set but empty")),
            Err(_) => Err(format!("env var {env_var} is not set")),
        }
    }

    /// Test-only constructor. Production code must use `from_env`.
    #[cfg(test)]
    pub fn test_value(s: &str) -> Self {
        ApiKey(s.to_string())
    }

    /// Borrow the underlying secret. Use exactly once at the
    /// `header()` call site; do not store the borrow elsewhere.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ApiKey(***)")
    }
}

impl fmt::Display for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "***")
    }
}

/// Cache TTL choice for a `cache_control` block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheTtl {
    /// Default 5-minute ephemeral cache.
    Ephemeral5Min,
    /// Extended 1-hour ephemeral cache (PLAN §7 P3). Requires the
    /// `extended-cache-ttl-2025-04-11` beta header on the request.
    Ephemeral1Hour,
}

impl CacheTtl {
    /// String form expected by the Anthropic API in the `cache_control`
    /// `ttl` field. The 5-min variant omits `ttl` because that's the API
    /// default, and serializing `null` would be misleading.
    pub fn as_ttl_field(self) -> Option<&'static str> {
        match self {
            CacheTtl::Ephemeral5Min => None,
            CacheTtl::Ephemeral1Hour => Some("1h"),
        }
    }

    /// Whether this TTL choice requires the beta header.
    pub fn needs_extended_beta(self) -> bool {
        matches!(self, CacheTtl::Ephemeral1Hour)
    }
}

// ---- Request types ------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct CreateMessageRequest {
    pub model: String,
    pub max_tokens: u32,
    pub system: Vec<SystemBlock>,
    pub messages: Vec<Message>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<Tool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SystemBlock {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cache_control: Option<CacheControl>,
    },
    ToolUse {
        id: String,
        name: String,
        input: serde_json::Value,
    },
    ToolResult {
        tool_use_id: String,
        content: String,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        is_error: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheControl {
    /// `"ephemeral"` is the only documented type as of late-2025.
    #[serde(rename = "type")]
    pub kind: String,
    /// `Some("1h")` for 1-hour beta cache; omitted for 5-min default.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub ttl: Option<String>,
}

impl CacheControl {
    pub fn ephemeral(ttl: CacheTtl) -> Self {
        Self {
            kind: "ephemeral".to_string(),
            ttl: ttl.as_ttl_field().map(str::to_string),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

// ---- Response types -----------------------------------------------------

#[derive(Debug, Clone, Deserialize)]
pub struct CreateMessageResponse {
    pub id: String,
    pub model: String,
    pub role: Role,
    pub content: Vec<ContentBlock>,
    pub stop_reason: Option<String>,
    pub usage: Usage,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct Usage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

// ---- Errors -------------------------------------------------------------

#[derive(Debug)]
pub enum ClientError {
    /// Network / transport failure (timeout, DNS, TLS).
    Transport(String),
    /// Authentication failed (HTTP 401). Do NOT retry.
    Auth,
    /// Model not found (HTTP 404). Caller should engage a 1-hour backoff.
    ModelNotFound { model: String },
    /// Rate limited / 5xx after retries are exhausted. Caller can retry
    /// later but should not loop tightly.
    Throttled { status: u16 },
    /// 4xx other than 401/404 — the request is malformed in a way that
    /// won't be fixed by retrying.
    BadRequest { status: u16, message: String },
    /// Response body could not be parsed as the expected JSON shape.
    Decode(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ClientError::Transport(s) => write!(f, "transport: {s}"),
            ClientError::Auth => write!(f, "anthropic auth failed (401)"),
            ClientError::ModelNotFound { model } => write!(f, "model not found: {model}"),
            ClientError::Throttled { status } => write!(f, "throttled / 5xx (status={status})"),
            ClientError::BadRequest { status, message } => {
                write!(f, "bad request (status={status}): {message}")
            }
            ClientError::Decode(s) => write!(f, "decode: {s}"),
        }
    }
}

impl std::error::Error for ClientError {}

// ---- Live send ----------------------------------------------------------

/// Anthropic Messages endpoint.
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
/// Pinned API version. PLAN §7.
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Beta header for the 1-hour ephemeral cache TTL (PLAN §7 P3).
const EXTENDED_CACHE_BETA: &str = "extended-cache-ttl-2025-04-11";

/// Process-wide reqwest client. Singleton so connection pooling
/// amortizes TLS handshakes — the same pattern used by the Mojang
/// resolver in [`crate::types::user`].
fn http_client() -> &'static reqwest::Client {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            // 60 s per-attempt budget; the retry layer above adds the
            // exponential backoff on top.
            .timeout(std::time::Duration::from_secs(60))
            .build()
            .expect("Failed to create Anthropic HTTP client")
    })
}

/// Send one Messages API request. Single attempt — the retry loop
/// belongs to the caller, which threads in the
/// [`retry_decision`]-driven sleep between attempts.
///
/// `use_extended_cache` controls whether the
/// `anthropic-beta: extended-cache-ttl-2025-04-11` header is sent. The
/// caller decides this based on whether any `cache_control` block in
/// the request needs the 1-hour TTL.
pub async fn send_one(
    api_key: &ApiKey,
    request: &CreateMessageRequest,
    use_extended_cache: bool,
) -> Result<CreateMessageResponse, ClientError> {
    let mut req = http_client()
        .post(MESSAGES_URL)
        .header("x-api-key", api_key.expose())
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(request);
    if use_extended_cache {
        req = req.header("anthropic-beta", EXTENDED_CACHE_BETA);
    }

    let response = req.send().await.map_err(|e| {
        // `e` may include the URL but not the headers we set; reqwest
        // already redacts the body of the source request. Still safe.
        ClientError::Transport(e.to_string())
    })?;

    let status = response.status();
    if status.is_success() {
        return response
            .json::<CreateMessageResponse>()
            .await
            .map_err(|e| ClientError::Decode(e.to_string()));
    }

    // Non-2xx — read body once for logging, then map to error variant.
    let body = response.text().await.unwrap_or_default();
    let safe = sanitize_for_log(&body);
    match status.as_u16() {
        401 => {
            tracing::error!(
                status = %status,
                body = %safe,
                "[Chat] anthropic auth failed (401)"
            );
            Err(ClientError::Auth)
        }
        404 => {
            tracing::error!(
                status = %status,
                model = %request.model,
                body = %safe,
                "[Chat] anthropic model 404 (deprecated?)"
            );
            Err(ClientError::ModelNotFound {
                model: request.model.clone(),
            })
        }
        429 | 500 | 502 | 503 | 504 => {
            tracing::warn!(
                status = %status,
                body = %safe,
                "[Chat] anthropic throttled / 5xx"
            );
            Err(ClientError::Throttled {
                status: status.as_u16(),
            })
        }
        s => {
            tracing::error!(
                status = %status,
                body = %safe,
                "[Chat] anthropic bad request"
            );
            Err(ClientError::BadRequest {
                status: s,
                message: safe,
            })
        }
    }
}

/// Wrapper around [`send_one`] that drives the [`retry_decision`]
/// policy: up to 3 attempts total, exponential backoff on 429/5xx.
/// Total wall-clock budget capped at 30 s.
pub async fn call_with_retry(
    api_key: &ApiKey,
    request: &CreateMessageRequest,
    use_extended_cache: bool,
) -> Result<CreateMessageResponse, ClientError> {
    let started = std::time::Instant::now();
    let budget = std::time::Duration::from_secs(30);
    let mut attempt: u32 = 0;
    loop {
        let res = send_one(api_key, request, use_extended_cache).await;
        match res {
            Ok(r) => return Ok(r),
            Err(e) => {
                let status = match &e {
                    ClientError::Throttled { status } => *status,
                    ClientError::Auth => 401,
                    ClientError::ModelNotFound { .. } => 404,
                    ClientError::BadRequest { status, .. } => *status,
                    ClientError::Transport(_) => 503, // treat as retryable transient
                    ClientError::Decode(_) => 0,
                };
                let decision = retry_decision(status, attempt);
                match decision {
                    RetryDecision::Stop => return Err(e),
                    RetryDecision::Retry { delay_ms } => {
                        if started.elapsed() + std::time::Duration::from_millis(delay_ms)
                            > budget
                        {
                            return Err(e);
                        }
                        tracing::warn!(
                            attempt,
                            delay_ms,
                            error = %e,
                            "[Chat] retrying anthropic call"
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                        attempt += 1;
                    }
                }
            }
        }
    }
}

// ---- Retry policy -------------------------------------------------------

/// Decision for one retry attempt. Pure function — no clock, no I/O —
/// so the policy can be unit-tested without sleeping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    /// Sleep this many milliseconds and try again.
    Retry { delay_ms: u64 },
    /// Stop. The error is final.
    Stop,
}

/// Decide whether to retry given the just-observed status and attempt
/// number (0-indexed). PLAN §7: exponential backoff on
/// `429, 500, 502, 503, 504`, capped at 3 attempts, total wall-clock
/// budget 30 s.
pub fn retry_decision(status: u16, attempt: u32) -> RetryDecision {
    if attempt >= 2 {
        // Already retried twice; this is the third attempt — no more.
        return RetryDecision::Stop;
    }
    let retryable = matches!(status, 429 | 500 | 502 | 503 | 504);
    if !retryable {
        return RetryDecision::Stop;
    }
    // 1s, 2s, 4s — but the docstring caps total at 30s. `attempt` is
    // bounded by the early-return at the top (≥ 2 stops), so the shift
    // is at most 1, and `1_000 << 1 = 2_000`. No overflow risk.
    let delay_ms = 1_000u64 << attempt;
    RetryDecision::Retry { delay_ms }
}

/// Sanitize a raw API response body for logging. The Anthropic 401 body
/// can include partial key fragments depending on the auth path; the
/// 5xx bodies frequently include request IDs that are useful but no
/// secrets. Strategy: keep at most 200 chars, replace any 32+ char
/// hex/alphanum run with `[redacted]`.
pub fn sanitize_for_log(body: &str) -> String {
    let mut out = String::new();
    let mut run_start: Option<usize> = None;
    let bytes = body.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        let alnum = b.is_ascii_alphanumeric() || b == b'-' || b == b'_';
        if alnum {
            if run_start.is_none() {
                run_start = Some(i);
            }
        } else if let Some(start) = run_start.take() {
            let run = &body[start..i];
            if run.len() >= 32 {
                out.push_str("[redacted]");
            } else {
                out.push_str(run);
            }
            out.push(b as char);
        } else {
            out.push(b as char);
        }
        if out.len() >= 200 {
            out.push_str("...");
            return out;
        }
    }
    if let Some(start) = run_start {
        let run = &body[start..];
        if run.len() >= 32 {
            out.push_str("[redacted]");
        } else {
            out.push_str(run);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- ApiKey ---------------------------------------------------------

    #[test]
    fn api_key_debug_redacts() {
        let k = ApiKey::test_value("sk-ant-secret-12345");
        let dbg = format!("{:?}", k);
        assert!(!dbg.contains("secret"));
        assert!(dbg.contains("***"));
    }

    #[test]
    fn api_key_display_redacts() {
        let k = ApiKey::test_value("sk-ant-secret-12345");
        let s = format!("{}", k);
        assert!(!s.contains("secret"));
        assert_eq!(s, "***");
    }

    #[test]
    fn api_key_from_env_rejects_unset_var() {
        // Use a name no one will set in CI.
        let r = ApiKey::from_env("CJ_STORE_DOES_NOT_EXIST_XYZZY");
        assert!(r.is_err());
        let msg = r.unwrap_err();
        assert!(msg.contains("not set"), "got: {msg}");
    }

    // ---- Cache TTL ------------------------------------------------------

    #[test]
    fn cache_ttl_5min_omits_ttl_field() {
        assert!(CacheTtl::Ephemeral5Min.as_ttl_field().is_none());
        assert!(!CacheTtl::Ephemeral5Min.needs_extended_beta());
    }

    #[test]
    fn cache_ttl_1hour_emits_1h_and_needs_beta() {
        assert_eq!(CacheTtl::Ephemeral1Hour.as_ttl_field(), Some("1h"));
        assert!(CacheTtl::Ephemeral1Hour.needs_extended_beta());
    }

    #[test]
    fn cache_control_5min_serializes_without_ttl_field() {
        let c = CacheControl::ephemeral(CacheTtl::Ephemeral5Min);
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["type"], "ephemeral");
        assert!(json.get("ttl").is_none());
    }

    #[test]
    fn cache_control_1hour_serializes_ttl_field() {
        let c = CacheControl::ephemeral(CacheTtl::Ephemeral1Hour);
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(json["type"], "ephemeral");
        assert_eq!(json["ttl"], "1h");
    }

    // ---- Request shape --------------------------------------------------

    #[test]
    fn system_block_with_cache_control_serializes_correctly() {
        let block = SystemBlock::Text {
            text: "you are helpful".to_string(),
            cache_control: Some(CacheControl::ephemeral(CacheTtl::Ephemeral1Hour)),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "text");
        assert_eq!(json["text"], "you are helpful");
        assert_eq!(json["cache_control"]["type"], "ephemeral");
        assert_eq!(json["cache_control"]["ttl"], "1h");
    }

    #[test]
    fn system_block_without_cache_control_omits_field() {
        let block = SystemBlock::Text {
            text: "hi".to_string(),
            cache_control: None,
        };
        let json = serde_json::to_value(&block).unwrap();
        assert!(json.get("cache_control").is_none());
    }

    #[test]
    fn user_message_with_text_content_serializes() {
        let msg = Message {
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: "hello".to_string(),
                cache_control: None,
            }],
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(json["role"], "user");
        assert_eq!(json["content"][0]["type"], "text");
        assert_eq!(json["content"][0]["text"], "hello");
    }

    #[test]
    fn tool_use_block_round_trips_through_serde() {
        let block = ContentBlock::ToolUse {
            id: "toolu_123".to_string(),
            name: "read_my_memory".to_string(),
            input: serde_json::json!({}),
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["type"], "tool_use");
        let back: ContentBlock = serde_json::from_value(json).unwrap();
        match back {
            ContentBlock::ToolUse { id, name, .. } => {
                assert_eq!(id, "toolu_123");
                assert_eq!(name, "read_my_memory");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn tool_result_omits_is_error_when_false() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "toolu_1".to_string(),
            content: "ok".to_string(),
            is_error: false,
        };
        let json = serde_json::to_value(&block).unwrap();
        assert!(json.get("is_error").is_none());
    }

    #[test]
    fn tool_result_includes_is_error_when_true() {
        let block = ContentBlock::ToolResult {
            tool_use_id: "toolu_1".to_string(),
            content: "boom".to_string(),
            is_error: true,
        };
        let json = serde_json::to_value(&block).unwrap();
        assert_eq!(json["is_error"], true);
    }

    // ---- Response parsing ----------------------------------------------

    #[test]
    fn parses_anthropic_response_shape() {
        let raw = r#"{
            "id": "msg_01",
            "model": "claude-haiku-4-5-20251001",
            "role": "assistant",
            "content": [{"type": "text", "text": "hello"}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 100, "output_tokens": 5}
        }"#;
        let r: CreateMessageResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(r.id, "msg_01");
        assert_eq!(r.role, Role::Assistant);
        assert_eq!(r.usage.input_tokens, 100);
        assert_eq!(r.usage.output_tokens, 5);
        assert_eq!(r.content.len(), 1);
    }

    #[test]
    fn parses_response_with_tool_use_blocks() {
        let raw = r#"{
            "id": "msg_02",
            "model": "claude-opus-4-7",
            "role": "assistant",
            "content": [
                {"type": "text", "text": "let me check"},
                {"type": "tool_use", "id": "toolu_1", "name": "read_my_memory", "input": {}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 500, "output_tokens": 30}
        }"#;
        let r: CreateMessageResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(r.content.len(), 2);
        assert!(matches!(r.content[1], ContentBlock::ToolUse { .. }));
    }

    #[test]
    fn parses_response_with_cache_token_fields() {
        // PLAN §7: usage carries cache_creation_input_tokens and
        // cache_read_input_tokens once prompt caching kicks in.
        let raw = r#"{
            "id": "msg_03",
            "model": "claude-opus-4-7",
            "role": "assistant",
            "content": [{"type": "text", "text": "ok"}],
            "stop_reason": "end_turn",
            "usage": {
                "input_tokens": 50,
                "output_tokens": 5,
                "cache_creation_input_tokens": 800,
                "cache_read_input_tokens": 2000
            }
        }"#;
        let r: CreateMessageResponse = serde_json::from_str(raw).unwrap();
        assert_eq!(r.usage.cache_creation_input_tokens, 800);
        assert_eq!(r.usage.cache_read_input_tokens, 2000);
    }

    // ---- Retry policy ---------------------------------------------------

    #[test]
    fn retry_decision_first_attempt_429_retries_at_1s() {
        let v = retry_decision(429, 0);
        assert_eq!(v, RetryDecision::Retry { delay_ms: 1_000 });
    }

    #[test]
    fn retry_decision_second_attempt_429_retries_at_2s() {
        let v = retry_decision(429, 1);
        assert_eq!(v, RetryDecision::Retry { delay_ms: 2_000 });
    }

    #[test]
    fn retry_decision_third_attempt_stops() {
        // PLAN §7: capped at 3 attempts (initial + 2 retries).
        let v = retry_decision(429, 2);
        assert_eq!(v, RetryDecision::Stop);
    }

    #[test]
    fn retry_decision_for_5xx_retries_same_as_429() {
        for status in [500, 502, 503, 504] {
            assert_eq!(
                retry_decision(status, 0),
                RetryDecision::Retry { delay_ms: 1_000 }
            );
        }
    }

    #[test]
    fn retry_decision_does_not_retry_401() {
        // Auth errors are non-retryable.
        assert_eq!(retry_decision(401, 0), RetryDecision::Stop);
    }

    #[test]
    fn retry_decision_does_not_retry_404() {
        // Model deprecation — caller engages 1h backoff externally.
        assert_eq!(retry_decision(404, 0), RetryDecision::Stop);
    }

    #[test]
    fn retry_decision_does_not_retry_400() {
        assert_eq!(retry_decision(400, 0), RetryDecision::Stop);
    }

    // ---- Log sanitization ----------------------------------------------

    #[test]
    fn sanitize_redacts_long_alnum_runs() {
        let s = sanitize_for_log("error: api_key=sk-ant-1234567890abcdefghijklmnopqrstuvwxyz");
        assert!(!s.contains("sk-ant-1234567890abcdefghijklmnopqrstuvwxyz"));
        assert!(s.contains("[redacted]"));
    }

    #[test]
    fn sanitize_keeps_short_words() {
        let s = sanitize_for_log("Bad request: missing model");
        assert!(s.contains("Bad"));
        assert!(s.contains("missing"));
        assert!(s.contains("model"));
    }

    #[test]
    fn sanitize_caps_long_alnum_input() {
        // A pure-alnum run of 1000 chars should be compressed to
        // "[redacted]" by the run-redact pass — the 200-char cap is
        // exercised by mixed inputs.
        let big = "a".repeat(1000);
        let s = sanitize_for_log(&big);
        assert!(s.len() <= 220, "got len {}: {s}", s.len());
        assert!(s.contains("[redacted]"));
    }

    #[test]
    fn sanitize_caps_at_200_chars_for_mixed_input() {
        // Many short words separated by spaces: the cap kicks in inside
        // the loop and "..." is appended.
        let big: String = (0..500).map(|i| format!("word{i} ")).collect();
        let s = sanitize_for_log(&big);
        assert!(s.len() <= 220, "got len {}: {s}", s.len());
        assert!(s.ends_with("..."));
    }

    #[test]
    fn sanitize_handles_short_alnum_runs_intact() {
        let s = sanitize_for_log("model=claude-opus");
        // "claude-opus" is 11 chars, under the 32-char redact threshold.
        assert!(s.contains("claude-opus"));
    }
}
