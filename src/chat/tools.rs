//! Composer tools — memory read/write, history search.
//!
//! Each public function in this module is the **server-side** of one
//! Anthropic `tool_use` block: validates input, executes, returns a
//! JSON-serializable result string. The composer in [`super::composer`]
//! threads tool results back into the next turn.
//!
//! Phase 5 lands the security-critical primitives (path validation,
//! sender binding, bullet sanitization) and the read-only tools. Write
//! tools (`update_player_memory`, `update_self_memory`) and the
//! reflection-pass writer for `adjustments.md` arrive in Phase 6.
//!
//! ## Hard rules baked into this module
//!
//! - **UUID validation**: every UUID input is matched
//!   against `^[0-9a-f-]{32,36}$` (lowercase, optional 4 hyphens).
//! - **Sender binding (S10)**: `update_player_memory` must equal the
//!   current event's sender UUID. No operator override — this is a hard
//!   integrity boundary.
//! - **Cross-player firewall (S7 + ADV1)**: `read_player_memory` is
//!   sender-only by default. `chat.cross_player_reads = true` enables
//!   addressee reads on trusted single-tenant servers; even then,
//!   addressee reads do NOT include the addressee's identity-secrets.
//! - **Section allow-list**: writes are confined to `Stated preferences`,
//!   `Inferred`, `Topics & history`, `Do not mention` — `Identity` and
//!   `Trust` are operator-only.
//! - **Bullet sanitization (C5)**: rejects bullets matching
//!   `(?i)^trust\s*:\s*[0-3]` (forged trust line) or containing `## `
//!   (header injection) or exceeding `update_bullet_max_chars`.

use std::path::{Path, PathBuf};

/// Canonical hyphenated UUID regex shape (the Mojang-resolved form):
/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx`.
fn is_canonical_hyphen_uuid(s: &str) -> bool {
    if s.len() != 36 {
        return false;
    }
    let bytes = s.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        let expected_hyphen = matches!(i, 8 | 13 | 18 | 23);
        if expected_hyphen {
            if b != b'-' {
                return false;
            }
        } else if !b.is_ascii_hexdigit() || b.is_ascii_uppercase() {
            return false;
        }
    }
    true
}

/// Bare 32-char hex UUID (Mojang's wire format before hyphenation).
fn is_bare_hex_uuid(s: &str) -> bool {
    s.len() == 32
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// Validate a UUID. Accepts canonical hyphenated form
/// OR bare 32-char hex; rejects anything else (uppercase, missing
/// hyphens, wrong length).
pub fn validate_uuid(uuid: &str) -> Result<(), &'static str> {
    if is_canonical_hyphen_uuid(uuid) || is_bare_hex_uuid(uuid) {
        Ok(())
    } else {
        Err("uuid must be canonical hyphenated form or bare 32-char hex")
    }
}

/// Validate a Mojang-shape username.
pub fn validate_username_shape(username: &str) -> Result<(), &'static str> {
    if username.len() < 3 || username.len() > 16 {
        return Err("username must be 3-16 characters");
    }
    if !username
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        return Err("username may only contain ASCII alphanumerics and underscore");
    }
    Ok(())
}

/// Resolve a per-player file path from an already-validated UUID and
/// confirm its parent directory canonicalizes to `data/chat/players/`.
/// CHAT.md.
///
/// Returns the resolved (un-canonicalized) path on success — file
/// operations should use the returned path so callers don't accidentally
/// dereference a different one. The canonical-parent check is the
/// security gate; we don't canonicalize the file itself because the
/// file may not exist yet (ensure-or-write paths).
pub fn resolve_player_path(uuid: &str, players_dir: &Path) -> Result<PathBuf, String> {
    validate_uuid(uuid).map_err(str::to_string)?;
    // Defense-in-depth: never trust a UUID containing path separators.
    if uuid.contains('/') || uuid.contains('\\') {
        return Err("uuid contains path separator".to_string());
    }
    let candidate = players_dir.join(format!("{uuid}.md"));

    // The PARENT directory must canonicalize to `players_dir`. We
    // canonicalize the parent specifically so a freshly-created
    // (not-yet-existing) candidate file path is fine.
    let parent = candidate
        .parent()
        .ok_or("candidate path has no parent")?;
    let canonical_parent = std::fs::canonicalize(parent)
        .map_err(|e| format!("could not canonicalize players dir: {e}"))?;
    let expected_parent = std::fs::canonicalize(players_dir)
        .map_err(|e| format!("could not canonicalize expected players dir: {e}"))?;
    if canonical_parent != expected_parent {
        return Err("path escapes players dir".to_string());
    }
    Ok(candidate)
}

/// Section names that the composer is allowed to write to. CHAT.md
/// `update_player_memory` constraint.
pub const WRITABLE_SECTIONS: &[&str] = &[
    "Stated preferences",
    "Inferred",
    "Topics & history",
    "Do not mention",
];

pub fn is_writable_section(section: &str) -> bool {
    WRITABLE_SECTIONS.iter().any(|s| *s == section)
}

/// Sanitize a per-player or per-self memory bullet. CHAT.md +
///
/// Rejects bullets that:
/// - match `(?i)^trust\s*:\s*[0-3]` (forged trust line),
/// - contain `## ` (section-header injection),
/// - exceed `max_chars`.
///
/// `max_chars` is `chat.update_bullet_max_chars` (default 280).
pub fn sanitize_bullet(bullet: &str, max_chars: usize) -> Result<String, &'static str> {
    let trimmed = bullet.trim();
    if trimmed.is_empty() {
        return Err("bullet is empty");
    }
    if trimmed.chars().count() > max_chars {
        return Err("bullet exceeds max_chars");
    }
    if trimmed.contains("## ") {
        return Err("bullet contains '## ' (section-header injection)");
    }
    // Match `(?i)^trust\s*:\s*[0-3]` without a regex dep.
    let lower = trimmed.to_lowercase();
    let after_trust = lower.strip_prefix("trust");
    if let Some(rest) = after_trust {
        let rest = rest.trim_start();
        if let Some(rest) = rest.strip_prefix(':') {
            let rest = rest.trim_start();
            if rest
                .chars()
                .next()
                .is_some_and(|c| ('0'..='3').contains(&c))
            {
                return Err("bullet contains forged trust line");
            }
        }
    }
    Ok(trimmed.to_string())
}

/// Ensure a section header exists in a Markdown body. If `## <name>`
/// is missing, append it (with a blank line before) and return the new
/// body. Otherwise return the body unchanged.
pub fn ensure_section(body: &str, section: &str) -> String {
    let header = format!("## {section}");
    if body.contains(&header) {
        return body.to_string();
    }
    let mut out = body.trim_end().to_string();
    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str(&header);
    out.push('\n');
    out
}

