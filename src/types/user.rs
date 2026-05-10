//! # User Management
//!
//! Handles user persistence and Mojang UUID lookup.
//! Each user is stored as a separate JSON file: `data/users/{uuid}.json`
//!
//! ## Key Features
//! - UUID-based identity (canonical key, survives username changes)
//! - Diamond balance tracking (f64 for fractional diamonds)
//! - Operator flag for privileged commands (additem, removeitem, addcurrency, removecurrency)
//!
//! ## Mojang API Integration
//! - `get_uuid_async()` calls Mojang's public API to resolve usernames to UUIDs
//! - Returns hyphenated UUID format: `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`
//! - Caching (TTL = `UUID_CACHE_TTL_SECS`) is handled in `crate::mojang::resolve_user_uuid`

use std::{
    collections::{HashMap, HashSet},
    fmt, fs, io,
    path::Path,
    sync::OnceLock,
    time::Duration,
};
#[cfg(test)]
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use tracing::{debug, info, warn};

use crate::fsutil::write_atomic;

// The Mojang lookup path is gated behind `#[cfg(not(test))]` so tests don't
// issue real HTTP requests. The supporting HTTP client, the request struct,
// and `get_uuid_async` therefore only have callers outside test builds — the
// cfg_attr below silences the test-only dead_code warnings without allowing
// dead code in the production build.

/// Hard timeout for a single Mojang UUID lookup. Referenced in the error
/// message so a tuning change cannot make the log lie.
const MOJANG_TIMEOUT_SECS: u64 = 10;

/// Process-wide reqwest client for Mojang API calls. Singleton so connection
/// pooling amortizes TLS handshakes across lookups for the lifetime of the bot.
#[cfg_attr(test, allow(dead_code))]
static HTTP_CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

#[cfg_attr(test, allow(dead_code))]
fn get_http_client() -> &'static reqwest::Client {
    HTTP_CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(MOJANG_TIMEOUT_SECS))
            .connect_timeout(std::time::Duration::from_secs(3))
            .build()
            .expect("Failed to create HTTP client")
    })
}

/// Represents a user in the store system.
///
/// **Persistence**: Saved to `data/users/{uuid}.json`
///
/// **Identity**: UUID is the canonical identifier (survives username changes).
/// Username is updated on each interaction but is not used as a key.
///
/// **Balance**: Stored as `f64` to support fractional diamonds (e.g., from sell orders
/// where bot offers whole diamonds but player receives fractional credit).
///
/// **Operator**: When `true`, user can execute privileged commands:
/// - `additem <item> <quantity>` - Add items to storage
/// - `removeitem <item> <quantity>` - Remove items from storage
/// - `addcurrency <item> <amount>` - Add diamonds to pair reserve
/// - `removecurrency <item> <amount>` - Remove diamonds from pair reserve
///
/// See `README.md` "Player command interface" for command details.
#[derive(Debug, Default, PartialEq, Serialize, Deserialize, Clone)]
pub struct User {
    /// Hyphenated Mojang UUID — canonical identifier (survives username changes).
    pub uuid: String,
    /// Last-seen username; ephemeral, not an identity key.
    pub username: String,
    pub balance: f64,
    /// `#[serde(default)]` so users from pre-operator-flag saves deserialize
    /// as non-operators instead of failing the whole load.
    #[serde(default)]
    pub operator: bool,
}

#[cfg_attr(test, allow(dead_code))]
#[derive(Deserialize)]
struct MojangResponse {
    id: String,
}

/// Typed Mojang-resolver error. The `Display` impl is short and entirely
/// author-controlled — no `reqwest` internals (URLs, header dumps, TLS chain
/// errors) leak through. Convert at the `reqwest::Error` boundary inside
/// `User::get_uuid_async` by inspecting `e.is_connect()` / `e.is_timeout()` /
/// `e.is_status()` / `e.is_decode()`; route every variant to a sanitized
/// `StoreError` at the call site (`StoreError::UserNotFound`,
/// `StoreError::ValidationError`, or `StoreError::MojangNetwork`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MojangResolveError {
    /// Mojang returned 204 No Content — the username does not exist.
    /// `username` is the original (caller-supplied) username for whisper
    /// rendering: "Player 'X' not found" reaches the player verbatim.
    NotFound { username: String },
    /// Username failed the in-process shape gate (3-16 ASCII alphanumeric+`_`)
    /// before any network call. Distinct from `NotFound` so callers can
    /// short-circuit a Mojang round-trip on garbage input.
    InvalidShape,
    /// `reqwest::Error::is_timeout()` — the HTTP client tripped its
    /// per-request timeout. Operator-visible detail goes to logs only.
    NetworkTimeout,
    /// Connection-level failure (DNS, TCP, TLS handshake, etc.) —
    /// `reqwest::Error::is_connect()`. Same operator-only sanitization rule.
    NetworkError,
    /// Non-success HTTP status (`is_status()`) other than the 204 mapped
    /// to `NotFound` and the 429 mapped to `RateLimited`. Generic-message
    /// at the player; full status logged.
    UpstreamError,
    /// Mojang returned 2xx but the body was undecodable JSON, or the
    /// `id` field failed the 32-char lowercase-hex shape check. Same
    /// generic player whisper.
    MalformedResponse,
    /// HTTP 429 Too Many Requests. `retry_after` is `Some(Duration)` if
    /// Mojang sent a parseable `Retry-After` header (integer-seconds or
    /// RFC 2822 absolute date), `None` otherwise. Distinct from
    /// `UpstreamError` so callers can route "rate-limited, retry later"
    /// differently from "upstream broken" (e.g. exponential backoff and
    /// quiet whisper vs. operator alert). The retry-budget logic in
    /// `User::get_uuid_async` deliberately does NOT auto-retry this
    /// variant — repeating the call would only deepen the throttle.
    RateLimited { retry_after: Option<Duration> },
}

impl fmt::Display for MojangResolveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MojangResolveError::NotFound { username } => {
                write!(f, "Player '{username}' not found")
            }
            MojangResolveError::InvalidShape => {
                write!(f, "Invalid Minecraft username")
            }
            MojangResolveError::NetworkTimeout => write!(f, "Mojang API timeout"),
            MojangResolveError::NetworkError => write!(f, "Mojang API network error"),
            MojangResolveError::UpstreamError => write!(f, "Mojang API upstream error"),
            MojangResolveError::MalformedResponse => {
                write!(f, "Mojang API returned malformed response")
            }
            MojangResolveError::RateLimited { retry_after } => match retry_after {
                Some(d) => write!(f, "Mojang API rate-limited (retry after {}s)", d.as_secs()),
                None => write!(f, "Mojang API rate-limited"),
            },
        }
    }
}

