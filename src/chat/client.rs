//! Anthropic Messages API client.
//!
//! `reqwest` (already in deps) — no new dependency. Key handling, request
//! body assembly, retry policy, and response parsing live here. The rest
//! of the chat module never talks to `reqwest` directly.
//!
//! ## Secret hygiene
//!
//! API keys are wrapped in [`ApiKey`], a hand-rolled newtype whose
//! `Debug` impl prints `***` and which is never serialized. The request
//! URL is logged but not the headers; on error paths only `status` and a
//! sanitized message reach the log.
//!
//! ## Retry policy
//!
//! Exponential backoff on `429`, `500`, `502`, `503`, `504`, capped at
//! 3 attempts, total wall-clock budget 30 s. Other errors fail fast.
//! Model-deprecation (404) is non-retryable: log + self-disable composer
//! for 1 hour, then retry once (the 1-hour timer lives in
//! [`ChatState::model_404_backoff_until`](crate::chat::state::ChatState)).
//!
//! ## Cache TTL
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
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

/// Tool names whose dispatch is owned by the Anthropic API itself (server-side
/// tools). Local dispatchers must NOT attempt to handle these — the API will
/// fold the result into the next assistant message. Currently only
/// `web_search_*` is server-side; if Anthropic adds more, extend this list.
pub const WEB_SEARCH_TOOL_NAME_PREFIX: &str = "web_search";

/// True if the named tool is dispatched by the Anthropic API (server-side)
/// rather than by our local dispatcher. Local dispatchers must return a
/// "no local handler" sentinel for these instead of erroring.
pub fn is_server_side_tool(name: &str) -> bool {
    name.starts_with(WEB_SEARCH_TOOL_NAME_PREFIX)
}

/// Runtime flag controlling whether the 1-hour ephemeral cache TTL beta is
/// available. Defaults to `true`; flipped to `false` the first time the
/// API returns a 4xx that mentions the beta header. Once flipped, all
/// subsequent calls demote `Ephemeral1Hour` -> `Ephemeral5Min` and skip
/// the beta header. There is intentionally no path to flip it back —
/// the next process restart re-probes.
static EXTENDED_CACHE_AVAILABLE: AtomicBool = AtomicBool::new(true);

/// True if the extended-cache-ttl beta is currently believed to be
/// available. Callers building requests can use this to choose between
/// `CacheTtl::Ephemeral1Hour` and `CacheTtl::Ephemeral5Min`.
pub fn extended_cache_available() -> bool {
    EXTENDED_CACHE_AVAILABLE.load(Ordering::Relaxed)
}

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
    /// Extended 1-hour ephemeral cache. Requires the
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
    /// Local (client-side) rate limiter held the call past
    /// `rate_limit_wait_max_secs` and gave up. CHAT.md.
    RateLimited { reason: String },
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
            ClientError::RateLimited { reason } => write!(f, "rate-limited locally: {reason}"),
        }
    }
}

impl std::error::Error for ClientError {}

// ---- Live send ----------------------------------------------------------

/// Anthropic Messages endpoint.
const MESSAGES_URL: &str = "https://api.anthropic.com/v1/messages";
/// Pinned API version. CHAT.md
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Beta header for the 1-hour ephemeral cache TTL.
const EXTENDED_CACHE_BETA: &str = "extended-cache-ttl-2025-04-11";