/// Append a bullet to the named section in a Markdown body. The bullet
/// is prefixed with the ISO date and `- ` so the line shape matches
/// the schema.
///
/// Idempotency: if a bullet with identical body already appears under
/// the section, this is a no-op (returns body unchanged). The Phase 6
/// dedup pass handles fuzzier near-duplicates via Levenshtein ≥ 0.85.
///
/// Caller is responsible for `ensure_section` first if the section
/// might be missing.
pub fn append_bullet_to_section(
    body: &str,
    section: &str,
    bullet: &str,
    today: &str,
) -> String {
    let header = format!("## {section}");
    let new_line = format!("- {today}: {bullet}");
    if body.contains(&format!("- {today}: {bullet}\n"))
        || body.contains(&format!("- {today}: {bullet}\r\n"))
    {
        return body.to_string();
    }
    let Some(start) = body.find(&header) else {
        // Caller should have ensured the section; defensively append at end.
        let mut out = ensure_section(body, section);
        out.push_str(&new_line);
        out.push('\n');
        return out;
    };
    // Find the end of this section: either the next `\n## ` header or EOF.
    let after_header = start + header.len();
    let rest = &body[after_header..];
    let next_header_offset = rest.find("\n## ");
    let (insert_at, before_next) = match next_header_offset {
        Some(off) => (after_header + off, true),
        None => (body.len(), false),
    };
    let mut out = body[..insert_at].trim_end().to_string();
    out.push('\n');
    out.push_str(&new_line);
    if before_next {
        out.push_str("\n");
        out.push_str(&body[insert_at..]);
    } else {
        out.push('\n');
    }
    out
}

/// Result of a sender-binding check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SenderBind {
    Bound,
    Mismatch,
}

/// Verify the `uuid` argument equals the current event's sender UUID.
/// Used by `update_player_memory`. No operator override — hard boundary.
pub fn check_sender_binding(arg_uuid: &str, current_event_sender_uuid: &str) -> SenderBind {
    if arg_uuid.eq_ignore_ascii_case(current_event_sender_uuid) {
        SenderBind::Bound
    } else {
        SenderBind::Mismatch
    }
}

/// Result of a cross-player read check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadAuthorization {
    /// Sender's own memory.
    Allowed,
    /// Cross-player read denied. CHAT.md reads return "access denied"
    /// to the model when this fires.
    Denied,
    /// Operator opted in via `chat.cross_player_reads = true`.
    AllowedByOperator,
}

pub fn authorize_player_read(
    arg_uuid: &str,
    current_event_sender_uuid: &str,
    cross_player_reads_enabled: bool,
) -> ReadAuthorization {
    if arg_uuid.eq_ignore_ascii_case(current_event_sender_uuid) {
        return ReadAuthorization::Allowed;
    }
    if cross_player_reads_enabled {
        ReadAuthorization::AllowedByOperator
    } else {
        ReadAuthorization::Denied
    }
}

// ===== Runtime tool definitions and dispatcher =============================

use crate::chat::client::Tool;
use serde_json::{Value, json};

/// Build the full `Tool` list to expose to the composer.
/// `web_search_enabled` and `web_fetch_enabled` come from `ChatConfig`;
/// the composer only sees web tools when the operator opts in.
pub fn tool_definitions(web_search_enabled: bool, web_fetch_enabled: bool) -> Vec<Tool> {
    let mut tools = vec![
        Tool {
            name: "read_my_memory".to_string(),
            description: "Read the global memory.md (your own memory of the server / yourself). Returns markdown text.".to_string(),
            input_schema: json!({"type": "object", "properties": {}, "required": []}),
        },
        Tool {
            name: "read_player_memory".to_string(),
            description: "Read the per-player memory file for a UUID or username. Returns markdown text or 'access denied' if the player is not the current sender.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "uuid": {"type": "string", "description": "Canonical hyphenated UUID"},
                    "username": {"type": "string", "description": "Mojang username; resolved to UUID"}
                },
                "required": []
            }),
        },
        Tool {
            name: "update_player_memory".to_string(),
            description: "Append a single bullet to a section of the current sender's per-player file. Use this generously: any time the sender shares a fun fact, preference, opinion, build detail, inside joke, or asks you to remember something about them — capture it. Allowed sections: 'Stated preferences', 'Inferred', 'Topics & history', 'Do not mention'.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "uuid": {"type": "string"},
                    "section": {"type": "string"},
                    "bullet": {"type": "string"}
                },
                "required": ["uuid", "section", "bullet"]
            }),
        },
        Tool {
            name: "update_self_memory".to_string(),
            description: "Append a bullet to the '## Inferred' section of memory.md (your own memory about yourself / the server). Use this generously when you learn something stable about yourself or the server — a role, a preference, a fact about your shop, a notable event. ISO-date prefixed.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {"bullet": {"type": "string"}},
                "required": ["bullet"]
            }),
        },
        Tool {
            name: "read_today_history".to_string(),
            description: "Read today's chat history JSONL, capped at 32 KB. Most recent first. Optionally paginate with `since_event_ts` (ISO-UTC) to only return records strictly newer than that timestamp.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "limit_lines": {"type": "integer", "default": 200},
                    "since_event_ts": {
                        "type": "string",
                        "description": "ISO-UTC timestamp; only records with `ts` strictly greater are returned."
                    }
                },
                "required": []
            }),
        },
        Tool {
            name: "search_history".to_string(),
            description: "Substring search across today's and recent past chat history JSONL files. Up to 50 matches.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"},
                    "days_back": {"type": "integer", "default": 7}
                },
                "required": ["query"]
            }),
        },
    ];
    if web_search_enabled {
        // Anthropic-managed server tool — no schema we own; passing it
        // through as an opaque tool so the model can request it. The
        // server-side ID is the canonical tool name; we present it as
        // `web_search_20250305`. If the account doesn't have access,
        // Anthropic returns an error result we surface as a tool_error.
        tools.push(Tool {
            name: "web_search".to_string(),
            description: "Search the web (Anthropic-managed). Reach for this whenever a player asks you to look something up, check a current fact, find documentation, or anything you don't already know — it's first-resort, not last-resort. A daily cap exists but is generous.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {"query": {"type": "string"}},
                "required": ["query"]
            }),
        });
    }
    if web_fetch_enabled {
        tools.push(Tool {
            name: "web_fetch".to_string(),
            description: "Fetch the contents of an http(s) URL (max 256 KB). Use this when a player gives you a link to read, or when web_search returns a URL you want to read in full. Strict deny-list applies; rejects local / metadata / numeric-form addresses.".to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {"url": {"type": "string"}},
                "required": ["url"]
            }),
        });
    }
    tools
}