impl std::error::Error for MojangResolveError {}

/// Mojang-shape username validator (3-16 chars, ASCII alphanumeric + `_`).
///
/// Single source of truth for the username byte/charset gate — every other
/// caller in the crate (`chat::tools::validate_username_shape`,
/// `mojang::resolve_user_uuid`, `store::handlers::validation::validate_username`)
/// delegates here so a tweak to the rule (e.g. a future Mojang shape change)
/// applies uniformly.
///
/// The byte-length check `(3..=16).contains(&username.len())` is correct
/// even for multi-byte UTF-8 inputs: a 4-byte string like `"ää"` (2 chars,
/// 4 bytes) is rejected on TWO independent grounds — the byte-length is in
/// `[3,16]` so the length gate alone would not save us, but the per-byte
/// `is_ascii_alphanumeric() || b == b'_'` check rejects every high-bit
/// continuation byte. See `is_valid_username_shape_rejects_multibyte_byte_vs_char`.
pub(crate) fn is_valid_username_shape(username: &str) -> bool {
    (3..=16).contains(&username.len())
        && username
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// Upper bound on a parsed `Retry-After` hint. A hostile or buggy upstream
/// could emit something like `Retry-After: 99999999999` which, taken at face
/// value, would propagate into a multi-thousand-year `tokio::time::sleep` —
/// effectively a denial of service on the resolver. 1 hour is generously
/// larger than any reasonable Mojang throttle window while keeping the worst
/// case bounded.
#[cfg_attr(test, allow(dead_code))]
const RETRY_AFTER_MAX_SECS: u64 = 3600;

/// Parse an HTTP `Retry-After` header value into a `Duration`.
///
/// RFC 7231 §7.1.3 permits two encodings:
/// 1. **delta-seconds** — a non-negative integer number of seconds
///    (`"120"`).
/// 2. **HTTP-date** — an absolute timestamp in one of three formats
///    (RFC 1123, RFC 850, asctime). We accept the modern RFC 2822 form
///    (RFC 1123 is a subset of RFC 2822 for the date portion) via
///    `chrono::DateTime::parse_from_rfc2822`, which covers what every
///    real-world server emits in 2026.
///
/// On parse failure (malformed header, negative skew, garbage), returns
/// `None` — the caller treats absence the same as "no hint, fall back to
/// generic backoff". Negative deltas (date already in the past) clamp to
/// `Duration::ZERO` rather than wrapping or producing a sentinel error,
/// because a server that says "retry after a moment ago" is effectively
/// saying "retry immediately".
///
/// Both encodings clamp the upper bound to [`RETRY_AFTER_MAX_SECS`] (1h)
/// so a hostile/buggy upstream cannot weaponize the header into a
/// multi-thousand-year sleep. The clamp emits a `warn!` so an
/// unrealistically large hint is visible in the operator log.
#[cfg_attr(test, allow(dead_code))]
fn parse_retry_after(value: &reqwest::header::HeaderValue) -> Option<Duration> {
    let s = value.to_str().ok()?.trim();
    // Form 1: integer-seconds.
    if let Ok(secs) = s.parse::<u64>() {
        if secs > RETRY_AFTER_MAX_SECS {
            warn!(
                "[Mojang] Retry-After delta-seconds {} clamped to {}",
                secs, RETRY_AFTER_MAX_SECS
            );
            return Some(Duration::from_secs(RETRY_AFTER_MAX_SECS));
        }
        return Some(Duration::from_secs(secs));
    }
    // Form 2: HTTP-date (RFC 2822 / RFC 1123).
    let target = chrono::DateTime::parse_from_rfc2822(s).ok()?;
    let now = chrono::Utc::now();
    let delta = target.signed_duration_since(now);
    if delta <= chrono::Duration::zero() {
        return Some(Duration::ZERO);
    }
    let std_delta = delta.to_std().ok()?;
    if std_delta.as_secs() > RETRY_AFTER_MAX_SECS {
        warn!(
            "[Mojang] Retry-After HTTP-date delta {}s clamped to {}",
            std_delta.as_secs(), RETRY_AFTER_MAX_SECS
        );
        return Some(Duration::from_secs(RETRY_AFTER_MAX_SECS));
    }
    Some(std_delta)
}

/// Canonicalize a Mojang-API `id` field (bare 32-char hex, any case) into
/// the canonical hyphenated lowercase form `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`.
///
/// Owns BOTH the lowercase step AND the shape gate so the slice into the
/// 5-segment hyphenation cannot panic on adversarial input:
/// - `is_ascii_hexdigit` on every byte ensures all bytes are ASCII (so the
///   `&id[0..8]…&id[20..32]` byte slices land on valid UTF-8 char boundaries),
/// - and the length-32 check ensures the indices are in range.
///
/// Extracted from `User::get_uuid_async` so the panic-guard is directly
/// unit-testable. Returns `MojangResolveError::MalformedResponse` on any
/// shape failure (length, non-hex byte, multi-byte non-ASCII, etc).
fn canonicalize_mojang_id(raw: &str) -> Result<String, MojangResolveError> {
    let id = raw.to_ascii_lowercase();
    if id.len() != 32 || !id.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(MojangResolveError::MalformedResponse);
    }
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &id[0..8],
        &id[8..12],
        &id[12..16],
        &id[16..20],
        &id[20..32]
    ))
}