/// Process-wide reqwest client. Singleton so connection pooling
/// amortizes TLS handshakes — the same pattern used by the Mojang
/// resolver in [`crate::types::user`].
fn http_client() -> &'static reqwest::Client {
    use std::sync::OnceLock;
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            // 15 s per-attempt budget. The retry layer caps total wall-clock
            // at 30 s with 1 s + 2 s backoff between attempts; a
            // 60 s per-attempt timeout would let a single hung attempt blow
            // through the entire budget. 15 s allows 2 attempts + backoff to
            // fit inside 30 s while still tolerating slow networks.
            .timeout(std::time::Duration::from_secs(15))
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
///
/// If the runtime flag [`extended_cache_available`] is `false` (the
/// API previously rejected the beta), the header is suppressed and any
/// `1h` TTL fields in the request body are demoted to 5-min on the
/// fly. This keeps callers correct without forcing them to re-thread
/// the flag through every request build site.
pub async fn send_one(
    api_key: &ApiKey,
    request: &CreateMessageRequest,
    use_extended_cache: bool,
) -> Result<CreateMessageResponse, ClientError> {
    // Runtime flag wins: if the beta has been disabled, demote the
    // request body and skip the header regardless of what the caller
    // asked for. The clone is cheap relative to the network round-trip.
    let beta_live = EXTENDED_CACHE_AVAILABLE.load(Ordering::Relaxed);
    let send_beta = use_extended_cache && beta_live;
    let body_owned;
    let request_to_send: &CreateMessageRequest = if !beta_live {
        body_owned = demote_request_to_5min(request);
        &body_owned
    } else {
        request
    };

    let mut req = http_client()
        .post(MESSAGES_URL)
        .header("x-api-key", api_key.expose())
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(request_to_send);
    if send_beta {
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
                model = %request_to_send.model,
                body = %safe,
                "[Chat] anthropic model 404 (deprecated?)"
            );
            Err(ClientError::ModelNotFound {
                model: request_to_send.model.clone(),
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
            // Beta-header rejection detection. If
            // the API rebuffs the extended-cache-ttl beta, flip the flag
            // off so subsequent calls demote to 5-min and skip the header.
            // Detection is conservative: only flip for 4xx whose body
            // string mentions the beta name, OR a 400 whose body mentions
            // "beta". The caller (`call_with_retry`) re-runs the same
            // request once after we flip.
            if send_beta && is_beta_rejection(s, &body) {
                let was_on = EXTENDED_CACHE_AVAILABLE.swap(false, Ordering::Relaxed);
                if was_on {
                    tracing::warn!(
                        "[Chat] extended-cache-ttl beta unavailable; falling back to 5-minute cache"
                    );
                }
            }
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

/// True if a 4xx response body indicates the extended-cache-ttl beta is
/// not honored by the API. Covers the explicit beta-header name and a
/// generic "beta" mention on a 400. Case-insensitive on the substring
/// check because Anthropic error bodies have varied capitalization in
/// the past.
fn is_beta_rejection(status: u16, body: &str) -> bool {
    if !(400..500).contains(&status) {
        return false;
    }
    let lower = body.to_ascii_lowercase();
    if lower.contains("extended-cache-ttl") {
        return true;
    }
    status == 400 && lower.contains("beta")
}

/// Return a copy of `req` with every `Ephemeral1Hour` `cache_control`
/// demoted to `Ephemeral5Min`. Used when the beta header has been
/// disabled at runtime — sending `ttl: "1h"` without the beta header is
/// what the API rejects, so we strip both together.
fn demote_request_to_5min(req: &CreateMessageRequest) -> CreateMessageRequest {
    let demote_cc = |cc: &Option<CacheControl>| -> Option<CacheControl> {
        cc.as_ref().map(|c| {
            if c.ttl.as_deref() == Some("1h") {
                CacheControl::ephemeral(CacheTtl::Ephemeral5Min)
            } else {
                c.clone()
            }
        })
    };

    let system = req
        .system
        .iter()
        .map(|b| match b {
            SystemBlock::Text { text, cache_control } => SystemBlock::Text {
                text: text.clone(),
                cache_control: demote_cc(cache_control),
            },
        })
        .collect();

    let messages = req
        .messages
        .iter()
        .map(|m| Message {
            role: m.role,
            content: m
                .content
                .iter()
                .map(|cb| match cb {
                    ContentBlock::Text { text, cache_control } => ContentBlock::Text {
                        text: text.clone(),
                        cache_control: demote_cc(cache_control),
                    },
                    ContentBlock::ToolUse { id, name, input } => ContentBlock::ToolUse {
                        id: id.clone(),
                        name: name.clone(),
                        input: input.clone(),
                    },
                    ContentBlock::ToolResult { tool_use_id, content, is_error } => {
                        ContentBlock::ToolResult {
                            tool_use_id: tool_use_id.clone(),
                            content: content.clone(),
                            is_error: *is_error,
                        }
                    }
                })
                .collect(),
        })
        .collect();

    CreateMessageRequest {
        model: req.model.clone(),
        max_tokens: req.max_tokens,
        system,
        messages,
        temperature: req.temperature,
        tools: req.tools.clone(),
    }
}

/// Wrapper around [`send_one`] that drives the [`retry_decision`]
/// policy: up to 3 attempts total, exponential backoff on 429/5xx.
/// Total wall-clock budget capped at 30 s.
///
/// Also handles the one-shot beta-rejection retry: if `send_one` flips
/// [`extended_cache_available`] to `false` mid-call, we retry the same
/// request immediately with the demoted TTL so the user-visible call
/// still succeeds.
pub async fn call_with_retry(
    api_key: &ApiKey,
    request: &CreateMessageRequest,
    use_extended_cache: bool,
) -> Result<CreateMessageResponse, ClientError> {
    let started = std::time::Instant::now();
    let budget = std::time::Duration::from_secs(30);
    let mut attempt: u32 = 0;
    let mut beta_retry_used = false;
    loop {
        let beta_was_on = EXTENDED_CACHE_AVAILABLE.load(Ordering::Relaxed);
        let res = send_one(api_key, request, use_extended_cache).await;
        match res {
            Ok(r) => return Ok(r),
            Err(e) => {
                // One-shot beta demotion retry: if the call failed AND
                // the beta flag was just flipped off by `send_one`, the
                // demoted body should now succeed. Doesn't consume a
                // regular retry slot.
                if beta_was_on
                    && !EXTENDED_CACHE_AVAILABLE.load(Ordering::Relaxed)
                    && !beta_retry_used
                    && started.elapsed() < budget
                {
                    beta_retry_used = true;
                    tracing::info!(
                        "[Chat] retrying call once with demoted 5-min cache TTL after beta rejection"
                    );
                    continue;
                }
                let status = match &e {
                    ClientError::Throttled { status } => *status,
                    ClientError::Auth => 401,
                    ClientError::ModelNotFound { .. } => 404,
                    ClientError::BadRequest { status, .. } => *status,
                    ClientError::Transport(_) => 503, // treat as retryable transient
                    ClientError::Decode(_) => 0,
                    ClientError::RateLimited { .. } => return Err(e),
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
/// number (0-indexed). CHAT.md: exponential backoff on
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

// ---- Per-model rate limiter -------------------------------

/// Per-model rate limiter: tracks requests-per-minute (RPM) and
/// input-tokens-per-minute (ITPM) in two sliding 60-second windows.
/// Acquire blocks (await) up to `wait_max_secs` before erroring.
///
/// Cheap to clone — internally just an `Arc<Mutex<_>>`. One limiter per
/// model (composer, classifier) is the intended deployment shape.
#[derive(Clone)]
pub struct RateLimiter {
    inner: std::sync::Arc<tokio::sync::Mutex<RateLimiterInner>>,
}

struct RateLimiterInner {
    rpm_max: u32,
    itpm_max: u32,
    wait_max_secs: u32,
    /// One entry per accepted request, timestamped at acquire time.
    requests: std::collections::VecDeque<std::time::Instant>,
    /// One entry per accepted request, with the estimated input-token
    /// weight that was charged. Same length as `requests`.
    tokens: std::collections::VecDeque<(std::time::Instant, u32)>,
}

impl RateLimiter {
    /// Build a new limiter with the given caps. `wait_max_secs == 0`
    /// degenerates to "fail immediately if the call doesn't fit", which
    /// is a legitimate operator choice but produces noisy errors —
    /// `validate()` rejects 0 in `ChatConfig`.
    pub fn new(rpm_max: u32, itpm_max: u32, wait_max_secs: u32) -> Self {
        Self {
            inner: std::sync::Arc::new(tokio::sync::Mutex::new(RateLimiterInner {
                rpm_max,
                itpm_max,
                wait_max_secs,
                requests: std::collections::VecDeque::new(),
                tokens: std::collections::VecDeque::new(),
            })),
        }
    }

    /// Block (await) until the call (with `estimated_input_tokens`)
    /// fits under both limits, or return [`ClientError::RateLimited`]
    /// after `wait_max_secs` of waiting. On success, the call is
    /// recorded under `Instant::now()` so concurrent acquirers see it
    /// in the window.
    pub async fn acquire(&self, estimated_input_tokens: u32) -> Result<(), ClientError> {
        let started = std::time::Instant::now();
        let window = std::time::Duration::from_secs(60);
        loop {
            let wait_max_secs;
            {
                let mut g = self.inner.lock().await;
                wait_max_secs = g.wait_max_secs;
                let now = std::time::Instant::now();
                // Prune entries older than 60 s.
                while let Some(&t) = g.requests.front() {
                    if now.duration_since(t) >= window {
                        g.requests.pop_front();
                    } else {
                        break;
                    }
                }
                while let Some(&(t, _)) = g.tokens.front() {
                    if now.duration_since(t) >= window {
                        g.tokens.pop_front();
                    } else {
                        break;
                    }
                }
                let cur_rpm = g.requests.len() as u32;
                let cur_itpm: u32 = g.tokens.iter().map(|(_, n)| *n).sum();
                let rpm_ok = cur_rpm < g.rpm_max;
                let itpm_ok = cur_itpm.saturating_add(estimated_input_tokens) <= g.itpm_max;
                if rpm_ok && itpm_ok {
                    g.requests.push_back(now);
                    g.tokens.push_back((now, estimated_input_tokens));
                    return Ok(());
                }
                if started.elapsed() >= std::time::Duration::from_secs(wait_max_secs as u64) {
                    return Err(ClientError::RateLimited {
                        reason: format!(
                            "waited {}s; rpm={}/{}, itpm={}/{} (need +{} tokens)",
                            wait_max_secs,
                            cur_rpm,
                            g.rpm_max,
                            cur_itpm,
                            g.itpm_max,
                            estimated_input_tokens
                        ),
                    });
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }
}

// ---- Model-404 backoff ----------------------------------------

/// Returns true if the given UTC-ISO `backoff_until` (from
/// `state.model_404_backoff_until`) is in the future. Callers consult
/// this BEFORE dispatching a composer call so a model-deprecation 404
/// engages a 1-hour cool-off without re-hitting the API.
///
/// Unparseable timestamps are treated as "not backed off" so a corrupt
/// state file fails open — losing replies for an hour because of a
/// state-file parse bug would be worse than the brief retry burst.
pub fn is_model_404_backed_off(backoff_until: Option<&str>) -> bool {
    match backoff_until {
        Some(s) => match chrono::DateTime::parse_from_rfc3339(s) {
            Ok(t) => t.with_timezone(&chrono::Utc) > chrono::Utc::now(),
            Err(_) => false,
        },
        None => false,
    }
}

/// Compute the new backoff timestamp for a model-404 (1 hour from now
/// ). Format is RFC 3339 with seconds precision and `Z`
/// suffix so it's stable across reloads.
pub fn model_404_backoff_until_now_plus_1h() -> String {
    use chrono::SecondsFormat;
    (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339_opts(SecondsFormat::Secs, true)
}

// ---- Startup spend estimate ------------------------------

/// Build a one-line log entry summarizing the worst-case daily spend
/// implied by the configured token caps. Composer input/output use
/// `composer_model` rates; classifier uses `classifier_model` rates.
/// The "effective ceiling" is the lower of the sum-of-cap-USDs and the
/// hard `daily_dollar_cap_usd`.
pub fn format_daily_ceiling_log_line(
    config: &crate::config::ChatConfig,
    pricing: &crate::chat::pricing::PricingTable,
) -> String {
    let composer = &config.composer_model;
    let classifier = &config.classifier_model;
    let input_usd =
        pricing.usd_for_tokens(composer, config.daily_input_token_cap, 0);
    let output_usd =
        pricing.usd_for_tokens(composer, 0, config.daily_output_token_cap);
    let classifier_usd =
        pricing.usd_for_tokens(classifier, config.daily_classifier_token_cap, 0);
    let token_sum = input_usd + output_usd + classifier_usd;
    let effective = token_sum.min(config.daily_dollar_cap_usd);
    format!(
        "daily caps: input={} tokens (~${:.2}/day), output={} tokens (~${:.2}/day), \
         classifier={} tokens (~${:.2}/day), USD cap: ${:.2}. Effective daily ceiling: ${:.2}",
        humanize_count(config.daily_input_token_cap),
        input_usd,
        humanize_count(config.daily_output_token_cap),
        output_usd,
        humanize_count(config.daily_classifier_token_cap),
        classifier_usd,
        config.daily_dollar_cap_usd,
        effective,
    )
}

/// Compact integer formatter: 2_000_000 -> "2M", 500_000 -> "500K",
/// 1_500 -> "1.5K", 42 -> "42". Used only by the startup ceiling line —
/// not exposed broadly because the rounding is not lossless.
fn humanize_count(n: u64) -> String {
    if n >= 1_000_000 {
        let m = (n as f64) / 1_000_000.0;
        if (m - m.round()).abs() < 0.05 {
            format!("{:.0}M", m)
        } else {
            format!("{:.1}M", m)
        }
    } else if n >= 1_000 {
        let k = (n as f64) / 1_000.0;
        if (k - k.round()).abs() < 0.05 {
            format!("{:.0}K", k)
        } else {
            format!("{:.1}K", k)
        }
    } else {
        n.to_string()
    }
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
        // CHAT.md: usage carries cache_creation_input_tokens and
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
        // CHAT.md: capped at 3 attempts (initial + 2 retries).
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

    // ---- Server-side tool sentinel -------------------------------------

    #[test]
    fn web_search_recognized_as_server_side() {
        assert!(is_server_side_tool("web_search"));
        assert!(is_server_side_tool("web_search_20250305"));
    }

    #[test]
    fn local_tool_names_not_server_side() {
        assert!(!is_server_side_tool("read_my_memory"));
        assert!(!is_server_side_tool("web_fetch"));
        assert!(!is_server_side_tool("websearch")); // missing underscore
    }

    // ---- Model-404 helpers ---------------------------------------------

    #[test]
    fn model_404_none_means_not_backed_off() {
        assert!(!is_model_404_backed_off(None));
    }

    #[test]
    fn model_404_past_means_not_backed_off() {
        let past = (chrono::Utc::now() - chrono::Duration::hours(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        assert!(!is_model_404_backed_off(Some(&past)));
    }

    #[test]
    fn model_404_future_means_backed_off() {
        let fut = (chrono::Utc::now() + chrono::Duration::hours(1))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        assert!(is_model_404_backed_off(Some(&fut)));
    }

    #[test]
    fn model_404_unparseable_means_not_backed_off() {
        // Fail-open on corrupt state file.
        assert!(!is_model_404_backed_off(Some("not-a-timestamp")));
    }

    #[test]
    fn model_404_helper_emits_future_timestamp() {
        let s = model_404_backoff_until_now_plus_1h();
        assert!(is_model_404_backed_off(Some(&s)));
    }

    // ---- Beta rejection detection --------------------------------------

    #[test]
    fn detects_beta_header_name_in_body() {
        assert!(is_beta_rejection(
            400,
            r#"{"error":"unknown anthropic-beta: extended-cache-ttl-2025-04-11"}"#,
        ));
    }

    #[test]
    fn detects_generic_400_beta_mention() {
        assert!(is_beta_rejection(400, "beta header rejected"));
    }

    #[test]
    fn does_not_detect_beta_on_500() {
        assert!(!is_beta_rejection(500, "beta something"));
    }

    #[test]
    fn does_not_detect_beta_on_unrelated_400() {
        assert!(!is_beta_rejection(400, "missing required field 'model'"));
    }

    // ---- TTL demotion --------------------------------------------------

    #[test]
    fn demote_request_replaces_1h_with_5min_in_system_block() {
        let req = CreateMessageRequest {
            model: "claude-opus-4-7".to_string(),
            max_tokens: 100,
            system: vec![SystemBlock::Text {
                text: "sys".to_string(),
                cache_control: Some(CacheControl::ephemeral(CacheTtl::Ephemeral1Hour)),
            }],
            messages: vec![],
            temperature: None,
            tools: vec![],
        };
        let out = demote_request_to_5min(&req);
        match &out.system[0] {
            SystemBlock::Text { cache_control: Some(cc), .. } => {
                assert!(cc.ttl.is_none(), "ttl field should be absent on 5-min");
            }
            _ => panic!("expected text system block"),
        }
    }

    #[test]
    fn demote_request_preserves_5min_blocks_unchanged() {
        let req = CreateMessageRequest {
            model: "m".to_string(),
            max_tokens: 1,
            system: vec![SystemBlock::Text {
                text: "x".to_string(),
                cache_control: Some(CacheControl::ephemeral(CacheTtl::Ephemeral5Min)),
            }],
            messages: vec![],
            temperature: None,
            tools: vec![],
        };
        let out = demote_request_to_5min(&req);
        match &out.system[0] {
            SystemBlock::Text { cache_control: Some(cc), .. } => assert!(cc.ttl.is_none()),
            _ => panic!(),
        }
    }

    // ---- Rate limiter --------------------------------------------------

    #[tokio::test]
    async fn rate_limiter_allows_calls_under_caps() {
        let rl = RateLimiter::new(10, 10_000, 1);
        // Three calls well under both caps must all succeed instantly.
        for _ in 0..3 {
            rl.acquire(100).await.expect("under caps");
        }
    }

    #[tokio::test]
    async fn rate_limiter_blocks_when_rpm_exceeded() {
        let rl = RateLimiter::new(2, 1_000_000, 1);
        rl.acquire(1).await.unwrap();
        rl.acquire(1).await.unwrap();
        // Third call within the same 60s window must time out after
        // ~1s of polling.
        let started = std::time::Instant::now();
        let res = rl.acquire(1).await;
        let waited = started.elapsed();
        assert!(matches!(res, Err(ClientError::RateLimited { .. })));
        assert!(waited >= std::time::Duration::from_millis(900),
                "expected ~1s wait, got {:?}", waited);
    }

    #[tokio::test]
    async fn rate_limiter_blocks_when_itpm_exceeded() {
        let rl = RateLimiter::new(1_000, 1_000, 1);
        rl.acquire(800).await.unwrap();
        // Adding 300 would put us at 1100 > 1000.
        let res = rl.acquire(300).await;
        assert!(matches!(res, Err(ClientError::RateLimited { .. })));
    }

    #[tokio::test]
    async fn rate_limiter_evicts_old_entries_after_60s() {
        // Use tokio's mock clock to avoid actually sleeping 60 s.
        let rl = RateLimiter::new(1, 1_000, 1);
        rl.acquire(10).await.unwrap();

        // Manually age the recorded entry past the window.
        {
            let mut g = rl.inner.lock().await;
            let aged = std::time::Instant::now() - std::time::Duration::from_secs(61);
            g.requests.clear();
            g.requests.push_back(aged);
            g.tokens.clear();
            g.tokens.push_back((aged, 10));
        }

        // The aged entry must be pruned and the next acquire succeed.
        rl.acquire(10).await.expect("aged entry should be evicted");
    }

    // ---- Daily ceiling formatter ---------------------------------------

    #[test]
    fn daily_ceiling_log_line_uses_token_sum_when_smaller() {
        let mut cfg = crate::config::ChatConfig::default();
        // Force the token-USD sum below the dollar cap.
        cfg.daily_input_token_cap = 1_000;
        cfg.daily_output_token_cap = 100;
        cfg.daily_classifier_token_cap = 1_000;
        cfg.daily_dollar_cap_usd = 100.00;
        let pricing = crate::chat::pricing::PricingTable::default_table();
        let line = format_daily_ceiling_log_line(&cfg, &pricing);
        assert!(line.contains("daily caps"));
        assert!(line.contains("input=1K tokens"));
        assert!(line.contains("Effective daily ceiling"));
    }

    #[test]
    fn daily_ceiling_log_line_uses_dollar_cap_when_smaller() {
        let mut cfg = crate::config::ChatConfig::default();
        cfg.daily_dollar_cap_usd = 0.01; // tighter than any token sum
        let pricing = crate::chat::pricing::PricingTable::default_table();
        let line = format_daily_ceiling_log_line(&cfg, &pricing);
        assert!(line.contains("Effective daily ceiling: $0.01"),
                "got: {line}");
    }

    #[test]
    fn humanize_count_handles_round_and_fractional() {
        assert_eq!(humanize_count(0), "0");
        assert_eq!(humanize_count(42), "42");
        assert_eq!(humanize_count(1_000), "1K");
        assert_eq!(humanize_count(1_500), "1.5K");
        assert_eq!(humanize_count(2_000_000), "2M");
        assert_eq!(humanize_count(2_500_000), "2.5M");
    }
}