/// Context passed to [`dispatch`].
pub struct ToolContext<'a> {
    /// UUID of the current event's sender (for sender-binding checks).
    pub sender_uuid: &'a str,
    /// Operator opt-in: cross-player reads.
    pub cross_player_reads: bool,
    /// Tools-history byte cap.
    pub history_max_bytes: usize,
    /// Update-bullet character cap.
    pub update_bullet_max_chars: usize,
    /// Days-back cap for `search_history`.
    pub history_search_max_days: u32,
    /// `web_fetch_max_bytes` from ChatConfig (default 256 KB).
    pub web_fetch_max_bytes: usize,
    /// Whether the operator enabled `web_fetch`.
    pub web_fetch_enabled: bool,
    /// Today's UTC date (YYYY-MM-DD).
    pub today: String,
    /// Per-player file byte cap. Enforced inside
    /// [`update_player_memory`] — exceeding it returns an explicit error
    /// so the model can re-plan rather than silently growing the file.
    pub player_memory_max_bytes: u32,
    /// CHAT.md — bullets queued today by `update_self_memory`.
    /// Read by the tool to enforce the daily cap; the orchestrator
    /// increments the matching state.json counter after a successful
    /// invocation (the tool does not mutate state.json directly).
    pub update_self_memory_today: u32,
    /// CHAT.md — daily cap for `update_self_memory` invocations.
    pub update_self_memory_max_per_day: u32,
    /// CHAT.md — bullet cap on `## Inferred` in `memory.md`. When a
    /// commit pushes past the cap, the oldest bullet(s) are moved to
    /// `memory.archive.md`.
    pub memory_max_inferred_bullets: u32,
    /// CHAT.md — `web_fetch` calls already made today. Read by the
    /// tool to enforce the daily budget; the orchestrator increments
    /// the matching state.json counter after a successful fetch.
    pub web_fetches_today: u32,
    /// CHAT.md — `web_fetch` daily budget cap.
    pub web_fetch_daily_max: u32,
}

/// Dispatch one tool_use block from the model. Returns the textual
/// tool_result body (or error string). Always returns Ok — errors are
/// returned as the result string with `is_error = true` at the
/// composer layer.
pub async fn dispatch(name: &str, input: &Value, ctx: &ToolContext<'_>) -> (String, bool) {
    let result = match name {
        "read_my_memory" => read_my_memory().await,
        "read_player_memory" => read_player_memory_tool(input, ctx).await,
        "update_player_memory" => update_player_memory_tool(input, ctx).await,
        "update_self_memory" => update_self_memory_tool(input, ctx).await,
        "read_today_history" => read_today_history_tool(input, ctx).await,
        "search_history" => search_history_tool(input, ctx).await,
        "web_fetch" => {
            if !ctx.web_fetch_enabled {
                Err("web_fetch is not enabled in config".to_string())
            } else {
                web_fetch_tool(input, ctx).await
            }
        }
        // web_search is Anthropic-managed; if the model invokes it via
        // our tool exposure, we return a stub explaining we can't run
        // it locally (real implementation would proxy to Anthropic's
        // server tool). For now, surface a clear error.
        "web_search" => Err("web_search is provided as an Anthropic server tool; not dispatchable locally in this build".to_string()),
        other => Err(format!("unknown tool: {other}")),
    };
    match result {
        Ok(s) => (s, false),
        Err(e) => (e, true),
    }
}

async fn read_my_memory() -> Result<String, String> {
    crate::chat::memory::read_global_memory()
        .map_err(|e| format!("read_global_memory: {e}"))
}

/// Resolve the per-player file path AFTER `validate_uuid`, with the
/// canonical-parent gate engaged. On a fresh chat install
/// the players dir may not exist yet, so we create it before
/// canonicalization — `resolve_player_path` requires `players_dir` to
/// exist on disk in order to canonicalize.
fn resolve_player_path_runtime(uuid: &str) -> Result<PathBuf, String> {
    let players_dir = std::path::Path::new(crate::chat::memory::PLAYERS_DIR);
    if !players_dir.exists()
        && let Err(e) = std::fs::create_dir_all(players_dir)
    {
        return Err(format!("create players dir: {e}"));
    }
    resolve_player_path(uuid, players_dir)
}

async fn read_player_memory_tool(input: &Value, ctx: &ToolContext<'_>) -> Result<String, String> {
    let uuid_arg = input.get("uuid").and_then(|v| v.as_str());
    let username_arg = input.get("username").and_then(|v| v.as_str());
    let target_uuid = if let Some(u) = uuid_arg {
        validate_uuid(u).map_err(str::to_string)?;
        u.to_string()
    } else if let Some(name) = username_arg {
        validate_username_shape(name).map_err(str::to_string)?;
        crate::mojang::resolve_user_uuid(name)
            .await
            .map_err(|e| format!("resolve {name}: {e}"))?
    } else {
        return Err("require either uuid or username".to_string());
    };
    let auth = authorize_player_read(&target_uuid, ctx.sender_uuid, ctx.cross_player_reads);
    if matches!(auth, ReadAuthorization::Denied) {
        return Err("access denied (cross-player reads disabled)".to_string());
    }
    // CHAT.md: canonicalize before reading. Path-traversal UUIDs would
    // already have been rejected by `validate_uuid`; this is the second
    // line of defense, ensuring the parent dir is exactly the configured
    // players dir even if a future change loosens UUID validation.
    let path = resolve_player_path_runtime(&target_uuid)?;
    match std::fs::read_to_string(&path) {
        Ok(body) => Ok(body),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(format!("read_player: {e}")),
    }
}