/// UUID-shape validator: canonical 36-char hyphenated lowercase hex
/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`, OR bare 32-char lowercase hex.
/// Rejects uppercase, missing hyphens, wrong length, and any path-separator
/// or `..` content. Strictly broader than the chat-tool boundary
/// validator at `crate::chat::tools::validate_uuid`, which now accepts
/// canonical-hyphenated only — the storage layer keeps the bare-hex
/// arm because Mojang's API returns the bare form and existing on-disk
/// user files were historically written with it. Intentionally
/// duplicated to keep `types::user` chat-independent.
///
/// Defense-in-depth against T15P1: the all-zeros sentinel (in either
/// hyphenated or bare-hex form) is the reserved "no sender resolved"
/// marker chat/mod.rs historically substituted on Mojang failure. It
/// must never reach a storage-path constructor here either, since the
/// chat tool layer is the first perimeter but the storage boundary is
/// the last one.
pub(crate) fn is_valid_uuid_shape(uuid: &str) -> bool {
    if uuid == "00000000-0000-0000-0000-000000000000"
        || uuid == "00000000000000000000000000000000"
    {
        return false;
    }
    let bytes = uuid.as_bytes();
    match bytes.len() {
        32 => bytes
            .iter()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        36 => {
            let hyphen_positions = [8usize, 13, 18, 23];
            for (i, &b) in bytes.iter().enumerate() {
                let expect_hyphen = hyphen_positions.contains(&i);
                if expect_hyphen {
                    if b != b'-' {
                        return false;
                    }
                } else if !(b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) {
                    return false;
                }
            }
            true
        }
        _ => false,
    }
}

impl User {
    // Directory where all individual user files will be stored
    const USERS_DIR: &str = "data/users";

    /// Resolves a Minecraft username to a hyphenated Mojang UUID via
    /// `https://api.mojang.com/users/profiles/minecraft/{username}`.
    /// HTTP 204 → `NotFound`; HTTP 429 → `RateLimited`; other non-2xx or
    /// network errors → typed [`MojangResolveError`]. Rejects out-of-shape
    /// usernames before constructing the URL so a malformed name (slash,
    /// query, fragment, control char) cannot escape the path component
    /// into an attacker-chosen Mojang endpoint.
    ///
    /// Returns a typed error rather than a stringified `reqwest::Error` so
    /// nothing from the underlying HTTP client (URLs, header dumps, TLS
    /// errors) can reach a player whisper. Operator-visible detail is
    /// emitted via the `warn!`/`debug!` log lines below.
    ///
    /// Retries ONCE after a ~500ms jittered backoff on `NetworkError`
    /// (fast-failing DNS/TCP/TLS errors that typically return well under
    /// the per-request budget). `NetworkTimeout` is deliberately NOT
    /// retried: the first attempt already burned the full
    /// `MOJANG_TIMEOUT_SECS` (10s) budget, so a second attempt with the
    /// same budget would risk a ~20s wall-time spike on a single resolve
    /// — bad for chat responsiveness and for the single-flight follower
    /// queue building up behind it. The other variants (`NotFound`,
    /// `InvalidShape`, `UpstreamError`, `MalformedResponse`,
    /// `RateLimited`) are single-shot for the same reasons as before
    /// (not transient, or retry would deepen the throttle).
    ///
    /// Total worst-case wall time: one attempt up to 10s, plus optional
    /// 300-700ms jitter sleep, plus a second 10s attempt only on
    /// `NetworkError` — so the budget is ~10.7s on the no-retry path
    /// (timeout) and ~20.7s on the retried path (connect/DNS failure
    /// that recovers). Documented here so a tuning change cannot make
    /// the contract drift silently.
    #[cfg_attr(test, allow(dead_code))]
    pub async fn get_uuid_async(username: &str) -> Result<String, MojangResolveError> {
        if !is_valid_username_shape(username) {
            warn!("[Mojang] rejecting out-of-shape username '{username}' before URL build");
            return Err(MojangResolveError::InvalidShape);
        }

        match Self::get_uuid_async_once(username).await {
            Ok(uuid) => Ok(uuid),
            Err(e @ MojangResolveError::NetworkError) => {
                // Single jittered retry on transient transport errors.
                // Bound the sleep tight (300-700ms) so the total wall
                // time stays well under the 10s timeout invariant — the
                // first attempt errored quickly (DNS/connect) and the
                // second attempt gets the same 10s budget.
                let mut buf = [0u8; 1];
                let jitter_ms: u64 = match getrandom::fill(&mut buf) {
                    Ok(()) => 300 + (buf[0] as u64 * 400 / 255), // 300..=700
                    Err(_) => 500, // RNG failure: fall back to fixed midpoint.
                };
                debug!(
                    "[Mojang] transient error for '{username}' ({e}); retrying once in {jitter_ms}ms"
                );
                tokio::time::sleep(Duration::from_millis(jitter_ms)).await;
                Self::get_uuid_async_once(username).await
            }
            Err(e) => Err(e),
        }
    }

    /// Single-shot Mojang lookup: one HTTPS round-trip, no retry. Lifted
    /// out of [`User::get_uuid_async`] so the retry envelope above can
    /// invoke it twice for transient-transport failures without
    /// duplicating the request/parse logic.
    #[cfg_attr(test, allow(dead_code))]
    async fn get_uuid_async_once(username: &str) -> Result<String, MojangResolveError> {
        let url = format!("https://api.mojang.com/users/profiles/minecraft/{username}");

        let client = get_http_client();
        let response = client.get(&url).send().await.map_err(|e| {
            if e.is_timeout() {
                warn!("[Mojang] timeout after {MOJANG_TIMEOUT_SECS}s resolving '{username}'");
                MojangResolveError::NetworkTimeout
            } else if e.is_connect() {
                warn!("[Mojang] connect failed resolving '{username}': {e}");
                MojangResolveError::NetworkError
            } else {
                warn!("[Mojang] request failed resolving '{username}': {e}");
                MojangResolveError::NetworkError
            }
        })?;

        if response.status() == reqwest::StatusCode::NO_CONTENT {
            debug!("[Mojang] username '{username}' not found (204)");
            return Err(MojangResolveError::NotFound {
                username: username.to_string(),
            });
        }

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            // Parse Retry-After before consuming `response` (the body is
            // typically empty on 429 and irrelevant either way). Mojang's
            // rate-limit semantics aren't well documented, so accept both
            // formats RFC 7231 permits: integer seconds and HTTP-date.
            let retry_after = response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(parse_retry_after);
            warn!(
                "[Mojang] rate-limited resolving '{username}' (retry_after={:?})",
                retry_after
            );
            return Err(MojangResolveError::RateLimited { retry_after });
        }

        if !response.status().is_success() {
            let status = response.status();
            warn!("[Mojang] non-success resolving '{username}': HTTP {status}");
            return Err(MojangResolveError::UpstreamError);
        }

        // Read the body bytes first (separate from JSON decode) so we can
        // classify body-drop / TLS-record-truncation / read-timeout (all
        // transient transport conditions reported by `reqwest::Error::is_body()`
        // or `is_timeout()`) as retryable `NetworkError` / `NetworkTimeout`
        // rather than the non-retryable `MalformedResponse`. Only a successful
        // body read that fails the subsequent JSON parse is truly "malformed
        // response" — i.e. Mojang spoke 2xx, gave us bytes, and the bytes
        // weren't decodable.
        let body_bytes = response.bytes().await.map_err(|e| {
            if e.is_timeout() {
                warn!(
                    "[Mojang] timeout reading body for '{username}' (after status 2xx): {e}"
                );
                MojangResolveError::NetworkTimeout
            } else if e.is_connect() || e.is_body() {
                warn!(
                    "[Mojang] transport error reading body for '{username}': {e}"
                );
                MojangResolveError::NetworkError
            } else {
                warn!(
                    "[Mojang] body read failed for '{username}': {e}"
                );
                MojangResolveError::NetworkError
            }
        })?;
        let mojang_response: MojangResponse =
            serde_json::from_slice(&body_bytes).map_err(|e| {
                warn!("[Mojang] malformed response for '{username}': {e}");
                MojangResolveError::MalformedResponse
            })?;

        // Mojang returns a bare-hex UUID; canonicalize to the hyphenated
        // lowercase xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx form. The helper
        // owns BOTH the lowercase step AND the shape gate so the slicing
        // there cannot panic on adversarial input. The persistence layer's
        // `is_valid_uuid_shape` is lowercase-only — canonicalizing here
        // means an uppercase Mojang reply does not survive into a filename
        // that `save_in_dir`/`load_all_in_dir` would silently reject.
        let formatted = canonicalize_mojang_id(&mojang_response.id).map_err(|e| {
            warn!(
                "[Mojang] invalid UUID shape for '{username}': got {:?}",
                mojang_response.id
            );
            e
        })?;

        debug!("[Mojang] resolved '{username}' -> {formatted}");
        Ok(formatted)
    }

    /// Build the on-disk path for a user file. Validates `uuid` shape so a
    /// tampered or legacy `User.uuid` (e.g. `"../foo"`) cannot turn the next
    /// `save_dirty` cycle into a write-anywhere primitive — `save_dirty`
    /// builds expected filenames from `user.uuid` verbatim and orphan-deletes
    /// any unmatched `.json` in the users directory, so a malformed value
    /// would both write outside the directory AND wipe legitimate files.
    ///
    /// Production code now goes through `save_in_dir`, which inlines the
    /// same shape gate; this helper is retained for the shape-gate tests
    /// and is therefore `#[cfg(test)]`-gated to keep the production binary
    /// free of dead code.
    #[cfg(test)]
    fn get_user_file_path(uuid: &str) -> io::Result<PathBuf> {
        if !is_valid_uuid_shape(uuid) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Invalid user UUID shape: {uuid:?}"),
            ));
        }
        Ok(PathBuf::from(Self::USERS_DIR).join(format!("{uuid}.json")))
    }

    /// Saves this single `User` to `data/users/{self.uuid}.json`, creating
    /// the directory if needed. Uses `write_atomic` so a crash mid-write
    /// cannot leave a partially written user file.
    ///
    /// Retained as a one-liner over `save_in_dir` for symmetry with the
    /// other `Type::save` methods on the storage types (and as a future
    /// callsite if a single-user write path is needed). Production code
    /// reaches the same logic through
    /// `save_dirty` → `save_dirty_in_dir` → `save_in_dir`, so this wrapper
    /// has no live callers — `#[allow(dead_code)]` keeps it available
    /// without a warning.
    #[allow(dead_code)]
    pub fn save(&self) -> io::Result<()> {
        self.save_in_dir(Path::new(Self::USERS_DIR))
    }

    /// Directory-parameterized form of `save`. Validates the embedded uuid
    /// shape before joining it to `dir` so a tampered `self.uuid` cannot
    /// escape the supplied directory. Tests target this directly with a
    /// `tempfile::tempdir()` to exercise the persistence path without
    /// touching `data/users/`.
    fn save_in_dir(&self, dir: &Path) -> io::Result<()> {
        if !is_valid_uuid_shape(&self.uuid) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("Invalid user UUID shape: {:?}", self.uuid),
            ));
        }
        let path = dir.join(format!("{}.json", self.uuid));

        if !dir.exists() {
            fs::create_dir_all(dir)?;
        }

        let json_str = serde_json::to_string_pretty(self)?;
        write_atomic(&path, &json_str)?;
        debug!("[User] saved {} (balance={}, operator={})", self.uuid, self.balance, self.operator);
        Ok(())
    }

    /// Loads every `{uuid}.json` from the users directory. Malformed or
    /// unreadable files are skipped with a `warn!` log that includes the path.
    pub fn load_all() -> io::Result<HashMap<String, Self>> {
        Self::load_all_in_dir(Path::new(Self::USERS_DIR))
    }

    /// Directory-parameterized form of `load_all`. Same skip-and-quarantine
    /// rules; tests target this directly with a `tempfile::tempdir()` to
    /// pin the malformed-entry guards without touching `data/users/`.
    fn load_all_in_dir(dir_path: &Path) -> io::Result<HashMap<String, Self>> {
        let mut users = HashMap::new();

        if !dir_path.exists() {
            info!("[User] users directory not found at {}, starting empty", dir_path.display());
            return Ok(HashMap::new());
        }

        let mut skipped = 0usize;
        for entry in fs::read_dir(dir_path)? {
            // Per-entry IO errors (transient lock, deleted-during-iter, EACCES
            // on a single file) skip the entry rather than aborting the whole
            // load. The whole-directory `read_dir` failure above remains fatal.
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    skipped += 1;
                    warn!("[User] skipping unreadable directory entry: {e}");
                    continue;
                }
            };
            let path = entry.path();

            if path.is_file() && path.extension().is_some_and(|ext| ext == "json") {
                match fs::read_to_string(&path) {
                    Ok(json_str) => match serde_json::from_str::<Self>(&json_str) {
                        Ok(user) => {
                            // Defense-in-depth at the load boundary: require
                            // the embedded `uuid` matches a hex-UUID shape
                            // AND equals the file stem. Without this, a
                            // tampered file `foo.json` could carry
                            // `"uuid": "../bar"` and on the next save_dirty
                            // cycle (a) write to an attacker-chosen path and
                            // (b) sweep away every legitimate user file as
                            // an "orphan" (their canonical filenames don't
                            // match the malformed `expected_files` set).
                            if !is_valid_uuid_shape(&user.uuid) {
                                skipped += 1;
                                warn!(
                                    "[User] skipping {}: embedded uuid {:?} fails shape check",
                                    path.display(), user.uuid
                                );
                                continue;
                            }
                            let stem = path
                                .file_stem()
                                .and_then(|s| s.to_str())
                                .unwrap_or("");
                            if stem != user.uuid {
                                skipped += 1;
                                warn!(
                                    "[User] skipping {}: filename stem {:?} does not match embedded uuid {:?}",
                                    path.display(), stem, user.uuid
                                );
                                continue;
                            }
                            let uuid = user.uuid.clone();
                            users.insert(uuid, user);
                        }
                        Err(e) => {
                            skipped += 1;
                            warn!("[User] skipping malformed {}: {e}", path.display());
                        }
                    },
                    Err(e) => {
                        skipped += 1;
                        warn!("[User] skipping unreadable {}: {e}", path.display());
                    }
                }
            }
        }
        info!("[User] loaded {} users (skipped {})", users.len(), skipped);
        Ok(users)
    }

    /// Saves a HashMap of `User`s, where each `User` is saved to its own file
    /// in the `data/users/` directory through the shared `save_dirty` →
    /// `save_dirty_in_dir` → `save_in_dir` chain.
    /// This method overwrites existing files and then removes any orphaned files.
    ///
    /// The orphan cleanup pass makes the on-disk directory a faithful mirror
    /// of the in-memory map: users removed from the map are also deleted from
    /// disk, preventing stale state from being resurrected by `load_all`.
    ///
    /// Thin wrapper around `save_dirty` that treats every user as dirty;
    /// used at shutdown and for the audit-repair path where the full state
    /// must be flushed. Hot paths (per-order autosave) should use
    /// `save_dirty` with a tracked dirty-set instead, to avoid O(N) fsyncs
    /// per trade.
    #[allow(dead_code)]
    pub fn save_all(users: &HashMap<String, Self>) -> io::Result<()> {
        let all_keys: HashSet<String> = users.keys().cloned().collect();
        Self::save_dirty(users, &all_keys)
    }

    /// Saves only the `User`s whose UUIDs appear in `dirty`, then runs the
    /// orphan-cleanup pass against `users`' current keys so the on-disk
    /// directory still mirrors the in-memory map.
    ///
    /// Skips persisting users that are in `dirty` but no longer present in
    /// `users` — they'll be removed by the orphan sweep below.
    ///
    /// Refuses to operate on an empty `users` map *only when the on-disk users
    /// directory still has `.json` files that the orphan sweep would wipe* —
    /// mirroring `Pair::save_all` and `Trade::save_all`. Empty-map + empty/
    /// missing dir is a no-op `Ok(())` so fresh-install autosaves are not
    /// blocked before the first user lands. A bug that empties the in-memory
    /// map AFTER users have been persisted still fails loud rather than
    /// silently zapping balances and operator flags.
    ///
    /// On a write failure, still completes population of `expected_files` for
    /// the remaining shape-valid users and runs the orphan sweep before
    /// returning the captured error — this preserves the "directory mirrors
    /// map" invariant and prevents stale on-disk files for users dropped from
    /// the map from accumulating across save attempts.
    pub fn save_dirty(users: &HashMap<String, Self>, dirty: &HashSet<String>) -> io::Result<()> {
        Self::save_dirty_in_dir(users, dirty, Path::new(Self::USERS_DIR))
    }

    /// Directory-parameterized form of `save_dirty`. The empty-map guard
    /// lives here (not just in the public wrapper) so tests can exercise
    /// the wipe-refusal invariant directly against a temp dir; the public
    /// `save_dirty` is a thin one-liner over this helper.
    fn save_dirty_in_dir(
        users: &HashMap<String, Self>,
        dirty: &HashSet<String>,
        dir_path: &Path,
    ) -> io::Result<()> {
        // Refuse an empty map only when there are real `.json` files on disk
        // that the orphan sweep below would actually wipe. A fresh install
        // (no users dir, or an empty/stub users dir) is a legitimate no-op:
        // the setup-phase autosave runs before any user has been seen
        // (operator-only flows like `addnode`/`addpair` set `store.dirty`
        // without populating `store.users`), and erroring here would block
        // the entire dirty-flag chain (`state::save` aggregates sub-save
        // errors first-error-keep-going and surfaces the first to the
        // caller; the autosave loop therefore never clears `self.dirty`,
        // and a shutdown then loses every staged mutation). Once any user
        // file exists on disk, an empty in-memory map is still treated as
        // "refuse to wipe".
        if users.is_empty() {
            let dir_has_user_files = match fs::read_dir(dir_path) {
                Ok(read_dir) => read_dir
                    .filter_map(|entry| entry.ok())
                    .any(|entry| {
                        let path = entry.path();
                        path.is_file() && path.extension().is_some_and(|ext| ext == "json")
                    }),
                Err(e) if e.kind() == io::ErrorKind::NotFound => false,
                Err(e) => return Err(e),
            };
            if dir_has_user_files {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "save_dirty called with an empty users map but on-disk user files exist; refusing to wipe the users directory",
                ));
            }
            return Ok(());
        }

        if !dir_path.exists() {
            fs::create_dir_all(dir_path)?;
        }

        // Filenames are keyed on `user.uuid` (not the HashMap key) so on-disk
        // files always match the canonical identity inside the User struct,
        // even if the two ever diverge. Skip any user whose `uuid` fails the
        // shape check — including their malformed filename in `expected_files`
        // would also exempt that path from the orphan sweep, defeating the
        // sweep's "directory mirrors map" contract.
        let mut expected_files = HashSet::with_capacity(users.len());
        let mut written = 0usize;
        let mut skipped_invalid = 0usize;
        let mut first_save_err: Option<io::Error> = None;

        let mut attempted = 0usize;
        for (key, user) in users.iter() {
            if !is_valid_uuid_shape(&user.uuid) {
                warn!(
                    "[User] skipping save for {:?} (key {:?}): uuid fails shape check",
                    user.uuid, key
                );
                skipped_invalid += 1;
                continue;
            }
            let filename = format!("{}.json", user.uuid);
            expected_files.insert(filename);
            if dirty.contains(key) {
                attempted += 1;
                // Attempt every dirty user even after a previous failure: each
                // `write_atomic` is independent, so one transient hiccup must
                // not silently drop later users' balance/operator updates.
                // Capture only the first error to surface to the caller.
                if let Err(e) = user.save_in_dir(dir_path) {
                    warn!("[User] save failed for {}: {e}", user.uuid);
                    if first_save_err.is_none() {
                        first_save_err = Some(e);
                    }
                } else {
                    written += 1;
                }
            }
        }

        // Wipe-refusal: if every user in the map failed the shape gate,
        // `expected_files` is empty and the orphan sweep below would delete
        // every legitimate `.json`. Match the explicit empty-map guard above:
        // only refuse when there are real `.json` files on disk to wipe; a
        // fresh / empty users dir is a legitimate no-op so the autosave
        // chain isn't broken by an all-shape-invalid in-memory map.
        if expected_files.is_empty() {
            let dir_has_user_files = match fs::read_dir(dir_path) {
                Ok(read_dir) => read_dir
                    .filter_map(|entry| entry.ok())
                    .any(|entry| {
                        let path = entry.path();
                        path.is_file() && path.extension().is_some_and(|ext| ext == "json")
                    }),
                Err(e) if e.kind() == io::ErrorKind::NotFound => false,
                Err(e) => return Err(e),
            };
            if dir_has_user_files {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "save_dirty: no shape-valid users to mirror but on-disk user files exist; refusing to wipe the users directory",
                ));
            }
            return Ok(());
        }

        // Orphan sweep: warn-and-continue on per-entry IO errors so a single
        // locked/transient failure doesn't abort the whole sweep, and so a
        // captured `first_save_err` always wins over a sweep-only error
        // (stale orphans self-heal next cycle; a swallowed save error makes
        // callers think state was persisted when it wasn't).
        let mut removed = 0usize;
        let mut first_sweep_err: Option<io::Error> = None;
        if dir_path.exists() {
            match fs::read_dir(dir_path) {
                Ok(read_dir) => {
                    for entry in read_dir {
                        let entry = match entry {
                            Ok(e) => e,
                            Err(e) => {
                                warn!("[User] orphan sweep: unreadable entry: {e}");
                                if first_sweep_err.is_none() {
                                    first_sweep_err = Some(e);
                                }
                                continue;
                            }
                        };
                        let path = entry.path();
                        if path.is_file() && path.extension().is_some_and(|ext| ext == "json")
                            && let Some(filename) = path.file_name().and_then(|n| n.to_str())
                            && !expected_files.contains(filename)
                        {
                            if let Err(e) = fs::remove_file(&path) {
                                warn!("[User] orphan sweep: remove_file({}) failed: {e}", path.display());
                                if first_sweep_err.is_none() {
                                    first_sweep_err = Some(e);
                                }
                            } else {
                                removed += 1;
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("[User] orphan sweep: read_dir({}) failed: {e}", dir_path.display());
                    first_sweep_err = Some(e);
                }
            }
        }

        info!(
            "[User] save_dirty: wrote {} of {} attempted (failed {}), cleaned {} orphan files, skipped {} with invalid uuid shape ({} users in map)",
            written, attempted, attempted - written, removed, skipped_invalid, users.len()
        );
        match first_save_err.or(first_sweep_err) {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_file_path_uses_uuid_with_json_extension() {
        let p = User::get_user_file_path("550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert!(p.ends_with("550e8400-e29b-41d4-a716-446655440000.json"));
    }

    #[test]
    fn user_file_path_rejects_path_traversal_and_separators() {
        for bad in [
            "../etc/passwd",
            "..\\windows\\system32",
            "/abs/path",
            "550e8400/../escape",
            "UPPER-CASE-IS-NOT-CANONICAL-LOWERCASE",
            "",
            "tooshort",
        ] {
            let err = User::get_user_file_path(bad).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::InvalidInput, "uuid {bad:?}");
        }
    }

    #[test]
    fn user_file_path_accepts_canonical_and_bare_hex_uuids() {
        // Canonical hyphenated, lowercase.
        assert!(User::get_user_file_path("550e8400-e29b-41d4-a716-446655440000").is_ok());
        // Bare 32-char lowercase hex.
        assert!(User::get_user_file_path("550e8400e29b41d4a716446655440000").is_ok());
    }

    #[test]
    fn serde_round_trip_preserves_all_fields() {
        let u = User {
            uuid: "uuid-1".into(),
            username: "alice".into(),
            balance: 42.5,
            operator: true,
        };
        let json = serde_json::to_string(&u).unwrap();
        let back: User = serde_json::from_str(&json).unwrap();
        assert_eq!(back, u);
    }

    #[test]
    fn operator_defaults_to_false_for_pre_flag_files() {
        // Older saves predate the `operator` field. `#[serde(default)]` must
        // keep them loading cleanly.
        let json = r#"{"uuid":"u","username":"a","balance":1.0}"#;
        let u: User = serde_json::from_str(json).unwrap();
        assert!(!u.operator);
    }

    #[test]
    fn save_dirty_refuses_empty_map_to_prevent_accidental_wipe() {
        // Pre-create two `*.json` files; an empty `users` map must NOT
        // trigger the orphan sweep that would wipe them.
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.json");
        let f2 = dir.path().join("11111111-2222-3333-4444-555555555555.json");
        fs::write(&f1, "{}").unwrap();
        fs::write(&f2, "{}").unwrap();

        let err = User::save_dirty_in_dir(&HashMap::new(), &HashSet::new(), dir.path())
            .expect_err("empty map must error");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(f1.exists(), "pre-existing file 1 must survive");
        assert!(f2.exists(), "pre-existing file 2 must survive");
    }

    #[test]
    fn save_dirty_empty_map_with_empty_dir_is_ok() {
        // Setup-phase autosave: operator-only flows (addnode/addpair/...) flip
        // `store.dirty` without populating `store.users`. With no user files
        // on disk yet, the empty-map guard must return Ok(()) so the dirty
        // flag can clear and the autosave chain isn't broken.

        // Case 1: dir exists but is empty.
        let dir = tempfile::tempdir().unwrap();
        User::save_dirty_in_dir(&HashMap::new(), &HashSet::new(), dir.path())
            .expect("empty map + empty dir must be Ok");

        // Case 2: dir is missing entirely (fresh install).
        let parent = tempfile::tempdir().unwrap();
        let missing = parent.path().join("nonexistent-users");
        assert!(!missing.exists());
        User::save_dirty_in_dir(&HashMap::new(), &HashSet::new(), &missing)
            .expect("empty map + missing dir must be Ok");

        // Case 3: dir has only a non-`.json` sibling file — still no wipe risk.
        let dir3 = tempfile::tempdir().unwrap();
        fs::write(dir3.path().join("README.txt"), "ignore me").unwrap();
        User::save_dirty_in_dir(&HashMap::new(), &HashSet::new(), dir3.path())
            .expect("empty map + dir with no .json files must be Ok");
    }

    #[test]
    fn load_all_in_dir_drops_file_with_tampered_embedded_uuid() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.json");
        let json = r#"{"uuid":"../bar","username":"alice","balance":1.0}"#;
        fs::write(&path, json).unwrap();

        let users = User::load_all_in_dir(dir.path()).unwrap();
        assert!(users.is_empty(), "tampered file must be dropped");
        assert!(path.exists(), "load_all does NOT delete; file must remain");
    }

    #[test]
    fn load_all_in_dir_drops_file_with_stem_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.json");
        // Embedded uuid is a different valid canonical UUID than the stem.
        let json = r#"{"uuid":"11111111-2222-3333-4444-555555555555","username":"alice","balance":1.0}"#;
        fs::write(&path, json).unwrap();

        let users = User::load_all_in_dir(dir.path()).unwrap();
        assert!(users.is_empty(), "stem-mismatched file must be dropped");
    }

    #[test]
    fn load_all_in_dir_loads_well_formed_file() {
        let dir = tempfile::tempdir().unwrap();
        let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let path = dir.path().join(format!("{uuid}.json"));
        let user = User {
            uuid: uuid.to_string(),
            username: "alice".to_string(),
            balance: 7.5,
            operator: true,
        };
        fs::write(&path, serde_json::to_string(&user).unwrap()).unwrap();

        let users = User::load_all_in_dir(dir.path()).unwrap();
        assert_eq!(users.len(), 1);
        let loaded = users.get(uuid).expect("keyed by embedded uuid");
        assert_eq!(loaded, &user);
    }

    #[test]
    fn save_dirty_in_dir_skips_user_with_invalid_uuid_shape() {
        // Mount the temp dir under a parent so we can assert no file
        // escaped via the malformed `../etc/passwd` uuid into the parent.
        let parent = tempfile::tempdir().unwrap();
        let dir = parent.path().join("users");
        fs::create_dir_all(&dir).unwrap();

        let valid_uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let valid = User {
            uuid: valid_uuid.to_string(),
            username: "alice".to_string(),
            balance: 1.0,
            operator: false,
        };
        let bogus = User {
            uuid: "../etc/passwd".to_string(),
            username: "mallory".to_string(),
            balance: 0.0,
            operator: false,
        };

        let mut users = HashMap::new();
        users.insert(valid_uuid.to_string(), valid);
        users.insert("bogus".to_string(), bogus);

        let mut dirty = HashSet::new();
        dirty.insert(valid_uuid.to_string());
        dirty.insert("bogus".to_string());

        User::save_dirty_in_dir(&users, &dirty, &dir).unwrap();

        // (i) only the valid user's `.json` exists in `dir`.
        let valid_path = dir.join(format!("{valid_uuid}.json"));
        assert!(valid_path.exists(), "valid user must be persisted");
        let entries: Vec<_> = fs::read_dir(&dir).unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .collect();
        assert_eq!(entries.len(), 1, "only one file expected, got {entries:?}");

        // (ii) no file escaped the directory via the malformed uuid.
        assert!(
            !parent.path().join("etc").exists(),
            "no `etc/` sibling should appear next to dir"
        );
        assert!(
            !parent.path().join("etc/passwd.json").exists(),
            "no escape file outside dir"
        );
    }

    // ---- canonicalize_mojang_id (panic-guard for the Mojang slice) --------

    #[test]
    fn canonicalize_mojang_id_lowercase_32hex_ok_hyphenated() {
        assert_eq!(
            canonicalize_mojang_id("550e8400e29b41d4a716446655440000").unwrap(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[test]
    fn canonicalize_mojang_id_uppercase_canonicalized_to_lowercase() {
        assert_eq!(
            canonicalize_mojang_id("550E8400E29B41D4A716446655440000").unwrap(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[test]
    fn canonicalize_mojang_id_mixed_case_canonicalized() {
        assert_eq!(
            canonicalize_mojang_id("550E8400e29B41d4A716446655440000").unwrap(),
            "550e8400-e29b-41d4-a716-446655440000"
        );
    }

    #[test]
    fn canonicalize_mojang_id_31_char_err() {
        // 31 chars (one short).
        assert_eq!(
            canonicalize_mojang_id("550e8400e29b41d4a71644665544000"),
            Err(MojangResolveError::MalformedResponse)
        );
    }

    #[test]
    fn canonicalize_mojang_id_33_char_err() {
        // 33 chars (one too long).
        assert_eq!(
            canonicalize_mojang_id("550e8400e29b41d4a7164466554400000"),
            Err(MojangResolveError::MalformedResponse)
        );
    }

    #[test]
    fn canonicalize_mojang_id_empty_err() {
        assert_eq!(
            canonicalize_mojang_id(""),
            Err(MojangResolveError::MalformedResponse)
        );
    }

    #[test]
    fn canonicalize_mojang_id_non_hex_char_err() {
        // 'g' is not hex.
        assert_eq!(
            canonicalize_mojang_id("550e8400e29b41d4a71644665544000g"),
            Err(MojangResolveError::MalformedResponse)
        );
    }

    #[test]
    fn canonicalize_mojang_id_embedded_nul_err() {
        let mut s = String::from("550e8400e29b41d4a716446655440000");
        // Replace the last byte with a NUL.
        s.pop();
        s.push('\0');
        assert_eq!(s.len(), 32);
        assert_eq!(
            canonicalize_mojang_id(&s),
            Err(MojangResolveError::MalformedResponse)
        );
    }

    #[test]
    fn canonicalize_mojang_id_multibyte_non_ascii_err_no_panic() {
        // 30 ASCII hex bytes + one 2-byte UTF-8 codepoint = 32 BYTES, 31 chars.
        // This is the panic-surface case: a naive byte-slice into [0..8] etc.
        // could land on a non-char-boundary if the byte gate were missing.
        // The function MUST return Err and MUST NOT panic.
        let ascii_30 = "550e8400e29b41d4a71644665544"; // 28 chars
        let s = format!("{ascii_30}aaé"); // 28 + 2 + 2 = 32 bytes
        assert_eq!(s.len(), 32);
        assert_eq!(
            canonicalize_mojang_id(&s),
            Err(MojangResolveError::MalformedResponse)
        );
    }

    #[test]
    fn canonicalize_mojang_id_already_hyphenated_36_err() {
        // Helper requires bare 32-char form; hyphenated is 36 chars and rejected.
        assert_eq!(
            canonicalize_mojang_id("550e8400-e29b-41d4-a716-446655440000"),
            Err(MojangResolveError::MalformedResponse)
        );
    }

    // ---- is_valid_uuid_shape ----------------------------------------------

    #[test]
    fn is_valid_uuid_shape_accepts_canonical_and_bare_hex() {
        assert!(is_valid_uuid_shape("550e8400-e29b-41d4-a716-446655440000"));
        assert!(is_valid_uuid_shape("550e8400e29b41d4a716446655440000"));
    }

    #[test]
    fn is_valid_uuid_shape_rejects_path_traversal() {
        for bad in [
            "../foo",
            "..\\foo",
            "foo/bar",
            "foo\\bar",
            ".",
            "..",
        ] {
            assert!(!is_valid_uuid_shape(bad), "must reject {bad:?}");
        }
    }

    #[test]
    fn is_valid_uuid_shape_rejects_uppercase_and_mixed_case() {
        // Uppercase canonical hyphenated.
        assert!(!is_valid_uuid_shape("550E8400-E29B-41D4-A716-446655440000"));
        // Uppercase bare hex.
        assert!(!is_valid_uuid_shape("550E8400E29B41D4A716446655440000"));
        // Mixed-case bare hex.
        assert!(!is_valid_uuid_shape("550E8400e29b41d4A716446655440000"));
    }

    #[test]
    fn is_valid_uuid_shape_rejects_wrong_hyphen_positions() {
        // Hyphen one position too early at index 7.
        assert!(!is_valid_uuid_shape("550e840-0e29b-41d4-a716-446655440000"));
        // Missing one hyphen (length 35).
        assert!(!is_valid_uuid_shape("550e8400e29b-41d4-a716-446655440000"));
        // Extra hyphen makes length 37.
        assert!(!is_valid_uuid_shape("550e8400--e29b-41d4-a716-446655440000"));
        // All-hyphen length 36.
        assert!(!is_valid_uuid_shape(&"-".repeat(36)));
    }

    #[test]
    fn is_valid_uuid_shape_rejects_wrong_lengths() {
        for n in [0usize, 31, 33, 35, 37, 200] {
            let s = "a".repeat(n);
            assert!(!is_valid_uuid_shape(&s), "must reject length {n}");
        }
    }

    #[test]
    fn is_valid_uuid_shape_rejects_embedded_nul() {
        let mut s = String::from("550e8400e29b41d4a716446655440000");
        s.pop();
        s.push('\0');
        assert_eq!(s.len(), 32);
        assert!(!is_valid_uuid_shape(&s));
    }

    #[test]
    fn is_valid_uuid_shape_rejects_multibyte_non_ascii() {
        // 32-byte string with multi-byte non-ASCII char — must reject without panic.
        let ascii_28 = "550e8400e29b41d4a71644665544";
        let s = format!("{ascii_28}aaé");
        assert_eq!(s.len(), 32);
        assert!(!is_valid_uuid_shape(&s));
    }

    // ---- is_valid_username_shape ------------------------------------------

    #[test]
    fn is_valid_username_shape_accepts_at_boundaries() {
        // 3-char minimum.
        assert!(is_valid_username_shape("abc"));
        // 16-char maximum.
        assert!(is_valid_username_shape("abcdefghijklmnop"));
        // Mixed alphanumeric with underscore.
        assert!(is_valid_username_shape("Steve_99"));
        assert!(is_valid_username_shape("_user_1"));
    }

    #[test]
    fn is_valid_username_shape_rejects_at_boundaries() {
        // 2-char (one short).
        assert!(!is_valid_username_shape("ab"));
        // 17-char (one over).
        assert!(!is_valid_username_shape("abcdefghijklmnopq"));
        // Empty.
        assert!(!is_valid_username_shape(""));
    }

    #[test]
    fn is_valid_username_shape_rejects_multibyte_byte_vs_char() {
        // 4 bytes / 2 chars (2-byte codepoint x 2). Length-in-bytes is in
        // (3..=16) but the byte-class check rejects each non-ASCII byte.
        let s = "éé";
        assert_eq!(s.len(), 4);
        assert!(!is_valid_username_shape(s));
    }

    #[test]
    fn is_valid_username_shape_rejects_disallowed_bytes() {
        for bad in [
            "ab-cd",
            "ab cd",
            "ab.cd",
            "ab:cd",
            "ab\0cd",
        ] {
            assert!(!is_valid_username_shape(bad), "must reject {bad:?}");
        }
    }

    // ---- save_dirty_in_dir wipe-refusal & orphan sweep --------------------

    #[test]
    fn save_dirty_in_dir_refuses_to_wipe_when_all_in_memory_uuids_invalid() {
        // Pre-write 2 valid `.json` files.
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.json");
        let f2 = dir.path().join("11111111-2222-3333-4444-555555555555.json");
        fs::write(&f1, "{}").unwrap();
        fs::write(&f2, "{}").unwrap();

        // All-shape-invalid in-memory map.
        let mut users = HashMap::new();
        users.insert(
            "k1".to_string(),
            User {
                uuid: "../etc/passwd".to_string(),
                username: "a".to_string(),
                balance: 0.0,
                operator: false,
            },
        );
        users.insert(
            "k2".to_string(),
            User {
                uuid: "".to_string(),
                username: "b".to_string(),
                balance: 0.0,
                operator: false,
            },
        );
        users.insert(
            "k3".to_string(),
            User {
                uuid: "UPPERCASE-IS-INVALID".to_string(),
                username: "c".to_string(),
                balance: 0.0,
                operator: false,
            },
        );

        let dirty: HashSet<String> = users.keys().cloned().collect();
        let err = User::save_dirty_in_dir(&users, &dirty, dir.path())
            .expect_err("all-invalid map with on-disk files must error");
        assert_eq!(err.kind(), io::ErrorKind::InvalidInput);
        assert!(f1.exists(), "pre-existing file 1 must survive wipe-refusal");
        assert!(f2.exists(), "pre-existing file 2 must survive wipe-refusal");
    }

    #[test]
    fn save_dirty_in_dir_all_invalid_uuids_with_empty_dir_is_ok() {
        // Symmetric positive case for the lines 558-576 fresh-install carve-out:
        // same all-invalid map but EMPTY users dir → returns Ok(()).
        let dir = tempfile::tempdir().unwrap();

        let mut users = HashMap::new();
        users.insert(
            "k1".to_string(),
            User {
                uuid: "../etc/passwd".to_string(),
                username: "a".to_string(),
                balance: 0.0,
                operator: false,
            },
        );
        users.insert(
            "k2".to_string(),
            User {
                uuid: "".to_string(),
                username: "b".to_string(),
                balance: 0.0,
                operator: false,
            },
        );
        users.insert(
            "k3".to_string(),
            User {
                uuid: "UPPERCASE-IS-INVALID".to_string(),
                username: "c".to_string(),
                balance: 0.0,
                operator: false,
            },
        );

        let dirty: HashSet<String> = users.keys().cloned().collect();
        User::save_dirty_in_dir(&users, &dirty, dir.path())
            .expect("all-invalid map + empty dir is a fresh-install no-op");
    }

    #[test]
    fn save_dirty_in_dir_orphan_sweep_removes_unmapped_json_files() {
        // Pre-write 2 valid-named `.json` files.
        let dir = tempfile::tempdir().unwrap();
        let uuid1 = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let uuid2 = "11111111-2222-3333-4444-555555555555";
        let f1 = dir.path().join(format!("{uuid1}.json"));
        let f2 = dir.path().join(format!("{uuid2}.json"));
        fs::write(&f1, "{}").unwrap();
        fs::write(&f2, "{}").unwrap();

        // HashMap contains only the FIRST as a valid User.
        let mut users = HashMap::new();
        users.insert(
            uuid1.to_string(),
            User {
                uuid: uuid1.to_string(),
                username: "alice".to_string(),
                balance: 1.0,
                operator: false,
            },
        );

        let mut dirty = HashSet::new();
        dirty.insert(uuid1.to_string());

        User::save_dirty_in_dir(&users, &dirty, dir.path()).expect("save_dirty must succeed");

        assert!(f1.exists(), "mapped user file must remain");
        assert!(!f2.exists(), "unmapped user file must be swept");
    }
}