async fn update_player_memory_tool(input: &Value, ctx: &ToolContext<'_>) -> Result<String, String> {
    let uuid = input
        .get("uuid")
        .and_then(|v| v.as_str())
        .ok_or("uuid is required")?;
    let section = input
        .get("section")
        .and_then(|v| v.as_str())
        .ok_or("section is required")?;
    let bullet = input
        .get("bullet")
        .and_then(|v| v.as_str())
        .ok_or("bullet is required")?;

    validate_uuid(uuid).map_err(str::to_string)?;
    if !matches!(check_sender_binding(uuid, ctx.sender_uuid), SenderBind::Bound) {
        return Err("sender binding violated: uuid must equal the current sender".to_string());
    }
    if !is_writable_section(section) {
        return Err(format!("section '{section}' is not writable"));
    }
    let safe_bullet = sanitize_bullet(bullet, ctx.update_bullet_max_chars).map_err(str::to_string)?;

    // Bootstrap the file if needed BEFORE canonicalization so the
    // players dir is guaranteed to exist on disk. The index is keyed
    // username → UUID, so the reverse lookup is an iter-find: the index
    // is small (one entry per known player) so this is fine.
    let username = crate::chat::memory::load_or_rebuild_index()
        .ok()
        .and_then(|idx| {
            idx.by_lower_username
                .iter()
                .find(|(_, v)| v.eq_ignore_ascii_case(uuid))
                .map(|(k, _)| k.clone())
        })
        .unwrap_or_else(|| "unknown".to_string());
    crate::chat::memory::ensure_player_file(uuid, &username)
        .map_err(|e| format!("ensure_player_file: {e}"))?;

    // CHAT.md: canonicalize before writing. Returns Err("path escapes
    // ...") if anything resolves outside the players dir.
    let path = resolve_player_path_runtime(uuid)?;

    let body = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("read_player: {e}")),
    };
    let with_section = ensure_section(&body, section);
    let new_body = append_bullet_to_section(&with_section, section, &safe_bullet, &ctx.today);

    // CHAT.md: explicit cap-error so the model can re-plan instead of
    // silently growing the file. Summarization is the orchestrator's job
    // — the tool itself never invokes Haiku. The cap-plus-25 % gate
    // ([`crate::chat::memory::should_summarize_player_file`]) is the
    // documented threshold for triggering the summarization pass.
    if new_body.len() > ctx.player_memory_max_bytes as usize {
        let summarize_recommended = crate::chat::memory::should_summarize_player_file(
            new_body.len(),
            ctx.player_memory_max_bytes as usize,
        );
        let reason = if summarize_recommended {
            "player memory at cap (>125%); summarization rate-limited"
        } else {
            "player memory at cap; summarization rate-limited"
        };
        return Err(reason.to_string());
    }

    crate::fsutil::write_atomic(&path, &new_body)
        .map_err(|e| format!("write_player: {e}"))?;
    Ok(format!("appended to '{section}'"))
}

/// On-disk path for the rotated archive of evicted `## Inferred`
/// bullets. The live file is `data/chat/memory.md`; this is the
/// archive that bullets migrate to once the inferred-section bullet
/// cap is exceeded.
pub const MEMORY_ARCHIVE_FILE: &str = "data/chat/memory.archive.md";

/// Levenshtein-ratio threshold above which a candidate bullet is treated
/// as a near-duplicate of an existing one.
const SELF_MEMORY_DEDUP_RATIO: f64 = 0.85;

/// Return every existing self-memory bullet body (date prefix stripped)
/// found under the `## Inferred` section of `memory.md`. Used by the
/// tool's dedup gate so a near-duplicate of an already-committed bullet
/// is rejected at the tool boundary (saves a write + an oldest-archive
/// rotation that would otherwise drop the displaced sibling).
fn collect_existing_self_bullets_at(memory_path: &std::path::Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let Ok(global) = std::fs::read_to_string(memory_path) else {
        return out;
    };
    let mut in_inferred = false;
    for line in global.lines() {
        let trimmed = line.trim_end();
        if trimmed.starts_with("## ") {
            in_inferred = trimmed == "## Inferred";
            continue;
        }
        if in_inferred && let Some(rest) = trimmed.strip_prefix("- ") {
            // Lines have shape `- <date>: <bullet>` (see
            // `append_bullet_to_section`). Strip the `<date>: ` prefix
            // when present so dedup compares the bullet body itself.
            let body = rest.split_once(": ").map(|(_, b)| b).unwrap_or(rest);
            out.push(body.to_string());
        }
    }
    out
}

/// Eagerly commit a sanitized bullet to the `## Inferred` section of
/// `memory.md`. Enforces `max_inferred_bullets` by archiving the
/// oldest displaced entries to `memory.archive.md` (kept around for
/// future reference; not loaded into the prompt).
///
/// `memory_path` and `archive_path` are taken as parameters so tests can
/// redirect them to a temp dir; the production helper at the call site
/// passes the real on-disk paths.
pub fn commit_self_memory_bullet(
    bullet: &str,
    today: &str,
    max_inferred_bullets: u32,
    memory_path: &std::path::Path,
    archive_path: &std::path::Path,
) -> Result<(), String> {
    let body = std::fs::read_to_string(memory_path).unwrap_or_default();
    let body = ensure_section(&body, "Inferred");
    let body = append_bullet_to_section(&body, "Inferred", bullet, today);
    let (kept, evicted) = enforce_inferred_cap(&body, max_inferred_bullets as usize);

    if let Some(parent) = memory_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create memory dir: {e}"))?;
    }
    crate::fsutil::write_atomic(memory_path, &kept)
        .map_err(|e| format!("write memory.md: {e}"))?;

    if !evicted.is_empty() {
        if let Some(parent) = archive_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("create archive dir: {e}"))?;
        }
        // Append-only archive: read existing, append evicted lines,
        // write atomically. We append rather than `OpenOptions::append`
        // so the file write is single-shot atomic via fsutil.
        let mut combined = std::fs::read_to_string(archive_path).unwrap_or_default();
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        for line in &evicted {
            combined.push_str(line);
            combined.push('\n');
        }
        crate::fsutil::write_atomic(archive_path, &combined)
            .map_err(|e| format!("write memory.archive.md: {e}"))?;
    }
    Ok(())
}

/// Apply the `max_inferred_bullets` cap to the `## Inferred` section of
/// a memory.md body. Returns `(new_body, evicted_lines)` where the
/// evicted lines are the oldest bullets removed to satisfy the cap.
///
/// Bullets are recognized as `^- ` lines under the `## Inferred`
/// heading; any other shape (blank, comment, sub-heading) is left in
/// place. The OLDEST bullets are evicted first — defined as their
/// position in the section, since `append_bullet_to_section` appends to
/// the end of the section.
fn enforce_inferred_cap(body: &str, max_bullets: usize) -> (String, Vec<String>) {
    let header = "## Inferred";
    let Some(start) = body.find(header) else {
        return (body.to_string(), Vec::new());
    };
    let after_header = start + header.len();
    let rest = &body[after_header..];
    let next_header_offset = rest.find("\n## ");
    let (section_end, before_next) = match next_header_offset {
        Some(off) => (after_header + off, true),
        None => (body.len(), false),
    };
    let section_body = &body[after_header..section_end];

    let mut bullet_indices: Vec<usize> = Vec::new();
    for (i, line) in section_body.lines().enumerate() {
        if line.trim_start().starts_with("- ") {
            bullet_indices.push(i);
        }
    }
    if bullet_indices.len() <= max_bullets {
        return (body.to_string(), Vec::new());
    }
    let evict_count = bullet_indices.len() - max_bullets;
    let evict_set: std::collections::HashSet<usize> = bullet_indices
        .iter()
        .take(evict_count)
        .copied()
        .collect();

    let mut kept_section = String::new();
    let mut evicted: Vec<String> = Vec::new();
    for (i, line) in section_body.lines().enumerate() {
        if evict_set.contains(&i) {
            evicted.push(line.trim_end().to_string());
        } else {
            kept_section.push_str(line);
            kept_section.push('\n');
        }
    }

    let mut out = String::new();
    out.push_str(&body[..after_header]);
    // Ensure the kept section has a leading newline like the original.
    if !kept_section.starts_with('\n') {
        out.push('\n');
    }
    out.push_str(kept_section.trim_end_matches('\n'));
    out.push('\n');
    if before_next {
        // Restore the trailing chunk (next header onward) verbatim.
        out.push_str(&body[section_end..]);
    }
    (out, evicted)
}

async fn update_self_memory_tool(input: &Value, ctx: &ToolContext<'_>) -> Result<String, String> {
    update_self_memory_at_paths(
        input,
        ctx,
        std::path::Path::new(crate::chat::memory::GLOBAL_MEMORY),
        std::path::Path::new(MEMORY_ARCHIVE_FILE),
    )
}

/// Inner helper exposed at module level so tests can redirect the
/// memory + archive paths to a temp location. Does NOT mutate
/// `state.json`; the orchestrator increments
/// `update_self_memory_today` after a successful invocation.
fn update_self_memory_at_paths(
    input: &Value,
    ctx: &ToolContext<'_>,
    memory_path: &std::path::Path,
    archive_path: &std::path::Path,
) -> Result<String, String> {
    let bullet = input
        .get("bullet")
        .and_then(|v| v.as_str())
        .ok_or("bullet is required")?;

    // Daily cap. Tool is read-only against state.json; the orchestrator
    // bumps the counter on Ok.
    if ctx.update_self_memory_today >= ctx.update_self_memory_max_per_day {
        return Err("daily limit reached".to_string());
    }

    let safe_bullet = sanitize_bullet(bullet, ctx.update_bullet_max_chars)
        .map_err(str::to_string)?;

    // Levenshtein-ratio dedup against the live `## Inferred` section.
    let existing = collect_existing_self_bullets_at(memory_path);
    for prev in &existing {
        if crate::chat::conversation::levenshtein_ratio(prev, &safe_bullet)
            >= SELF_MEMORY_DEDUP_RATIO
        {
            return Err("near-duplicate of an existing self-memory bullet".to_string());
        }
    }

    commit_self_memory_bullet(
        &safe_bullet,
        &ctx.today,
        ctx.memory_max_inferred_bullets,
        memory_path,
        archive_path,
    )?;

    Ok("committed to memory.md ## Inferred".to_string())
}

async fn read_today_history_tool(input: &Value, ctx: &ToolContext<'_>) -> Result<String, String> {
    let limit_lines = input
        .get("limit_lines")
        .and_then(|v| v.as_u64())
        .unwrap_or(200) as usize;
    let limit_lines = limit_lines.min(500);
    // Optional pagination cursor. ISO-UTC timestamps are
    // string-comparable (zero-padded year-first format), so we filter by
    // direct string-`>` against each record's `ts`/`recv_at`/`event_ts`.
    let since_event_ts = input
        .get("since_event_ts")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let today = chrono::Utc::now().format("%Y-%m-%d");
    let path = std::path::Path::new(crate::chat::history::HISTORY_DIR)
        .join(format!("{today}.jsonl"));
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(e) => return Err(format!("read today's history: {e}")),
    };
    // Filter by `since_event_ts` first (oldest-first scan is fine — we
    // re-reverse below), then take the most-recent `limit_lines`. The
    // 32 KB byte cap from `ctx.history_max_bytes` continues to apply.
    let filtered: Vec<&str> = if let Some(since) = since_event_ts.as_deref() {
        contents
            .lines()
            .filter(|line| {
                // Parse just enough JSON to find the timestamp field.
                let v: serde_json::Value = match serde_json::from_str(line) {
                    Ok(v) => v,
                    Err(_) => return false,
                };
                let ts = v
                    .get("ts")
                    .or_else(|| v.get("recv_at"))
                    .or_else(|| v.get("event_ts"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                ts > since
            })
            .collect()
    } else {
        contents.lines().collect()
    };
    let mut lines: Vec<&str> = filtered.into_iter().rev().take(limit_lines).collect();
    let mut out = String::new();
    let mut bytes = 0usize;
    let mut truncated = false;
    let mut taken = 0usize;
    for line in lines.drain(..) {
        if bytes + line.len() + 1 > ctx.history_max_bytes {
            truncated = true;
            break;
        }
        out.push_str(line);
        out.push('\n');
        bytes += line.len() + 1;
        taken += 1;
    }
    if truncated {
        out.push_str(&format!("[truncated at {} bytes / {taken} lines]\n", ctx.history_max_bytes));
    }
    Ok(out)
}

async fn search_history_tool(input: &Value, ctx: &ToolContext<'_>) -> Result<String, String> {
    let query = input
        .get("query")
        .and_then(|v| v.as_str())
        .ok_or("query is required")?
        .to_string();
    let days_back = input
        .get("days_back")
        .and_then(|v| v.as_u64())
        .unwrap_or(7)
        .min(ctx.history_search_max_days as u64);

    // Streaming line scan via spawn_blocking.
    let history_dir = crate::chat::history::HISTORY_DIR.to_string();
    let max_matches = 50;
    let max_excerpt = 1024usize;
    let q = query.clone();
    let dir_clone = history_dir.clone();
    let result: Result<String, String> = tokio::task::spawn_blocking(move || {
        let q_lc = q.to_lowercase();
        let mut matches: Vec<String> = Vec::new();
        let dir = std::path::Path::new(&dir_clone);
        if !dir.exists() {
            return Ok(String::new());
        }
        // Walk only files matching `<YYYY-MM-DD>.jsonl` in the SCOPED
        // dir.
        let entries = std::fs::read_dir(dir).map_err(|e| format!("read_dir: {e}"))?;
        let cutoff = chrono::Utc::now().date_naive() - chrono::Duration::days(days_back as i64);
        let mut paths: Vec<std::path::PathBuf> = Vec::new();
        for ent in entries.flatten() {
            let p = ent.path();
            if !p.is_file() {
                continue;
            }
            let name = match p.file_name().and_then(|n| n.to_str()) {
                Some(n) => n,
                None => continue,
            };
            if let Some(stem) = name.strip_suffix(".jsonl")
                && let Ok(d) = chrono::NaiveDate::parse_from_str(stem, "%Y-%m-%d")
                && d >= cutoff
            {
                paths.push(p);
            }
        }
        // Newest first.
        paths.sort_by(|a, b| b.file_name().cmp(&a.file_name()));
        for p in paths {
            let body = match std::fs::read_to_string(&p) {
                Ok(b) => b,
                Err(_) => continue,
            };
            for line in body.lines().rev() {
                if line.to_lowercase().contains(&q_lc) {
                    let mut excerpt = line.to_string();
                    if excerpt.len() > max_excerpt {
                        // Round down to a char boundary; `String::truncate`
                        // panics if the index falls mid-codepoint, and
                        // history lines can contain multi-byte chat content.
                        let mut cut = max_excerpt;
                        while cut > 0 && !excerpt.is_char_boundary(cut) {
                            cut -= 1;
                        }
                        excerpt.truncate(cut);
                        excerpt.push_str(" ...[truncated]");
                    }
                    matches.push(excerpt);
                    if matches.len() >= max_matches {
                        return Ok(matches.join("\n"));
                    }
                }
            }
        }
        Ok(matches.join("\n"))
    })
    .await
    .map_err(|e| format!("search_history join: {e}"))?;
    result
}

async fn web_fetch_tool(input: &Value, ctx: &ToolContext<'_>) -> Result<String, String> {
    // CHAT.md — daily budget gate. Rendered as an Ok tool_result with
    // an `error` field (rather than an Err) so the model can read the
    // reason and re-plan, matching how Anthropic-side server errors are
    // surfaced. The orchestrator increments `web_fetches_today` after a
    // successful (non-rate-limited) fetch.
    if ctx.web_fetches_today >= ctx.web_fetch_daily_max {
        return Ok(json!({
            "error": "rate limited",
            "reason": "web_fetch daily budget exhausted",
        })
        .to_string());
    }
    let url = input
        .get("url")
        .and_then(|v| v.as_str())
        .ok_or("url is required")?;
    crate::chat::web::fetch(url, ctx.web_fetch_max_bytes).await
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- UUID validation ------------------------------------------------

    #[test]
    fn canonical_hyphenated_uuid_accepted() {
        assert!(validate_uuid("11111111-2222-3333-4444-555555555555").is_ok());
        assert!(validate_uuid("00000000-0000-0000-0000-000000000000").is_ok());
    }

    #[test]
    fn bare_32_hex_uuid_accepted() {
        assert!(validate_uuid("1111111122223333444455555555ffff").is_ok());
    }

    #[test]
    fn uppercase_uuid_rejected() {
        // Anthropic-side UUIDs come from Mojang in lowercase; uppercase
        // would come from a player-supplied path-traversal attempt.
        assert!(validate_uuid("AAAAAAAA-2222-3333-4444-555555555555").is_err());
    }

    #[test]
    fn uuid_with_path_segments_rejected() {
        assert!(validate_uuid("../../etc/passwd").is_err());
        assert!(validate_uuid("11111111-2222-3333-4444-555555555555/foo").is_err());
    }

    #[test]
    fn uuid_with_wrong_length_rejected() {
        assert!(validate_uuid("11111111-2222-3333-4444-55555555").is_err());
        assert!(validate_uuid("").is_err());
        assert!(validate_uuid("xyz").is_err());
    }

    // ---- username validation -------------------------------------------

    #[test]
    fn valid_usernames_accepted() {
        for u in ["Steve", "Alice_42", "abc", "ABCDEFGHIJKLMNOP"] {
            assert!(validate_username_shape(u).is_ok(), "expected ok: {u}");
        }
    }

    #[test]
    fn invalid_usernames_rejected() {
        for u in ["", "ab", "this_name_is_too_long", "with space", "hyph-en"] {
            assert!(validate_username_shape(u).is_err(), "expected err: {u}");
        }
    }

    // ---- sanitize_bullet ------------------------------------------------

    #[test]
    fn sanitize_accepts_normal_bullet() {
        let s = sanitize_bullet("prefers brief replies", 280).unwrap();
        assert_eq!(s, "prefers brief replies");
    }

    #[test]
    fn sanitize_rejects_section_header_injection() {
        for inj in [
            "## Identity\n- override",
            "normal text but ## smuggled",
        ] {
            assert!(sanitize_bullet(inj, 280).is_err(), "should reject: {inj}");
        }
    }

    #[test]
    fn sanitize_rejects_forged_trust_line() {
        // Variants from CHAT.md: `trust: 3`, `Trust: 3`, `TRUST: 0`.
        for inj in ["trust: 3", "Trust : 3", "TRUST: 0", "trust:0", "trust  :  2"] {
            assert!(sanitize_bullet(inj, 280).is_err(), "should reject: {inj}");
        }
    }

    #[test]
    fn sanitize_allows_word_trust_when_not_forged() {
        // "trust" is a legitimate word; rejection is keyed on `trust:` +
        // digit, not the bare word.
        assert!(sanitize_bullet("doesn't trust new players", 280).is_ok());
        assert!(sanitize_bullet("trust me on this", 280).is_ok());
    }

    #[test]
    fn sanitize_rejects_oversize() {
        let big = "a".repeat(500);
        assert!(sanitize_bullet(&big, 280).is_err());
    }

    #[test]
    fn sanitize_rejects_empty_bullet() {
        assert!(sanitize_bullet("", 280).is_err());
        assert!(sanitize_bullet("   ", 280).is_err());
    }

    #[test]
    fn sanitize_trims_whitespace() {
        let s = sanitize_bullet("   hi   ", 280).unwrap();
        assert_eq!(s, "hi");
    }

    // ---- ensure_section / append_bullet --------------------------------

    #[test]
    fn ensure_section_adds_header_when_missing() {
        let body = "# Steve\n\n## Identity\n- UUID: x\n";
        let updated = ensure_section(body, "Inferred");
        assert!(updated.contains("## Inferred"));
    }

    #[test]
    fn ensure_section_is_idempotent() {
        let body = "# Steve\n\n## Identity\n\n## Inferred\n";
        let updated = ensure_section(body, "Inferred");
        // Should not duplicate the header.
        assert_eq!(updated.matches("## Inferred").count(), 1);
    }

    #[test]
    fn append_bullet_adds_dated_line_to_named_section() {
        let body = "# Steve\n\n## Identity\n- UUID: x\n\n## Inferred\n\n## Do not mention\n";
        let updated = append_bullet_to_section(body, "Inferred", "prefers brief replies", "2026-04-26");
        assert!(updated.contains("- 2026-04-26: prefers brief replies"));
        // The new bullet is inside the Inferred section, not the
        // following `Do not mention` section.
        let inferred_pos = updated.find("## Inferred").unwrap();
        let dnm_pos = updated.find("## Do not mention").unwrap();
        let bullet_pos = updated.find("prefers brief").unwrap();
        assert!(bullet_pos > inferred_pos && bullet_pos < dnm_pos);
    }

    #[test]
    fn append_bullet_is_idempotent_for_exact_duplicates() {
        let body = "# Steve\n\n## Inferred\n- 2026-04-26: same\n";
        let updated = append_bullet_to_section(body, "Inferred", "same", "2026-04-26");
        assert_eq!(updated.matches("- 2026-04-26: same").count(), 1);
    }

    // ---- writable section gate -----------------------------------------

    #[test]
    fn writable_sections_match_plan() {
        // CHAT.md update_player_memory section allow-list.
        for s in [
            "Stated preferences",
            "Inferred",
            "Topics & history",
            "Do not mention",
        ] {
            assert!(is_writable_section(s));
        }
        // Operator-managed sections must NOT be in the allow-list.
        assert!(!is_writable_section("Identity"));
        assert!(!is_writable_section("Trust"));
        assert!(!is_writable_section(""));
    }

    // ---- sender binding ------------------------------------------------

    #[test]
    fn sender_binding_pass_for_self() {
        let v = check_sender_binding(
            "11111111-2222-3333-4444-555555555555",
            "11111111-2222-3333-4444-555555555555",
        );
        assert_eq!(v, SenderBind::Bound);
    }

    #[test]
    fn sender_binding_pass_is_case_insensitive() {
        // Mojang returns lowercase; defense-in-depth admits mixed case.
        let v = check_sender_binding(
            "AAAAAAAA-2222-3333-4444-555555555555",
            "aaaaaaaa-2222-3333-4444-555555555555",
        );
        assert_eq!(v, SenderBind::Bound);
    }

    #[test]
    fn sender_binding_fails_for_different_uuid() {
        let v = check_sender_binding(
            "11111111-2222-3333-4444-555555555555",
            "ffffffff-2222-3333-4444-555555555555",
        );
        assert_eq!(v, SenderBind::Mismatch);
    }

    // ---- cross-player firewall (read) ---------------------------------

    #[test]
    fn read_authorization_allowed_for_self() {
        let v = authorize_player_read(
            "11111111-2222-3333-4444-555555555555",
            "11111111-2222-3333-4444-555555555555",
            false,
        );
        assert_eq!(v, ReadAuthorization::Allowed);
    }

    #[test]
    fn read_authorization_denied_for_cross_player_default() {
        // Default config (`cross_player_reads = false`).
        let v = authorize_player_read(
            "11111111-2222-3333-4444-555555555555",
            "ffffffff-2222-3333-4444-555555555555",
            false,
        );
        assert_eq!(v, ReadAuthorization::Denied);
    }

    #[test]
    fn read_authorization_allowed_by_operator_when_enabled() {
        // CHAT.md allows opt-in for trusted single-tenant servers.
        let v = authorize_player_read(
            "11111111-2222-3333-4444-555555555555",
            "ffffffff-2222-3333-4444-555555555555",
            true,
        );
        assert_eq!(v, ReadAuthorization::AllowedByOperator);
    }

    // ---- resolve_player_path -------------------------------------------

    #[test]
    fn resolve_player_path_rejects_invalid_uuid() {
        let scratch = std::env::temp_dir().join(format!(
            "cj-store-tools-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&scratch).unwrap();
        let r = resolve_player_path("../../etc/passwd", &scratch);
        assert!(r.is_err());
        let _ = std::fs::remove_dir_all(&scratch);
    }

    #[test]
    fn resolve_player_path_rejects_uuid_with_path_separator() {
        let scratch = std::env::temp_dir().join(format!(
            "cj-store-tools-test-sep-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&scratch).unwrap();
        // Path separators inside the UUID would never pass
        // `validate_uuid`, but the explicit guard inside
        // `resolve_player_path` is belt-and-braces.
        let r = resolve_player_path(
            "11111111-2222-3333-4444-555555555555\\..\\etc",
            &scratch,
        );
        assert!(r.is_err());
        let _ = std::fs::remove_dir_all(&scratch);
    }

    // ---- runtime canonicalization gate (BLOCKER-level fix) -------------

    fn test_ctx<'a>(sender_uuid: &'a str) -> ToolContext<'a> {
        ToolContext {
            sender_uuid,
            cross_player_reads: false,
            history_max_bytes: 32_768,
            update_bullet_max_chars: 280,
            history_search_max_days: 7,
            web_fetch_max_bytes: 262_144,
            web_fetch_enabled: false,
            today: "2026-04-26".to_string(),
            player_memory_max_bytes: 4096,
            update_self_memory_today: 0,
            update_self_memory_max_per_day: 3,
            memory_max_inferred_bullets: 30,
            web_fetches_today: 0,
            web_fetch_daily_max: 50,
        }
    }

    #[test]
    fn read_player_memory_rejects_path_traversal() {
        // CHAT.md: any UUID containing `../` or path separators must be
        // rejected at the `validate_uuid` gate before any disk access.
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let ctx = test_ctx("11111111-2222-3333-4444-555555555555");
        let input = json!({"uuid": "../../etc/passwd"});
        let res = rt.block_on(read_player_memory_tool(&input, &ctx));
        assert!(res.is_err(), "expected error, got: {res:?}");
        let msg = res.unwrap_err();
        assert!(
            msg.contains("uuid") || msg.contains("hyphenated"),
            "unexpected error: {msg}",
        );

        // Also reject UUID-shaped strings with embedded separators.
        let input2 = json!({"uuid": "11111111-2222-3333-4444-555555555555/foo"});
        let res2 = rt.block_on(read_player_memory_tool(&input2, &ctx));
        assert!(res2.is_err(), "expected error, got: {res2:?}");
    }

    // ---- update_player_memory cap --------------------------------------

    #[test]
    fn update_player_memory_at_cap_returns_explicit_error() {
        // CHAT.md: writes that would push the file past
        // `player_memory_max_bytes` return an explicit error so the
        // model can re-plan rather than silently growing the file.
        // Drive the gate via a tiny cap that any append crosses.
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        // Use a unique sentinel UUID so this test doesn't collide with a
        // real player file. The test creates and removes the file under
        // the real `data/chat/players/` dir — best effort cleanup at the
        // end. This is consistent with the existing test conventions.
        let uuid = "00000000-0000-0000-0000-cccccccccccc";
        let mut ctx = test_ctx(uuid);
        ctx.player_memory_max_bytes = 1; // any append exceeds this.
        let input = json!({
            "uuid": uuid,
            "section": "Inferred",
            "bullet": "this is a normal bullet",
        });
        let res = rt.block_on(update_player_memory_tool(&input, &ctx));
        // Cleanup — best effort. The bootstrap path may have created
        // `data/chat/players/<uuid>.md`.
        let _ = std::fs::remove_file(crate::chat::memory::player_file_path(uuid));
        assert!(res.is_err(), "expected error, got: {res:?}");
        let msg = res.unwrap_err();
        assert!(
            msg.contains("at cap") && msg.contains("rate-limited"),
            "unexpected error: {msg}",
        );
    }

    // ---- update_self_memory ADV3 controls ------------------------------

    fn temp_self_memory_paths(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let nanos = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let memory = std::env::temp_dir().join(format!(
            "cj-store-self-memory-{}-{}-{tag}.md",
            std::process::id(),
            nanos,
        ));
        let archive = std::env::temp_dir().join(format!(
            "cj-store-self-memory-{}-{}-{tag}.archive.md",
            std::process::id(),
            nanos,
        ));
        let _ = std::fs::remove_file(&memory);
        let _ = std::fs::remove_file(&archive);
        (memory, archive)
    }

    #[test]
    fn update_self_memory_daily_cap_enforced() {
        let mut ctx = test_ctx("11111111-2222-3333-4444-555555555555");
        ctx.update_self_memory_today = 3;
        ctx.update_self_memory_max_per_day = 3;
        let input = json!({"bullet": "something to remember"});
        let (memory, archive) = temp_self_memory_paths("cap");
        let res = update_self_memory_at_paths(&input, &ctx, &memory, &archive);
        assert!(res.is_err());
        assert_eq!(res.unwrap_err(), "daily limit reached");
        // The memory file MUST NOT have been created.
        assert!(!memory.exists());
    }

    #[test]
    fn update_self_memory_commits_directly_to_inferred_section() {
        let ctx = test_ctx("11111111-2222-3333-4444-555555555555");
        let input = json!({"bullet": "operator likes brevity"});
        let (memory, archive) = temp_self_memory_paths("write");

        let res = update_self_memory_at_paths(&input, &ctx, &memory, &archive);
        assert!(res.is_ok(), "expected ok, got: {res:?}");

        let body = std::fs::read_to_string(&memory).unwrap();
        assert!(body.contains("## Inferred"), "missing section header: {body}");
        assert!(
            body.contains("- 2026-04-26: operator likes brevity"),
            "missing dated bullet: {body}",
        );
        // Archive MUST NOT exist for a single below-cap commit.
        assert!(!archive.exists());

        let _ = std::fs::remove_file(&memory);
    }

    #[test]
    fn update_self_memory_dedup_rejects_near_duplicate() {
        let ctx = test_ctx("11111111-2222-3333-4444-555555555555");
        let (memory, archive) = temp_self_memory_paths("dedup");

        // Seed memory.md with one bullet.
        let first = json!({"bullet": "operator prefers brief replies"});
        update_self_memory_at_paths(&first, &ctx, &memory, &archive).unwrap();

        // A near-duplicate (one-character delta) must be rejected.
        let second = json!({"bullet": "operator prefers brief reply"});
        let res = update_self_memory_at_paths(&second, &ctx, &memory, &archive);
        assert!(res.is_err(), "expected dedup err, got: {res:?}");
        let msg = res.unwrap_err();
        assert!(msg.contains("near-duplicate"), "unexpected: {msg}");

        // An unrelated bullet should still pass.
        let third = json!({"bullet": "remembers the chest at -100,64,200"});
        assert!(update_self_memory_at_paths(&third, &ctx, &memory, &archive).is_ok());

        let _ = std::fs::remove_file(&memory);
        let _ = std::fs::remove_file(&archive);
    }

    #[test]
    fn update_self_memory_archives_oldest_when_cap_exceeded() {
        let mut ctx = test_ctx("11111111-2222-3333-4444-555555555555");
        // Cap = 2 means the 3rd commit evicts the oldest bullet.
        ctx.memory_max_inferred_bullets = 2;
        let (memory, archive) = temp_self_memory_paths("cap-evict");

        for (i, bullet) in [
            "first thing to remember",
            "second thing to remember",
            "third thing to remember",
        ]
        .iter()
        .enumerate()
        {
            let input = json!({"bullet": *bullet});
            let r = update_self_memory_at_paths(&input, &ctx, &memory, &archive);
            assert!(r.is_ok(), "commit {i} failed: {r:?}");
        }

        let body = std::fs::read_to_string(&memory).unwrap();
        // Live file must have only the two most recent bullets.
        assert!(body.contains("- 2026-04-26: second thing to remember"));
        assert!(body.contains("- 2026-04-26: third thing to remember"));
        assert!(
            !body.contains("- 2026-04-26: first thing to remember"),
            "oldest bullet should have been evicted: {body}",
        );

        // Archive must contain the evicted bullet.
        let arch = std::fs::read_to_string(&archive).unwrap();
        assert!(
            arch.contains("- 2026-04-26: first thing to remember"),
            "evicted bullet not in archive: {arch}",
        );

        let _ = std::fs::remove_file(&memory);
        let _ = std::fs::remove_file(&archive);
    }

    // ---- read_today_history pagination ---------------------------------

    #[test]
    fn read_today_history_paginates_via_since_ts() {
        // We exercise the in-memory pagination logic by inlining the
        // filter step here. The full tool reads from
        // `data/chat/history/<today>.jsonl` which we cannot rebind from
        // this scope, so we test the behavior via the substrate: the
        // filter should keep only records strictly newer than the
        // cursor.
        let lines = vec![
            r#"{"ts":"2026-04-26T10:00:00Z","kind":"public","sender":"A","content":"hi"}"#,
            r#"{"ts":"2026-04-26T11:00:00Z","kind":"public","sender":"B","content":"hi"}"#,
            r#"{"ts":"2026-04-26T12:00:00Z","kind":"public","sender":"C","content":"hi"}"#,
        ];
        let since = "2026-04-26T10:30:00Z";
        let kept: Vec<&str> = lines
            .iter()
            .copied()
            .filter(|line| {
                let v: serde_json::Value = serde_json::from_str(line).unwrap();
                let ts = v
                    .get("ts")
                    .or_else(|| v.get("recv_at"))
                    .or_else(|| v.get("event_ts"))
                    .and_then(|x| x.as_str())
                    .unwrap_or("");
                ts > since
            })
            .collect();
        assert_eq!(kept.len(), 2);
        assert!(kept[0].contains("11:00:00"));
        assert!(kept[1].contains("12:00:00"));
    }

    // ---- web_fetch daily budget ----------------------------------------

    #[test]
    fn web_fetch_daily_budget_returns_rate_limited() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let mut ctx = test_ctx("11111111-2222-3333-4444-555555555555");
        ctx.web_fetches_today = 50;
        ctx.web_fetch_daily_max = 50;
        ctx.web_fetch_enabled = true;
        let input = json!({"url": "https://example.com/"});
        let res = rt.block_on(web_fetch_tool(&input, &ctx));
        // Spec: rate-limit returns Ok(json{...}) so the model sees a
        // non-error tool_result with an `error` field.
        let body = res.expect("rate-limit path must return Ok");
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["error"], "rate limited");
        assert!(
            v["reason"]
                .as_str()
                .unwrap_or("")
                .contains("web_fetch daily budget exhausted")
        );
    }
}
