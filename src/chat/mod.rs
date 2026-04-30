//! # Chat module — natural-language chat AI
//!
//! Disabled by default behind `chat.enabled = false`. See CHAT.md for the
//! full design. The orchestration entry point is [`chat_task`]; per-event
//! flow (pre-filter → classifier → composer → pacing → send) lives in
//! [`process_event`]. Whisper routing happens at the bot layer via
//! [`conversation::route_whisper`].

pub mod classifier;
pub mod client;
pub mod composer;
pub mod conversation;
pub mod decisions;
pub mod history;
pub mod memory;
pub mod pacing;
pub mod persona;
pub mod pricing;
pub mod reflection;
pub mod retention;
pub mod state;
pub mod tools;
pub mod web;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::sync::{RwLock, broadcast, mpsc, oneshot};
use tracing::{debug, error, info, warn};

use crate::config::ChatConfig;
use crate::messages::{BotInstruction, ChatCommand, ChatEvent, ChatEventKind};

/// Snapshot returned by `Chat: status`. Operator-facing
/// — keep field names stable.
#[derive(Debug, Clone, Default)]
pub struct ChatStatusReport {
    pub enabled: bool,
    pub paused: bool,
    pub dry_run_effective: bool,
    pub bot_username: Option<String>,
    pub composer_input_today: u64,
    pub composer_output_today: u64,
    pub classifier_input_today: u64,
    pub classifier_output_today: u64,
    pub estimated_usd_today: f64,
    pub usd_cap: f64,
    pub history_drops_today: u64,
    pub moderation_backoff_until: Option<String>,
    /// CHAT.md — operator-facing fields filled by chat orchestrator.
    pub model_404_backoff_until: Option<String>,
    pub composer_throttle_backoff_until: Option<String>,
    pub persona_regen_cooldown_until: Option<String>,
    pub last_persona_regenerated_at: Option<String>,
    pub pending_adjustments_count: u32,
    pub critical_section_active: bool,
    pub last_composer_call_at: Option<String>,
    pub last_composer_call_usd: f64,
    pub web_fetches_today: u32,
    /// Number of senders currently tracked in the classifier per-sender
    /// counter — operator-visible measure of the active classifier load.
    pub classifier_active_senders: usize,
}

/// Run the chat task.
///
/// **Quick exit when disabled.** If `config.enabled == false` the task
/// drops every channel and returns immediately, so trade-only operators
/// pay zero CPU and require no Anthropic API key.
///
/// **Panic isolation** is supplied by the caller in `main.rs` — this
/// function is wrapped in an outer `tokio::spawn` whose `JoinError` is
/// caught and logged, so a chat panic never tears down the trade bot.
///
/// Phase 1 behavior: when enabled, log every received `ChatEvent` at debug
/// level and drain the channels. Composition, persona, memory, and tools
/// arrive in later phases.
pub async fn chat_task(
    mut chat_events_rx: broadcast::Receiver<ChatEvent>,
    bot_tx: mpsc::Sender<BotInstruction>,
    mut chat_cmd_rx: mpsc::Receiver<ChatCommand>,
    in_critical_section: Arc<AtomicBool>,
    bot_username: Arc<RwLock<Option<String>>>,
    config: ChatConfig,
) {
    if !config.enabled {
        info!("[Chat] disabled (config.chat.enabled=false), task exiting");
        drop(chat_events_rx);
        drop(chat_cmd_rx);
        return;
    }

    info!(
        "[Chat] enabled (dry_run={}); fully wired — pre-filter -> classifier -> composer -> pacing",
        config.dry_run
    );

    // Load persistent supporting state.
    let pricing = match pricing::PricingTable::load_or_create() {
        Ok(t) => t,
        Err(e) => {
            warn!(error = %e, "[Chat] pricing.json load failed, using defaults");
            pricing::PricingTable::default_table()
        }
    };
    // Acquire the API key. Failure is loud and self-disables.
    let api_key = match client::ApiKey::from_env(&config.api_key_env) {
        Ok(k) => k,
        Err(e) => {
            error!(env = %config.api_key_env, error = %e, "[Chat] API key not set; chat self-disabling");
            return;
        }
    };

    // Persona — generate on first run.
    let mut persona_body = match persona::load() {
        Ok(Some(b)) => b,
        Ok(None) => {
            info!("[Chat] persona.md missing, generating from seed");
            match persona::generate(&api_key, &config.persona_seed, &config.composer_model, &[]).await {
                Ok(b) => b,
                Err(e) => {
                    error!(error = %e, "[Chat] persona generation failed; chat self-disabling");
                    return;
                }
            }
        }
        Err(e) => {
            error!(error = %e, "[Chat] persona.md unreadable; chat self-disabling");
            return;
        }
    };
    let persona_name = persona::extract_name(&persona_body).unwrap_or_default();
    let mut persona_nicknames = persona::extract_nicknames(&persona_body);
    if !persona_name.is_empty() {
        persona_nicknames.insert(0, persona_name.clone());
    }
    let mut persona_lowercase_default = persona::declares_lowercase_default(&persona_body);
    info!(
        bot_persona = %persona_name,
        nicknames = ?persona_nicknames,
        lowercase_default = persona_lowercase_default,
        "[Chat] persona loaded"
    );

    // CHAT.md: print the daily ceiling at startup so operators can
    // see what they're spending without doing the math from token rates.
    info!("[Chat] {}", client::format_daily_ceiling_log_line(&config, &pricing));

    // Operator-managed dictionaries / filters loaded once at startup.
    // Hot-reload is not implemented; restart the process to pick up
    // edits to these files.
    let common_words: Vec<String> =
        conversation::load_lines_or_empty(&format!("{}/common_words.txt", memory::CHAT_DIR));
    let blocklist: std::collections::HashSet<String> =
        conversation::load_blocklist(&format!("{}/blocklist.txt", memory::CHAT_DIR));
    let system_senders_re: Vec<String> = conversation::load_lines_or_empty(
        &format!("{}/system_senders_re.txt", memory::CHAT_DIR),
    );
    let system_senders_exact: Vec<String> = conversation::load_lines_or_empty(
        &format!("{}/system_senders.txt", memory::CHAT_DIR),
    );
    let moderation_patterns = conversation::ModerationPatterns::load_with_defaults(
        &format!("{}/moderation_patterns.txt", memory::CHAT_DIR),
        &persona_name,
    );

    // Load runtime state (token meter, pause flag, backoff timers).
    let mut runtime_state = match state::ChatState::load_or_default() {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "[Chat] state.json load failed, using defaults");
            state::ChatState::default()
        }
    };

    // CHAT.md: seed `bot_username` from the cached `last_known_bot_username`
    // so events arriving in the pre-Init window (join broadcasts, early
    // chat) aren't dropped with `bot_username_unknown`. `Event::Init`
    // overwrites this with the authoritative value once the handshake
    // completes, and divergence between the two is logged in `bot::handle_event`.
    if let Some(cached) = runtime_state.last_known_bot_username.clone() {
        let mut guard = bot_username.write().await;
        if guard.is_none() {
            info!(cached_username = %cached, "[Chat] seeded bot_username from cached state for pre-Init window");
            *guard = Some(cached);
        }
    }

    // Run the retention sweep on startup. Honor config values rather
    // than the 30/30/10 defaults — operators that lowered retention
    // were silently kept on the defaults at startup; only the daily
    // sweep used the configured values.
    if let Some(today) = retention::sweep_due_today(None) {
        let cfg = retention::SweepConfig {
            chat_dir: PathBuf::from(memory::CHAT_DIR),
            history_retention_days: config.history_retention_days,
            decisions_retention_days: config.decisions_retention_days,
            persona_archive_max: config.persona_archive_max,
            today: chrono::Utc::now(),
        };
        let report = retention::run_sweep(&cfg);
        info!(
            today = %today,
            deleted = report.total(),
            "[Chat] startup retention sweep complete"
        );
        // Record so the per-event "first event of new day" trigger
        // doesn't immediately re-run the same sweep.
        runtime_state.last_sweep_day = Some(today);
    }

    // Per-channel sliding window of last 8 events for dyad detection.
    let mut window: VecDeque<ChatEvent> = VecDeque::with_capacity(8);
    // Per-sender classifier dispatch counter.
    let mut classifier_counter = classifier::PerSenderCounter::new();
    // Per-sender spam guard.
    let mut spam_guard = conversation::SpamGuard::new();
    // Recent-speaker map: sender -> last interaction Instant.
    let mut recent_speakers: HashMap<String, Instant> = HashMap::new();
    // Track replies sent in the trailing 60 s for max_replies_per_minute.
    let mut recent_bot_send_times: VecDeque<Instant> = VecDeque::with_capacity(8);
    let mut last_bot_send_at: Option<Instant> = None;
    // Per-model client-side RPM/ITPM rate limiters. CHAT.md prevents 429
    // spirals from eating the retry budget by blocking briefly on the
    // local bucket before the actual HTTPS round-trip.
    let composer_limiter = client::RateLimiter::new(
        config.composer_rpm_max,
        config.composer_itpm_max,
        config.rate_limit_wait_max_secs,
    );
    let classifier_limiter = client::RateLimiter::new(
        config.classifier_rpm_max,
        config.classifier_itpm_max,
        config.rate_limit_wait_max_secs,
    );

    loop {
        tokio::select! {
            cmd = chat_cmd_rx.recv() => {
                match cmd {
                    Some(ChatCommand::Shutdown { ack }) => {
                        info!("[Chat] shutdown command received, draining and exiting");
                        // Best-effort drain of the broadcast so any in-flight
                        // events are observed before we leave.
                        while let Ok(ev) = chat_events_rx.try_recv() {
                            debug!(
                                kind = ?ev.kind,
                                sender = %ev.sender,
                                content_len = ev.content.len(),
                                "[Chat] draining residual event on shutdown"
                            );
                        }
                        // Persist final state on shutdown.
                        if let Err(e) = runtime_state.save() {
                            warn!(error = %e, "[Chat] final state save failed");
                        }
                        let _ = ack.send(());
                        return;
                    }
                    Some(ChatCommand::Status { respond_to }) => {
                        runtime_state.roll_to_today();
                        // Count pending_adjustments.jsonl lines for status display.
                        let pending_count = std::fs::read_to_string(
                            std::path::Path::new(memory::CHAT_DIR).join("pending_adjustments.jsonl"),
                        )
                        .map(|s| s.lines().count() as u32)
                        .unwrap_or(0);
                        let report = ChatStatusReport {
                            enabled: config.enabled,
                            paused: runtime_state.paused,
                            dry_run_effective: config.dry_run || runtime_state.dry_run_runtime_override,
                            bot_username: bot_username.read().await.clone(),
                            composer_input_today: runtime_state.tokens_today.composer_input,
                            composer_output_today: runtime_state.tokens_today.composer_output,
                            classifier_input_today: runtime_state.tokens_today.classifier_input,
                            classifier_output_today: runtime_state.tokens_today.classifier_output,
                            estimated_usd_today: runtime_state.tokens_today.estimated_usd,
                            usd_cap: config.daily_dollar_cap_usd,
                            history_drops_today: runtime_state.history_drops_today,
                            moderation_backoff_until: runtime_state.moderation_backoff_until.clone(),
                            model_404_backoff_until: runtime_state.model_404_backoff_until.clone(),
                            composer_throttle_backoff_until: runtime_state
                                .composer_throttle_backoff_until
                                .clone(),
                            persona_regen_cooldown_until: runtime_state.persona_regen_cooldown_until.clone(),
                            last_persona_regenerated_at: runtime_state
                                .last_persona_regenerated_at
                                .clone(),
                            pending_adjustments_count: pending_count,
                            critical_section_active: in_critical_section.load(Ordering::Acquire),
                            last_composer_call_at: runtime_state
                                .last_composer_call
                                .as_ref()
                                .map(|c| c.at_utc.clone()),
                            last_composer_call_usd: runtime_state
                                .last_composer_call
                                .as_ref()
                                .map(|c| c.usd)
                                .unwrap_or(0.0),
                            web_fetches_today: runtime_state.web_fetches_today,
                            classifier_active_senders: classifier_counter.active_senders(),
                        };
                        let _ = respond_to.send(report);
                    }
                    Some(ChatCommand::SetPaused { paused, respond_to }) => {
                        runtime_state.roll_to_today();
                        runtime_state.paused = paused;
                        info!(paused, "[Chat] pause flag updated");
                        if let Err(e) = runtime_state.save() {
                            warn!(error = %e, "[Chat] state save failed after SetPaused");
                        }
                        let _ = respond_to.send(());
                    }
                    Some(ChatCommand::SetDryRun { dry_run, respond_to }) => {
                        runtime_state.roll_to_today();
                        runtime_state.dry_run_runtime_override = dry_run;
                        info!(dry_run, "[Chat] dry-run override updated");
                        if let Err(e) = runtime_state.save() {
                            warn!(error = %e, "[Chat] state save failed after SetDryRun");
                        }
                        let _ = respond_to.send(());
                    }
                    Some(ChatCommand::ClearModerationBackoff { respond_to }) => {
                        runtime_state.roll_to_today();
                        let was = runtime_state.moderation_backoff_until.take();
                        info!(prior = ?was, "[Chat] moderation backoff cleared");
                        if let Err(e) = runtime_state.save() {
                            warn!(error = %e, "[Chat] state save failed after ClearModerationBackoff");
                        }
                        let _ = respond_to.send(());
                    }
                    Some(ChatCommand::RunRetentionSweep { respond_to }) => {
                        let cfg = retention::SweepConfig {
                            chat_dir: PathBuf::from(memory::CHAT_DIR),
                            history_retention_days: 30,
                            decisions_retention_days: 30,
                            persona_archive_max: 10,
                            today: chrono::Utc::now(),
                        };
                        let report = retention::run_sweep(&cfg);
                        info!(deleted = report.total(), "[Chat] on-demand retention sweep complete");
                        let _ = respond_to.send(report);
                    }
                    Some(ChatCommand::RunReflection { respond_to }) => {
                        let pending = match reflection::read_pending() {
                            Ok(p) => p,
                            Err(e) => {
                                let _ = respond_to.send(Err(format!("read pending: {e}")));
                                continue;
                            }
                        };
                        if pending.is_empty() {
                            let _ = respond_to.send(Ok(reflection::ReflectionOutcome::default()));
                            continue;
                        }
                        // Operator-triggered runs use a permissive
                        // validator: distinct-senders/triggers checks
                        // are bypassed (they apply to auto-triggered
                        // runs only — see CHAT.md).
                        let validator = reflection::MultiAxisValidator {
                            min_distinct_triggers: 1,
                            min_distinct_senders: 1,
                            substring_overlap_threshold: 0.40,
                        };
                        // Trust function: every sender is "trusted" for
                        // operator-triggered runs.
                        let trust = |_s: &str| 3u8;
                        let today_iso = chrono::Utc::now().format("%Y-%m-%d").to_string();
                        let adj = read_adjustments_or_empty();
                        let result = reflection::run_pass(
                            &api_key,
                            &config.classifier_model,
                            &pending,
                            &adj,
                            &trust,
                            &validator,
                            &today_iso,
                        )
                        .await;
                        if let Ok(ref outcome) = result {
                            let usd = pricing.usd_for_call(
                                &config.classifier_model,
                                outcome.haiku_input_tokens,
                                outcome.haiku_output_tokens,
                                outcome.haiku_cache_creation_input_tokens,
                                outcome.haiku_cache_read_input_tokens,
                            );
                            runtime_state.record_classifier(
                                &state::capture_today_utc(),
                                outcome.haiku_input_tokens,
                                outcome.haiku_output_tokens,
                                usd,
                            );
                            if let Err(e) = runtime_state.save() {
                                warn!(error = %e, "[Chat] state save failed after reflection");
                            }
                            decisions::write(
                                &decisions::DecisionRecord::new("reflection")
                                    .with_tokens(
                                        outcome.haiku_input_tokens,
                                        outcome.haiku_output_tokens,
                                        usd,
                                    )
                                    .extra("admitted", serde_json::Value::from(outcome.admitted.len()))
                                    .extra("rejected_substring", serde_json::Value::from(outcome.rejected_substring))
                                    .extra("rejected_distinct_triggers", serde_json::Value::from(outcome.rejected_distinct_triggers))
                                    .extra("rejected_distinct_senders", serde_json::Value::from(outcome.rejected_distinct_senders))
                                    .extra("rejected_low_trust", serde_json::Value::from(outcome.rejected_low_trust)),
                            );
                        }
                        let _ = respond_to.send(result);
                    }
                    Some(ChatCommand::BotDisconnected) => {
                        // CHAT.md in-flight cancellation. Today the
                        // composer call is sequential and short; just log
                        // the signal — the actual CancellationToken plumbing
                        // arrives in a follow-up. The signal remains useful
                        // because the chat task's bot_username read becomes
                        // None once Event::Disconnect fires, so subsequent
                        // events skip with "bot_username_unknown".
                        info!("[Chat] bot disconnected; in-flight composer (if any) will resolve on its own");
                    }
                    Some(ChatCommand::ShowDecisionLog { last_n, respond_to }) => {
                        // Read today's decisions JSONL and return the trailing
                        // `last_n` lines.
                        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
                        let path = std::path::Path::new(decisions::DECISIONS_DIR)
                            .join(format!("{today}.jsonl"));
                        let result = match std::fs::read_to_string(&path) {
                            Ok(body) => {
                                let lines: Vec<String> = body.lines().map(str::to_string).collect();
                                let start = lines.len().saturating_sub(last_n);
                                Ok(lines[start..].to_vec())
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
                            Err(e) => Err(e.to_string()),
                        };
                        let _ = respond_to.send(result);
                    }
                    Some(ChatCommand::ReplayEvent { event_ts, respond_to }) => {
                        // CHAT.md `Chat: replay event <ts>` — re-render the
                        // system prompt that WOULD be sent for the given
                        // historical event timestamp. Pure local replay; no
                        // API call, no token cost. Useful for "why did the
                        // bot say (or not say) that?" diagnostics.
                        let result = build_replay_prompt(
                            &event_ts,
                            &persona_body,
                        )
                        .await;
                        let _ = respond_to.send(result);
                    }
                    Some(ChatCommand::ResetPlayerMemory { username, respond_to }) => {
                        // Resolve username → UUID via the local index
                        // first, falling back to Mojang. Delete
                        // `data/chat/players/<uuid>.md`. The orchestrator
                        // owns this rather than the tools layer because
                        // the tools layer is sender-bound.
                        let result = match resolve_username_via_index_or_mojang(&username).await {
                            Ok(uuid) => {
                                let path = memory::player_file_path(&uuid);
                                if path.exists() {
                                    std::fs::remove_file(&path).map_err(|e| e.to_string())
                                } else {
                                    Ok(())
                                }
                            }
                            Err(e) => Err(e),
                        };
                        let _ = respond_to.send(result);
                    }
                    Some(ChatCommand::DumpPlayerMemory { username, respond_to }) => {
                        let result = match resolve_username_via_index_or_mojang(&username).await {
                            Ok(uuid) => {
                                let path = memory::player_file_path(&uuid);
                                std::fs::read_to_string(&path).map_err(|e| e.to_string())
                            }
                            Err(e) => Err(e),
                        };
                        let _ = respond_to.send(result);
                    }
                    Some(ChatCommand::SetOperatorTrust { username, set, reason, respond_to }) => {
                        // CHAT.md — write/clear `## Trust: 3` in the player
                        // file and append an entry to operator_audit.jsonl
                        // so trust toggles are auditable after the fact.
                        let result = match resolve_username_via_index_or_mojang(&username).await {
                            Ok(uuid) => {
                                memory::ensure_player_file(&uuid, &username)
                                    .map_err(|e| e.to_string())
                                    .and_then(|_| {
                                        let path = memory::player_file_path(&uuid);
                                        let body = std::fs::read_to_string(&path)
                                            .map_err(|e| e.to_string())?;
                                        let new_body = if set {
                                            // Replace any existing `## Trust: <n>` heading line.
                                            body.lines()
                                                .map(|l| {
                                                    if l.trim_start().starts_with("## Trust:") {
                                                        "## Trust: 3"
                                                    } else {
                                                        l
                                                    }
                                                })
                                                .collect::<Vec<_>>()
                                                .join("\n")
                                        } else {
                                            body.lines()
                                                .map(|l| {
                                                    if l.trim_start() == "## Trust: 3" {
                                                        "## Trust: 0"
                                                    } else {
                                                        l
                                                    }
                                                })
                                                .collect::<Vec<_>>()
                                                .join("\n")
                                        };
                                        crate::fsutil::write_atomic(&path, &new_body)
                                            .map_err(|e| e.to_string())
                                    })
                                    .inspect(|_| {
                                        // CHAT.md operator-audit ledger.
                                        // We do not block on the write —
                                        // this is best-effort.
                                        let action = if set { "set_trust3" } else { "clear_trust3" };
                                        write_operator_audit(action, &username, &uuid, &reason);
                                    })
                            }
                            Err(e) => Err(e),
                        };
                        let _ = respond_to.send(result);
                    }
                    Some(ChatCommand::RegeneratePersona { respond_to }) => {
                        // CHAT.md — regenerate persona, archive prior, set
                        // 24h cooldown. Full archive rotation is in
                        // `persona::regenerate`; we just call it.
                        // Archive prior persona body before regenerate.
                        // `persona::generate` overwrites persona.md atomically;
                        // we rotate the prior file with a UTC timestamp so
                        // hand-edits aren't lost.
                        let now_stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
                        let archive_path = std::path::Path::new(memory::CHAT_DIR)
                            .join(format!("persona.md.{now_stamp}"));
                        let live_path = std::path::Path::new(memory::CHAT_DIR).join("persona.md");
                        if live_path.exists()
                            && let Err(e) = std::fs::rename(&live_path, &archive_path)
                        {
                            warn!(error = %e, "[Chat] persona archive before regen failed; continuing");
                        }
                        let result = match persona::generate(
                            &api_key,
                            &config.persona_seed,
                            &config.composer_model,
                            &[],
                        )
                        .await
                        {
                            Ok(new_body) => {
                                persona_body = new_body;
                                persona_nicknames = persona::extract_nicknames(&persona_body);
                                if let Some(name) = persona::extract_name(&persona_body) {
                                    persona_nicknames.insert(0, name);
                                }
                                persona_lowercase_default =
                                    persona::declares_lowercase_default(&persona_body);
                                runtime_state.last_persona_regenerated_at =
                                    Some(state::iso_utc(chrono::Utc::now()));
                                if let Err(e) = runtime_state.save() {
                                    warn!(error = %e, "[Chat] state save failed after persona regen");
                                }
                                Ok(())
                            }
                            Err(e) => Err(e.to_string()),
                        };
                        let _ = respond_to.send(result);
                    }
                    Some(ChatCommand::ForgetPlayer { username, respond_to }) => {
                        // CHAT.md GDPR purge: delete the per-player file
                        // AND scrub matching records from history /
                        // decisions JSONL files plus any uuids overlay
                        // sidecars. The action is recorded to the
                        // operator audit ledger so a later compliance
                        // review can confirm the deletion happened.
                        let result = match resolve_username_via_index_or_mojang(&username).await {
                            Ok(uuid) => {
                                let player_path = memory::player_file_path(&uuid);
                                let player_outcome = if player_path.exists() {
                                    std::fs::remove_file(&player_path).map_err(|e| e.to_string())
                                } else {
                                    Ok(())
                                };
                                let scrub_outcome = scrub_history_for_player(&uuid, &username);
                                let outcome = match (player_outcome, scrub_outcome) {
                                    (Ok(()), Ok(stats)) => {
                                        info!(
                                            uuid = %uuid,
                                            username = %username,
                                            history_lines_removed = stats.history_lines,
                                            decisions_lines_removed = stats.decisions_lines,
                                            uuid_overlay_entries_removed = stats.overlay_entries,
                                            "[Chat] forget_player completed"
                                        );
                                        write_operator_audit("forget_player", &username, &uuid, "");
                                        Ok(())
                                    }
                                    (Err(e), _) | (_, Err(e)) => Err(e),
                                };
                                outcome
                            }
                            Err(e) => Err(e),
                        };
                        let _ = respond_to.send(result);
                    }
                    None => {
                        info!("[Chat] command channel closed, exiting");
                        if let Err(e) = runtime_state.save() {
                            warn!(error = %e, "[Chat] final state save failed");
                        }
                        return;
                    }
                }
            }
            ev = chat_events_rx.recv() => {
                match ev {
                    Ok(event) => {
                        // Refresh runtime state's day rollover (lazy reset).
                        runtime_state.roll_to_today();
                        // Maintain channel sliding window — but skip system
                        // pseudo-senders ([Server], plugin output, mod
                        // broadcasts). They aren't conversational and would
                        // pollute dyad detection: e.g. an alternating
                        // [Server]/player flood would synthesize a false
                        // dyad and silence legitimate player chat.
                        let is_system = conversation::is_system_pseudo_sender(
                            &event.sender,
                            &system_senders_re,
                            &system_senders_exact,
                        );
                        if !is_system {
                            if window.len() == 8 {
                                window.pop_front();
                            }
                            window.push_back(event.clone());
                        }

                        // Persist `last_known_bot_username` on every transition
                        //. Cheap — the read returns immediately
                        // because the lock is rarely contended in the chat
                        // task's hot path.
                        let live = bot_username.read().await.clone();
                        if live != runtime_state.last_known_bot_username {
                            runtime_state.last_known_bot_username = live;
                        }

                        // CHAT.md — fire the retention sweep on the first
                        // event of each new UTC day, in addition to startup.
                        let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
                        if retention::should_run_today(runtime_state.last_sweep_day.as_deref()) {
                            let cfg = retention::SweepConfig {
                                chat_dir: PathBuf::from(memory::CHAT_DIR),
                                history_retention_days: config.history_retention_days,
                                decisions_retention_days: config.decisions_retention_days,
                                persona_archive_max: config.persona_archive_max,
                                today: chrono::Utc::now(),
                            };
                            let report = retention::run_sweep(&cfg);
                            info!(today = %today, deleted = report.total(), "[Chat] daily retention sweep complete");
                            runtime_state.last_sweep_day = Some(today.clone());
                        }

                        // CHAT.md — auto-trigger reflection when the
                        // pending file's size cap or idle window is met.
                        // Both branches are gated by `min_interval_elapsed`
                        // so a quiet stretch can't fire reflection more
                        // often than the operator-configured floor.
                        if reflection::min_interval_elapsed(
                            runtime_state.last_reflection_at.as_deref(),
                            config.reflection_min_interval_secs,
                        ) {
                            let pending = reflection::read_pending().unwrap_or_default();
                            let last_composer_iso = runtime_state
                                .last_composer_call
                                .as_ref()
                                .map(|c| c.at_utc.clone());
                            let trigger = reflection::should_trigger_size_cap(
                                &pending,
                                config.reflection_max_pending,
                                config.reflection_min_distinct_senders,
                            ) || reflection::should_trigger_idle(
                                &pending,
                                last_composer_iso.as_deref(),
                                config.reflection_idle_trigger_secs,
                                config.reflection_min_distinct_senders,
                            );
                            if trigger {
                                runtime_state.last_reflection_at =
                                    Some(state::iso_utc(chrono::Utc::now()));
                                let validator = reflection::MultiAxisValidator {
                                    min_distinct_triggers: config.reflection_min_distinct_triggers as usize,
                                    min_distinct_senders: config.reflection_min_distinct_senders as usize,
                                    substring_overlap_threshold: 0.40,
                                };
                                // Trust function: looks up the per-player file
                                // and computes derived trust. Auto-triggered
                                // runs require Trust >= 1.
                                let history_dir = std::path::Path::new(history::HISTORY_DIR);
                                let trust_for_sender = |sender: &str| -> u8 {
                                    // Cache-only lookup: avoid stalling the
                                    // reflection pass on a Mojang fetch.
                                    // Senders not yet in cache are treated
                                    // as Trust 0 — the lesson validator
                                    // requires Trust ≥ 1 across multiple
                                    // distinct senders, so a cold cache
                                    // simply rejects more lessons (safe).
                                    let uuid = match crate::mojang::lookup_cached_uuid(sender) {
                                        Some(u) => u,
                                        None => return 0,
                                    };
                                    let file = memory::read_player(&uuid).ok().flatten().unwrap_or_default();
                                    let (interactions, distinct_days) =
                                        memory::count_interactions_for_uuid(history_dir, &uuid, &sender.to_lowercase(), 14);
                                    memory::compute_trust(&file, interactions, distinct_days, false)
                                };
                                let today_iso = chrono::Utc::now().format("%Y-%m-%d").to_string();
                                let adj = read_adjustments_or_empty();
                                let auto_result = reflection::run_pass(
                                    &api_key,
                                    &config.classifier_model,
                                    &pending,
                                    &adj,
                                    &trust_for_sender,
                                    &validator,
                                    &today_iso,
                                )
                                .await;
                                match auto_result {
                                    Ok(outcome) => {
                                        let usd = pricing.usd_for_call(
                                            &config.classifier_model,
                                            outcome.haiku_input_tokens,
                                            outcome.haiku_output_tokens,
                                            outcome.haiku_cache_creation_input_tokens,
                                            outcome.haiku_cache_read_input_tokens,
                                        );
                                        runtime_state.record_classifier(
                                            &state::capture_today_utc(),
                                            outcome.haiku_input_tokens,
                                            outcome.haiku_output_tokens,
                                            usd,
                                        );
                                        if let Err(e) = runtime_state.save() {
                                            warn!(error = %e, "[Chat] state save failed after auto reflection");
                                        }
                                        decisions::write(
                                            &decisions::DecisionRecord::new("reflection")
                                                .with_tokens(
                                                    outcome.haiku_input_tokens,
                                                    outcome.haiku_output_tokens,
                                                    usd,
                                                )
                                                .extra("trigger", serde_json::Value::from("auto"))
                                                .extra("admitted", serde_json::Value::from(outcome.admitted.len()))
                                                .extra("rejected_substring", serde_json::Value::from(outcome.rejected_substring))
                                                .extra("rejected_distinct_triggers", serde_json::Value::from(outcome.rejected_distinct_triggers))
                                                .extra("rejected_distinct_senders", serde_json::Value::from(outcome.rejected_distinct_senders))
                                                .extra("rejected_low_trust", serde_json::Value::from(outcome.rejected_low_trust)),
                                        );
                                    }
                                    Err(e) => {
                                        warn!(error = %e, "[Chat] auto reflection pass failed");
                                    }
                                }
                            }
                        }

                        // Persisted pause flag short-circuits everything.
                        if runtime_state.paused {
                            decisions::write(
                                &decisions::DecisionRecord::new("pre_filter_skip")
                                    .with_sender(&event.sender)
                                    .with_event_ts(event.recv_at)
                                    .with_reason("paused"),
                            );
                            continue;
                        }

                        // Process the live event, then drain any events
                        // accumulated during composer execution (CHAT.md
                        // concurrent-message policy). Priority order in the
                        // drain: most-recent direct-address > most-recent.
                        let process_result = process_event(
                            &event,
                            &api_key,
                            &config,
                            &pricing,
                            &mut runtime_state,
                            &bot_username,
                            &persona_body,
                            &persona_nicknames,
                            persona_lowercase_default,
                            &common_words,
                            &blocklist,
                            &system_senders_re,
                            &system_senders_exact,
                            &moderation_patterns,
                            &mut window,
                            &mut classifier_counter,
                            &mut spam_guard,
                            &mut recent_speakers,
                            &mut recent_bot_send_times,
                            &mut last_bot_send_at,
                            &in_critical_section,
                            &bot_tx,
                            &composer_limiter,
                            &classifier_limiter,
                        ).await;
                        if let Err(e) = process_result {
                            warn!(sender = %event.sender, error = %e, "[Chat] event processing error");
                        }

                        // Drain any backlog accumulated during the (slow)
                        // composer call. Pick by priority and process in
                        // priority order.
                        let mut backlog: Vec<ChatEvent> = Vec::new();
                        while let Ok(ev) = chat_events_rx.try_recv() {
                            backlog.push(ev);
                        }
                        // Sort by recency descending; stable so original
                        // arrival order is preserved within ties.
                        backlog.sort_by(|a, b| b.recv_at.cmp(&a.recv_at));
                        // Reorder so direct-addressed events come first
                        // (preserves recency order within the group).
                        backlog.sort_by(|a, b| {
                            let a_da = conversation::is_direct_address_with_common_words(
                                &a.content, &persona_nicknames, &common_words,
                            );
                            let b_da = conversation::is_direct_address_with_common_words(
                                &b.content, &persona_nicknames, &common_words,
                            );
                            b_da.cmp(&a_da)
                        });
                        for backlog_ev in backlog {
                            // Update window before processing — but skip
                            // system pseudo-senders so they don't pollute
                            // dyad detection (same reason as the live-event
                            // path above).
                            let bl_is_system = conversation::is_system_pseudo_sender(
                                &backlog_ev.sender,
                                &system_senders_re,
                                &system_senders_exact,
                            );
                            if !bl_is_system {
                                if window.len() == 8 {
                                    window.pop_front();
                                }
                                window.push_back(backlog_ev.clone());
                            }
                            let backlog_result = process_event(
                                &backlog_ev,
                                &api_key,
                                &config,
                                &pricing,
                                &mut runtime_state,
                                &bot_username,
                                &persona_body,
                                &persona_nicknames,
                                persona_lowercase_default,
                                &common_words,
                                &blocklist,
                                &system_senders_re,
                                &system_senders_exact,
                                &moderation_patterns,
                                &mut window,
                                &mut classifier_counter,
                                &mut spam_guard,
                                &mut recent_speakers,
                                &mut recent_bot_send_times,
                                &mut last_bot_send_at,
                                &in_critical_section,
                                &bot_tx,
                                &composer_limiter,
                                &classifier_limiter,
                            )
                            .await;
                            if let Err(e) = backlog_result {
                                warn!(sender = %backlog_ev.sender, error = %e, "[Chat] backlog event processing error");
                            }
                        }

                        // Prune `recent_speakers` so a long-running session
                        // doesn't accumulate every player who has ever
                        // spoken. Drop entries older than the
                        // `recent_speaker_secs` window.
                        let cutoff_speakers = Instant::now()
                            - Duration::from_secs(config.recent_speaker_secs as u64);
                        recent_speakers.retain(|_, t| *t >= cutoff_speakers);
                        // Prune the classifier per-sender counter the same
                        // way — its own `record_and_check` only prunes the
                        // entries it touches, so a sender who left the
                        // server still occupies a slot until pruned here.
                        // The spam guard has the same shape and the same
                        // leak; prune it on the same tick.
                        let now_prune = Instant::now();
                        classifier_counter.prune(now_prune);
                        spam_guard.prune(now_prune);

                        // Persist runtime state after each event so token
                        // counters survive a crash.
                        if let Err(e) = runtime_state.save() {
                            warn!(error = %e, "[Chat] state save failed after event");
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // CHAT.md: handle Lagged explicitly — durable
                        // history is on the publisher side, so a lag here
                        // only delays decision logic, not persistence.
                        warn!(lagged = n, "[Chat] broadcast lag (events dropped from decision pipeline; durable history unaffected)");
                        decisions::write(
                            &decisions::DecisionRecord::new("broadcast_lag")
                                .extra("lagged", serde_json::Value::from(n)),
                        );
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        info!("[Chat] event channel closed, exiting");
                        // Persona body is held only for read, suppress
                        // `unused mut` lint by binding-touch on shutdown.
                        let _ = &mut persona_body;
                        return;
                    }
                }
            }
        }
    }
}

/// Process one chat event end-to-end. Returns `Ok` on every recoverable
/// outcome (the event was either handled, skipped with a decision-log
/// entry, or dropped after a transient error logged here). `Err` is
/// reserved for state-corruption errors that the caller should surface.
#[allow(clippy::too_many_arguments)]
async fn process_event(
    event: &ChatEvent,
    api_key: &client::ApiKey,
    config: &ChatConfig,
    pricing_table: &pricing::PricingTable,
    runtime_state: &mut state::ChatState,
    bot_username: &Arc<RwLock<Option<String>>>,
    persona_body: &str,
    persona_nicknames: &[String],
    persona_lowercase_default: bool,
    common_words: &[String],
    blocklist: &std::collections::HashSet<String>,
    system_senders_re: &[String],
    system_senders_exact: &[String],
    moderation_patterns: &conversation::ModerationPatterns,
    window: &mut VecDeque<ChatEvent>,
    classifier_counter: &mut classifier::PerSenderCounter,
    spam_guard: &mut conversation::SpamGuard,
    recent_speakers: &mut HashMap<String, Instant>,
    recent_bot_send_times: &mut VecDeque<Instant>,
    last_bot_send_at: &mut Option<Instant>,
    in_critical_section: &Arc<AtomicBool>,
    bot_tx: &mpsc::Sender<BotInstruction>,
    composer_limiter: &client::RateLimiter,
    classifier_limiter: &client::RateLimiter,
) -> Result<(), String> {
    let now = Instant::now();

    // System pseudo-sender filter runs FIRST so server broadcasts (e.g.
    // a literal `"1"` sender on some anarchy proxies, `[Server]`,
    // `[CONSOLE]`) don't get spuriously skipped with `bot_username_unknown`
    // when they arrive in the post-disconnect/pre-Init window. These
    // events never produce a response anyway — they only need to feed
    // the moderation-pattern detector, which works on content alone.
    if conversation::is_system_pseudo_sender(&event.sender, system_senders_re, system_senders_exact) {
        if moderation_patterns.is_moderation_event(&event.content) {
            let until = state::iso_utc(
                chrono::Utc::now()
                    + chrono::Duration::seconds(config.moderation_backoff_secs as i64),
            );
            warn!(
                until = %until,
                trigger = %event.content,
                "[Chat] moderation event detected; entering long backoff"
            );
            runtime_state.moderation_backoff_until = Some(until);
            decisions::write(
                &decisions::DecisionRecord::new("moderation_backoff")
                    .with_sender(&event.sender)
                    .with_event_ts(event.recv_at)
                    .with_reason("moderation pattern matched"),
            );
        }
        return Ok(());
    }

    // Resolve bot's live username — refuse to act if unknown. Reaches
    // here only for events that passed the system-sender shape gate, so
    // every skip from this point onward represents a real player line
    // we can't safely classify without knowing our own name. Falls back
    // to `last_known_bot_username` (seeded from disk at startup) so
    // events arriving in the post-disconnect / pre-Init window — when
    // the live Arc has been cleared but the cache still holds the right
    // name — are processed instead of dropped. Init will overwrite the
    // Arc with the authoritative value once the handshake completes.
    let bot_name = match bot_username.read().await.clone() {
        Some(n) => n,
        None => match runtime_state.last_known_bot_username.clone() {
            Some(cached) => {
                info!(
                    cached_username = %cached,
                    sender = %event.sender,
                    "[Chat] live bot_username unknown; using cached identity for this event"
                );
                cached
            }
            None => {
                info!(
                    sender = %event.sender,
                    "[Chat] skipping event — bot username not yet known and no cached identity (waiting on Event::Init)"
                );
                decisions::write(
                    &decisions::DecisionRecord::new("pre_filter_skip")
                        .with_sender(&event.sender)
                        .with_event_ts(event.recv_at)
                        .with_reason("bot_username_unknown"),
                );
                return Ok(());
            }
        },
    };

    // self-echo guard.
    if event.sender.eq_ignore_ascii_case(&bot_name) {
        return Ok(());
    }

    // moderation backoff — silently observe while in backoff.
    if let Some(ref until) = runtime_state.moderation_backoff_until
        && let Ok(t) = chrono::DateTime::parse_from_rfc3339(until)
        && t.with_timezone(&chrono::Utc) > chrono::Utc::now()
    {
        decisions::write(
            &decisions::DecisionRecord::new("pre_filter_skip")
                .with_sender(&event.sender)
                .with_event_ts(event.recv_at)
                .with_reason("moderation_backoff_active"),
        );
        return Ok(());
    }

    // active-hours gate (public events only — DMs are always
    // answered when the bot is connected and the operator hasn't
    // paused).
    if event.kind == ChatEventKind::Public && !pacing::within_active_hours_now(config.active_hours_utc) {
        info!(
            sender = %event.sender,
            "[Chat] skipping public event — outside configured active hours"
        );
        decisions::write(
            &decisions::DecisionRecord::new("pre_filter_skip")
                .with_sender(&event.sender)
                .with_event_ts(event.recv_at)
                .with_reason("outside_active_hours"),
        );
        return Ok(());
    }

    // blocklist short-circuit — operator-managed allow-list of
    // names/UUIDs to ignore entirely. We don't have the sender's UUID
    // resolved at this point in the pipeline (resolution happens later,
    // when the composer is about to be called) so we pass `None`. UUID
    // entries in the blocklist still work for any sender we resolved
    // earlier; the username check is the common path.
    if conversation::SpamGuard::is_blocklisted(&event.sender.to_lowercase(), None, blocklist) {
        decisions::write(
            &decisions::DecisionRecord::new("pre_filter_skip")
                .with_sender(&event.sender)
                .with_event_ts(event.recv_at)
                .with_reason("blocklisted"),
        );
        return Ok(());
    }

    // spam guard. Record + check, all knobs from config now
    // (no more 5/30/300 hardcodes).
    let _ = spam_guard.record(
        event,
        config.spam_msgs_per_window,
        config.spam_window_secs,
        config.spam_cooldown_secs,
        now,
    );
    let spam_suppressed = spam_guard.is_suppressed(&event.sender, now);

    // direct-address detection — common-words downgrade enforced.
    // Whispers always count as direct contact: the player explicitly
    // DM'd the bot, so a bare "hi" should bypass dyad / lurk / silence
    // guards just like an `@bot ping` in public chat does. The candidate
    // names are the persona's chosen name and nicknames PLUS the bot's
    // live Minecraft username — players will use whichever they know.
    let mut address_names: Vec<String> = persona_nicknames.to_vec();
    if !address_names
        .iter()
        .any(|n| n.eq_ignore_ascii_case(&bot_name))
    {
        address_names.push(bot_name.clone());
    }
    let directly_addressed = event.kind == ChatEventKind::Whisper
        || conversation::is_direct_address_with_common_words(
            &event.content,
            &address_names,
            common_words,
        );

    // reply-to-other-speaker heuristic. If the message looks like
    // it's threaded at someone else (and the bot isn't the addressee),
    // stay silent unless directly addressed.
    if !directly_addressed {
        let recent_speaker_list: Vec<String> = window
            .iter()
            .rev()
            .map(|e| e.sender.clone())
            .filter(|s| !s.eq_ignore_ascii_case(&bot_name))
            .collect();
        if conversation::is_reply_to_other_speaker(
            &event.content,
            &bot_name,
            &recent_speaker_list,
            common_words,
        ) {
            info!(
                sender = %event.sender,
                "[Chat] skipping event — looks like reply to another player (not the bot)"
            );
            decisions::write(
                &decisions::DecisionRecord::new("pre_filter_skip")
                    .with_sender(&event.sender)
                    .with_event_ts(event.recv_at)
                    .with_reason("reply_to_other_speaker"),
            );
            return Ok(());
        }
    }

    // No hard dyad suppression — the classifier sees the recent window
    // and the persona's social-judgment guidance and decides whether to
    // chime in. A 1-on-1 between the bot and a player IS the bot's own
    // conversation; a 1-on-1 between two other players occasionally
    // deserves a chime-in when there's something worth contributing.

    // classifier gate. Passes the same combined address-name list the
    // pre-filter used so the gate's own direct-address check matches
    // the bot's live MC username AND the persona nicknames.
    let recent_speaker = recent_speakers
        .get(&event.sender)
        .is_some_and(|t| now.duration_since(*t).as_secs() < config.recent_speaker_secs as u64);
    let gate_verdict = classifier::classifier_gate(
        event,
        Some(&bot_name),
        &address_names,
        recent_speaker,
        spam_suppressed,
        config,
        classifier_counter,
        now,
        || rand_unit_f32(),
    );
    match gate_verdict {
        classifier::GateVerdict::Skip(reason) => {
            // Surface the skip reason at INFO so an operator watching
            // `store.log` can see why their message was ignored without
            // grepping the decisions JSONL. Pre-classifier skips are by
            // design (cost firewall) but are easy to mistake for a
            // broken pipeline when you're testing.
            info!(
                sender = %event.sender,
                kind = ?event.kind,
                reason = ?reason,
                "[Chat] event skipped before classifier dispatch"
            );
            decisions::write(
                &decisions::DecisionRecord::new("classifier_skip")
                    .with_sender(&event.sender)
                    .with_event_ts(event.recv_at)
                    .with_reason(format!("{reason:?}")),
            );
            return Ok(());
        }
        classifier::GateVerdict::Classify => {}
    }

    // Cap check before classifier dispatch.
    let estimated_classifier_input = 1500u64;
    let estimated_usd =
        pricing_table.usd_for_tokens(&config.classifier_model, estimated_classifier_input, 100);
    let cap_v =
        runtime_state.would_exceed_caps_classifier(estimated_classifier_input, 100, estimated_usd, config);
    if !matches!(cap_v, state::CapVerdict::Ok) {
        decisions::write(
            &decisions::DecisionRecord::new("cap_tripped")
                .with_sender(&event.sender)
                .with_event_ts(event.recv_at)
                .with_reason(format!("classifier {cap_v:?}")),
        );
        return Ok(());
    }

    // Classifier call.
    let started = Instant::now();
    let started_day = state::capture_today_utc();
    let history_slice = recent_history_slice_blocking(30).await;
    // Honor the runtime extended-cache flag — once the API has rejected
    // the beta header the static auto-demotes to 5 min, but we also want
    // request shapes to match so subsequent calls don't keep emitting a
    // `1h` ttl field that the client has to strip on the way out.
    let cache_ttl = if client::extended_cache_available() {
        client::CacheTtl::Ephemeral1Hour
    } else {
        client::CacheTtl::Ephemeral5Min
    };
    let classifier_req = classifier::build_request(
        &config.classifier_model,
        &persona_summary(persona_body),
        &read_adjustments_or_empty(),
        &history_slice,
        event,
        cache_ttl,
    );
    // Local rate limiter — burns the wait budget before the network call so
    // 429 spirals from the API never reach us. Token estimate is the same
    // input estimate used for the pre-flight cap check.
    if let Err(e) = classifier_limiter
        .acquire(estimated_classifier_input as u32)
        .await
    {
        decisions::write(
            &decisions::DecisionRecord::new("classifier_skip")
                .with_sender(&event.sender)
                .with_event_ts(event.recv_at)
                .with_reason(format!("local_rate_limit: {e}")),
        );
        return Ok(());
    }
    let resp = match client::call_with_retry(api_key, &classifier_req, true).await {
        Ok(r) => r,
        Err(client::ClientError::ModelNotFound { model }) => {
            // Engage the 1-hour self-disable so subsequent classifier
            // dispatches short-circuit rather than re-hitting the API.
            runtime_state.model_404_backoff_until =
                Some(client::model_404_backoff_until_now_plus_1h());
            decisions::write(
                &decisions::DecisionRecord::new("classifier_error")
                    .with_sender(&event.sender)
                    .with_event_ts(event.recv_at)
                    .with_reason(format!("model_not_found: {model}; engaging 1h backoff")),
            );
            return Ok(());
        }
        Err(e) => {
            decisions::write(
                &decisions::DecisionRecord::new("classifier_error")
                    .with_sender(&event.sender)
                    .with_event_ts(event.recv_at)
                    .with_reason(e.to_string()),
            );
            return Ok(());
        }
    };
    let usd = pricing_table.usd_for_call(
        &config.classifier_model,
        resp.usage.input_tokens,
        resp.usage.output_tokens,
        resp.usage.cache_creation_input_tokens,
        resp.usage.cache_read_input_tokens,
    );
    runtime_state.record_classifier(&started_day, resp.usage.input_tokens, resp.usage.output_tokens, usd);

    let mut text_buf = String::new();
    for b in &resp.content {
        if let client::ContentBlock::Text { text, .. } = b {
            text_buf.push_str(text);
        }
    }
    let verdict = match classifier::parse_verdict(&text_buf) {
        Ok(v) => v,
        Err(e) => {
            decisions::write(
                &decisions::DecisionRecord::new("classifier_decode_error")
                    .with_sender(&event.sender)
                    .with_event_ts(event.recv_at)
                    .with_reason(e),
            );
            return Ok(());
        }
    };
    decisions::write(
        &decisions::DecisionRecord::new("classifier")
            .with_sender(&event.sender)
            .with_event_ts(event.recv_at)
            .with_latency(started.elapsed().as_millis() as u64)
            .with_tokens(resp.usage.input_tokens, resp.usage.output_tokens, usd)
            .with_cache_tokens(
                resp.usage.cache_creation_input_tokens,
                resp.usage.cache_read_input_tokens,
            )
            .extra("respond", serde_json::Value::from(verdict.respond))
            .extra("confidence", serde_json::Value::from(verdict.confidence))
            .extra("reason", serde_json::Value::from(verdict.reason.clone()))
            .extra("urgency", serde_json::Value::from(verdict.urgency.clone())),
    );

    // AI call-out: write to pending_adjustments.jsonl, regardless of
    // respond decision.
    if let Some(ac) = &verdict.ai_callout
        && ac.detected
        && let Some(trigger) = ac.trigger.as_deref()
        && !trigger.is_empty()
    {
        classifier::write_pending_adjustment(trigger, &event.sender, None);
    }

    if !verdict.respond || verdict.confidence < config.classifier_min_confidence {
        return Ok(());
    }

    // / CON4 lurk skip — applied AFTER classifier said respond,
    // BEFORE composer dispatch. Bypassed for direct-address events
    // (CHAT.md explicitly: real players miss messages but always answer
    // when called by name).
    if !directly_addressed {
        let mut rng_unit = rand_unit_f32;
        if pacing::roll_lurk_skip(config.lurk_probability, &mut rng_unit) {
            decisions::write(
                &decisions::DecisionRecord::new("lurk_skip")
                    .with_sender(&event.sender)
                    .with_event_ts(event.recv_at)
                    .with_reason("post_classifier_lurk"),
            );
            return Ok(());
        }
    }

    // model-404 backoff — short-circuit composer if a recent 404
    // tripped the per-model self-disable.
    if client::is_model_404_backed_off(runtime_state.model_404_backoff_until.as_deref()) {
        decisions::write(
            &decisions::DecisionRecord::new("model_404_backoff")
                .with_sender(&event.sender)
                .with_event_ts(event.recv_at)
                .with_reason("composer_model_self_disabled"),
        );
        return Ok(());
    }

    // Composer throttle backoff — short-circuit if Anthropic recently
    // 429'd us through the in-call retry budget. Lazy clear: once the
    // timestamp is in the past we drop it and proceed.
    if let Some(ref until) = runtime_state.composer_throttle_backoff_until {
        match chrono::DateTime::parse_from_rfc3339(until) {
            Ok(t) if t.with_timezone(&chrono::Utc) > chrono::Utc::now() => {
                decisions::write(
                    &decisions::DecisionRecord::new("composer_throttle_backoff")
                        .with_sender(&event.sender)
                        .with_event_ts(event.recv_at)
                        .with_reason("upstream_429_recently"),
                );
                return Ok(());
            }
            _ => {
                runtime_state.composer_throttle_backoff_until = None;
            }
        }
    }

    // Cap check before composer dispatch.
    let estimated_composer_input = 4000u64;
    let estimated_composer_usd =
        pricing_table.usd_for_tokens(&config.composer_model, estimated_composer_input, 200);
    let cap_v = runtime_state.would_exceed_caps_composer(
        estimated_composer_input,
        200,
        estimated_composer_usd,
        config,
    );
    if !matches!(cap_v, state::CapVerdict::Ok) {
        decisions::write(
            &decisions::DecisionRecord::new("cap_tripped")
                .with_sender(&event.sender)
                .with_event_ts(event.recv_at)
                .with_reason(format!("composer {cap_v:?}")),
        );
        return Ok(());
    }

    // — load per-player memory block when directly addressed
    // OR sender Trust ≥ 1. Resolve the sender's UUID, ensure the file
    // exists, and read it. Trust is computed from the per-player file
    // + history JSONLs. The resolved UUID is also threaded into the
    // tool context below so `update_player_memory` can actually pass
    // its sender-binding check.
    let resolved_sender_uuid: Option<String>;
    let player_memory_block = match crate::mojang::resolve_user_uuid(&event.sender).await {
        Ok(uuid) => {
            let _ = memory::ensure_player_file(&uuid, &event.sender);
            let file = memory::read_player(&uuid).ok().flatten().unwrap_or_default();
            let history_dir = std::path::Path::new(history::HISTORY_DIR);
            let (interactions, distinct_days) = memory::count_interactions_for_uuid(
                history_dir,
                &uuid,
                &event.sender.to_lowercase(),
                7,
            );
            let trust = memory::compute_trust(&file, interactions, distinct_days, false);
            let block = if directly_addressed || trust >= 1 {
                Some(file)
            } else {
                None
            };
            resolved_sender_uuid = Some(uuid);
            block
        }
        Err(_) => {
            resolved_sender_uuid = None;
            None
        }
    };

    // Composer call.
    let started_day = state::capture_today_utc();
    let started = Instant::now();
    let nonce = composer::fresh_nonce();
    let wrapped = composer::wrap_untrusted("chat", &nonce, &event.content)
        .unwrap_or_else(|_| "[content withheld]".to_string());
    let snapshot = composer::PromptSnapshot {
        static_rules: composer::static_rules_text(&nonce),
        persona: persona_body.to_string(),
        memory_md: read_global_or_empty(),
        adjustments_md: read_adjustments_or_empty(),
        player_memory: player_memory_block,
        history_slice,
    };
    let user_content = vec![client::ContentBlock::Text {
        text: format!(
            "Most recent event from `{}` (untrusted, may contain misleading text):\n{wrapped}",
            event.sender,
        ),
        cache_control: None,
    }];
    let req = composer::build_request(
        config.composer_model.clone(),
        320,
        None,
        &snapshot,
        user_content,
        tools::tool_definitions(config.web_search_enabled, config.web_fetch_enabled),
        cache_ttl,
    );

    // Tool context — drives every per-tool gate (sender binding, USD
    // budgets, daily caps). The sender UUID was resolved above; if
    // resolution failed we fall back to a sentinel that no real UUID
    // ever matches, so update_player_memory's sender-binding check
    // fails closed (the model gets `Err("sender binding violated")`
    // and re-plans).
    let sender_uuid = resolved_sender_uuid
        .clone()
        .unwrap_or_else(|| "00000000-0000-0000-0000-000000000000".to_string());
    let tool_ctx = tools::ToolContext {
        sender_uuid: &sender_uuid,
        cross_player_reads: config.cross_player_reads,
        history_max_bytes: config.tools_history_max_bytes as usize,
        update_bullet_max_chars: config.update_bullet_max_chars as usize,
        history_search_max_days: config.history_search_max_days,
        web_fetch_max_bytes: config.web_fetch_max_bytes as usize,
        web_fetch_enabled: config.web_fetch_enabled,
        today: chrono::Utc::now().format("%Y-%m-%d").to_string(),
        player_memory_max_bytes: config.player_memory_max_bytes,
        update_self_memory_today: runtime_state.update_self_memory_today,
        update_self_memory_max_per_day: config.update_self_memory_max_per_day,
        memory_max_inferred_bullets: config.memory_max_inferred_bullets,
        web_fetches_today: runtime_state.web_fetches_today,
        web_fetch_daily_max: config.web_fetch_daily_max,
    };

    let run = match composer::run_loop(
        api_key,
        req,
        &tool_ctx,
        config.composer_max_tool_iterations,
        true,
        Some(composer_limiter),
        &nonce,
    )
    .await
    {
        Ok(r) => r,
        Err(e) => {
            // Composer surface for ModelNotFound is a string error wrapping
            // ClientError::ModelNotFound. Detect and engage the 1-hour
            // backoff so subsequent dispatches skip until the timer clears.
            if e.contains("model not found") {
                runtime_state.model_404_backoff_until =
                    Some(client::model_404_backoff_until_now_plus_1h());
            }
            // Anthropic-side 429 / 5xx that exhausted the retry budget:
            // pause composer dispatch for `composer_throttle_backoff_secs`
            // so the next event doesn't immediately re-race the same
            // throttled bucket. Detected via the unique "upstream-throttled"
            // marker emitted by `ClientError::Throttled`'s Display impl —
            // intentionally NOT a generic "throttled" substring, because
            // `ClientError::Transport(_)` (DNS / TLS / ECONNREFUSED) is
            // mapped to status=503 inside `call_with_retry` for retry
            // bookkeeping and would otherwise silently engage this 60s
            // cooldown on every flaky-network blip. See `client.rs`.
            if config.composer_throttle_backoff_secs > 0 && e.contains("upstream-throttled") {
                let until = state::iso_utc(
                    chrono::Utc::now()
                        + chrono::Duration::seconds(
                            config.composer_throttle_backoff_secs as i64,
                        ),
                );
                warn!(
                    until = %until,
                    error = %e,
                    "[Chat] composer hit upstream 429/5xx; pausing composer dispatch"
                );
                runtime_state.composer_throttle_backoff_until = Some(until);
            }
            decisions::write(
                &decisions::DecisionRecord::new("composer_error")
                    .with_sender(&event.sender)
                    .with_event_ts(event.recv_at)
                    .with_reason(e),
            );
            return Ok(());
        }
    };
    let composer_usd = pricing_table.usd_for_call(
        &config.composer_model,
        run.input_tokens,
        run.output_tokens,
        run.cache_creation_input_tokens,
        run.cache_read_input_tokens,
    );
    runtime_state.record_composer(&started_day, run.input_tokens, run.output_tokens, composer_usd);

    // Update Chat: status surfacing of the latest call.
    runtime_state.last_composer_call = Some(state::LastCallSummary {
        at_utc: state::iso_utc(chrono::Utc::now()),
        usd: composer_usd,
        input_tokens: run.input_tokens,
        output_tokens: run.output_tokens,
    });

    // Tool-call counter increments. The composer's tool-use loop ran
    // some number of `update_self_memory` and `web_fetch` calls; the
    // tool layer doesn't mutate state.json directly so the orchestrator
    // sums them after the fact. `run` carries the counts.
    if run.update_self_memory_calls > 0 {
        runtime_state.update_self_memory_today =
            runtime_state.update_self_memory_today.saturating_add(run.update_self_memory_calls);
    }
    if run.web_fetch_calls > 0 {
        runtime_state.web_fetches_today =
            runtime_state.web_fetches_today.saturating_add(run.web_fetch_calls);
    }

    decisions::write(
        &decisions::DecisionRecord::new("composer")
            .with_sender(&event.sender)
            .with_event_ts(event.recv_at)
            .with_latency(started.elapsed().as_millis() as u64)
            .with_tokens(run.input_tokens, run.output_tokens, composer_usd)
            .with_cache_tokens(run.cache_creation_input_tokens, run.cache_read_input_tokens)
            .extra("iterations", serde_json::Value::from(run.iterations))
            .extra("hit_cap", serde_json::Value::from(run.hit_cap))
            .extra("had_text_reply", serde_json::Value::from(run.reply.is_some())),
    );

    let Some(reply) = run.reply else {
        return Ok(());
    };
    let reply = pacing::strip_ai_tells(&reply);
    // Apply persona-driven capitalization habit (CHAT.md): a persona that
    // self-describes as lowercase-by-default lowercases every
    // sentence-initial alphabetic character. Mid-sentence proper nouns are
    // preserved.
    let reply = if persona_lowercase_default {
        pacing::lowercase_first_per_sentence(&reply)
    } else {
        reply
    };
    let reply = pacing::truncate_to_chat_limit(&reply, config.composer_max_chars as usize);
    if reply.trim().is_empty() {
        return Ok(());
    }

    // Pacing — typing delay then post-sleep recheck. Use the proper
    // Box-Muller Gaussian; every pacing knob now config-driven.
    let mut rng_unit = rand_unit_f32;
    let jitter_ms = pacing::gaussian_jitter_ms(0, config.typing_delay_jitter_ms, &mut rng_unit);
    let delay = pacing::compute_typing_delay(
        reply.chars().count(),
        config.typing_delay_base_ms,
        config.typing_delay_per_char_ms,
        jitter_ms,
        config.typing_delay_floor_ms,
        config.typing_delay_max_ms,
    );
    tokio::time::sleep(Duration::from_millis(delay as u64)).await;

    // Recompute window-bound counters using a fresh `Instant::now()` —
    // the `now` captured at the top of `process_event` is stale by the
    // typing-delay sleep we just awaited, which would over-count
    // recent_bot_send_times (and over-throttle the bot relative to the
    // configured cap).
    let now_post_sleep = Instant::now();
    let cutoff = now_post_sleep - Duration::from_secs(60);
    while let Some(&t) = recent_bot_send_times.front() {
        if t < cutoff {
            recent_bot_send_times.pop_front();
        } else {
            break;
        }
    }
    let secs_since_last = last_bot_send_at.map(|t| now_post_sleep.duration_since(t).as_secs());

    let decision = pacing::recheck_after_sleep(
        directly_addressed,
        in_critical_section.load(Ordering::Acquire),
        event.kind == ChatEventKind::Public,
        recent_bot_send_times.len() as u32,
        config.max_replies_per_minute,
        secs_since_last,
        config.min_silence_secs,
    );
    match decision {
        pacing::SendDecision::Send => {}
        other => {
            decisions::write(
                &decisions::DecisionRecord::new("pacing_drop")
                    .with_sender(&event.sender)
                    .with_event_ts(event.recv_at)
                    .with_reason(format!("{other:?}")),
            );
            return Ok(());
        }
    }

    // Honor dry-run.
    let dry = config.dry_run || runtime_state.dry_run_runtime_override;
    if dry {
        decisions::write(
            &decisions::DecisionRecord::new("dry_run")
                .with_sender(&event.sender)
                .with_event_ts(event.recv_at)
                .extra("would_send", serde_json::Value::from(reply.clone())),
        );
        return Ok(());
    }

    // Send via SendChat (public) or Whisper.
    let (resp_tx, resp_rx) = oneshot::channel();
    let send_msg = if event.kind == ChatEventKind::Whisper {
        BotInstruction::Whisper {
            target: event.sender.clone(),
            message: reply.clone(),
            respond_to: resp_tx,
        }
    } else {
        BotInstruction::SendChat {
            content: reply.clone(),
            respond_to: resp_tx,
        }
    };
    if bot_tx.send(send_msg).await.is_err() {
        warn!("[Chat] bot channel closed before send");
        return Ok(());
    }
    match resp_rx.await {
        Ok(Ok(())) => {
            let sent_at = Instant::now();
            recent_bot_send_times.push_back(sent_at);
            *last_bot_send_at = Some(sent_at);
            recent_speakers.insert(event.sender.clone(), sent_at);
            decisions::write(
                &decisions::DecisionRecord::new("sent")
                    .with_sender(&event.sender)
                    .with_event_ts(event.recv_at)
                    .extra("reply_len", serde_json::Value::from(reply.len())),
            );
        }
        Ok(Err(e)) => {
            warn!(error = %e, "[Chat] bot send failed");
            decisions::write(
                &decisions::DecisionRecord::new("send_error")
                    .with_sender(&event.sender)
                    .with_event_ts(event.recv_at)
                    .with_reason(e),
            );
        }
        Err(_) => {
            warn!("[Chat] send response channel dropped");
        }
    }
    Ok(())
}

/// Stats returned by [`scrub_history_for_player`] for the operator log.
#[derive(Debug, Clone, Copy, Default)]
struct ForgetScrubStats {
    history_lines: u64,
    decisions_lines: u64,
    overlay_entries: u64,
}

/// Walk every JSONL under `data/chat/history/` and `data/chat/decisions/`,
/// dropping records whose `sender`/`target`/`uuid`/`target_uuid` field
/// matches the forgotten player. Also strips `<date>.uuids.json` overlay
/// entries keyed on the same UUID. Each affected file is rewritten via
/// [`crate::fsutil::write_atomic`] so a crash mid-scrub leaves either
/// the original or the scrubbed file, never a torn one.
fn scrub_history_for_player(uuid: &str, username: &str) -> Result<ForgetScrubStats, String> {
    let mut stats = ForgetScrubStats::default();
    let username_lc = username.to_lowercase();
    let uuid_lc = uuid.to_lowercase();
    for (dir, kind) in [
        (history::HISTORY_DIR, "history"),
        (decisions::DECISIONS_DIR, "decisions"),
    ] {
        let path = std::path::Path::new(dir);
        if !path.exists() {
            continue;
        }
        let entries = std::fs::read_dir(path)
            .map_err(|e| format!("read_dir({dir}): {e}"))?;
        for ent in entries.flatten() {
            let p = ent.path();
            if !p.is_file() {
                continue;
            }
            let name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.ends_with(".uuids.json") {
                // Sidecar overlay — load JSON object, drop keys/values
                // matching the UUID.
                if let Ok(body) = std::fs::read_to_string(&p)
                    && let Ok(mut v) =
                        serde_json::from_str::<serde_json::Value>(&body)
                    && let Some(obj) = v.as_object_mut()
                {
                    let before = obj.len();
                    obj.retain(|_, val| {
                        val.as_str().is_none_or(|s| !s.eq_ignore_ascii_case(&uuid_lc))
                    });
                    let removed = before.saturating_sub(obj.len()) as u64;
                    if removed > 0 {
                        let new = serde_json::to_string_pretty(&v)
                            .map_err(|e| format!("overlay re-serialize: {e}"))?;
                        crate::fsutil::write_atomic(&p, &new)
                            .map_err(|e| format!("overlay rewrite: {e}"))?;
                        stats.overlay_entries =
                            stats.overlay_entries.saturating_add(removed);
                    }
                }
                continue;
            }
            if !name.ends_with(".jsonl") {
                continue;
            }
            let body = match std::fs::read_to_string(&p) {
                Ok(b) => b,
                Err(_) => continue,
            };
            let mut kept = String::with_capacity(body.len());
            let mut removed = 0u64;
            for line in body.lines() {
                let drop_it = match serde_json::from_str::<serde_json::Value>(line) {
                    Ok(v) => record_matches_player(&v, &uuid_lc, &username_lc),
                    Err(_) => false,
                };
                if drop_it {
                    removed = removed.saturating_add(1);
                    continue;
                }
                kept.push_str(line);
                kept.push('\n');
            }
            if removed > 0 {
                crate::fsutil::write_atomic(&p, &kept)
                    .map_err(|e| format!("rewrite {kind} {}: {e}", p.display()))?;
                if kind == "history" {
                    stats.history_lines = stats.history_lines.saturating_add(removed);
                } else {
                    stats.decisions_lines =
                        stats.decisions_lines.saturating_add(removed);
                }
            }
        }
    }
    Ok(stats)
}

fn record_matches_player(v: &serde_json::Value, uuid_lc: &str, username_lc: &str) -> bool {
    let str_field = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
    str_field("uuid").eq_ignore_ascii_case(uuid_lc)
        || str_field("target_uuid").eq_ignore_ascii_case(uuid_lc)
        || str_field("sender_uuid").eq_ignore_ascii_case(uuid_lc)
        || str_field("sender").eq_ignore_ascii_case(username_lc)
        || str_field("target").eq_ignore_ascii_case(username_lc)
}

/// Replay the system prompt that WOULD have been sent for a historical
/// event timestamp. Local-only — no API call. Searches today's and the
/// preceding day's history JSONL for a record whose `ts` field matches
/// `event_ts` (string-equal, RFC3339), reconstructs a [`PromptSnapshot`]
/// from the live persona / memory / adjustments, and renders the system
/// prompt that the composer would have built.
async fn build_replay_prompt(
    event_ts: &str,
    persona_body: &str,
) -> Result<String, String> {
    // Walk recent history files newest-first looking for the record.
    let event_ts_owned = event_ts.to_string();
    let found_line = tokio::task::spawn_blocking(move || -> Option<String> {
        for d in 0..7i64 {
            let day = (chrono::Utc::now() - chrono::Duration::days(d))
                .format("%Y-%m-%d")
                .to_string();
            let path = std::path::Path::new(history::HISTORY_DIR)
                .join(format!("{day}.jsonl"));
            let body = match std::fs::read_to_string(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            for line in body.lines() {
                let v: serde_json::Value = match serde_json::from_str(line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let ts = v.get("ts").and_then(|x| x.as_str()).unwrap_or("");
                if ts == event_ts_owned {
                    return Some(line.to_string());
                }
            }
        }
        None
    })
    .await
    .map_err(|e| format!("history scan join: {e}"))?;
    let line = found_line.ok_or_else(|| {
        format!("no history record found for ts={event_ts}")
    })?;
    let v: serde_json::Value =
        serde_json::from_str(&line).map_err(|e| format!("history line parse: {e}"))?;
    let sender = v
        .get("sender")
        .and_then(|x| x.as_str())
        .unwrap_or("<unknown>")
        .to_string();
    let content = v
        .get("content")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let nonce = composer::fresh_nonce();
    let snapshot = composer::PromptSnapshot {
        static_rules: composer::static_rules_text(&nonce),
        persona: persona_body.to_string(),
        memory_md: read_global_or_empty(),
        adjustments_md: read_adjustments_or_empty(),
        player_memory: None,
        history_slice: recent_history_slice_blocking(30).await,
    };
    let mut out = String::new();
    out.push_str("=== REPLAY ===\n");
    out.push_str(&format!("event_ts: {event_ts}\nsender: {sender}\ncontent: {content}\n"));
    out.push_str("\n--- system blocks ---\n\n");
    for (i, block) in [
        ("static_rules", snapshot.static_rules.as_str()),
        ("persona", snapshot.persona.as_str()),
        ("memory_md", snapshot.memory_md.as_str()),
        ("adjustments_md", snapshot.adjustments_md.as_str()),
        ("history_slice", snapshot.history_slice.as_str()),
    ]
    .iter()
    .enumerate()
    {
        out.push_str(&format!("[block {} — {}]\n{}\n\n", i, block.0, block.1));
    }
    Ok(out)
}

/// Append a single record to `data/chat/operator_audit.jsonl`. Used by
/// Trust-3 toggles and `forget_player` so a later compliance review can
/// confirm what the operator did and why. Best-effort — failures are
/// logged but never bubble up to the CLI.
fn write_operator_audit(action: &str, username: &str, uuid: &str, reason: &str) {
    use std::io::Write;
    let entry = serde_json::json!({
        "ts": state::iso_utc(chrono::Utc::now()),
        "action": action,
        "username": username,
        "uuid": uuid,
        "reason": reason,
    });
    let path = std::path::Path::new(memory::CHAT_DIR).join("operator_audit.jsonl");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let line = match serde_json::to_string(&entry) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "[Chat] operator_audit serialize failed");
            return;
        }
    };
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        Ok(mut f) => {
            if let Err(e) = writeln!(f, "{line}") {
                warn!(error = %e, path = %path.display(), "[Chat] operator_audit append failed");
            }
        }
        Err(e) => warn!(error = %e, path = %path.display(), "[Chat] operator_audit open failed"),
    }
}

/// Resolve `username` → `uuid` via the local `_index.json` first, falling
/// back to a Mojang lookup if the index is cold. Avoids burning the
/// Mojang rate limit when an operator runs CLI commands against players
/// the bot already knows. Best-effort — index-load failures fall through
/// to the network call rather than aborting.
///
/// Defense-in-depth: every returned UUID is shape-validated. The CLI
/// commands that act on the result (`reset_player_memory`,
/// `forget_player`, etc.) build paths via `memory::player_file_path`
/// which does NOT canonicalize, so a malformed UUID slipped into the
/// index could in principle escape the players dir. Validating here
/// keeps every CLI caller honest without each having to remember.
async fn resolve_username_via_index_or_mojang(username: &str) -> Result<String, String> {
    let uuid = if let Ok(idx) = memory::load_or_rebuild_index()
        && let Some(u) = idx.lookup(username)
    {
        u.to_string()
    } else {
        crate::mojang::resolve_user_uuid(username).await?
    };
    crate::chat::tools::validate_uuid(&uuid).map_err(|e| {
        format!("resolved uuid for {username:?} failed shape check: {e}")
    })?;
    Ok(uuid)
}

/// Cheap deterministic-process-RNG returning a value in [0.0, 1.0).
/// Mixes the same monotonic counter and time used by `composer::fresh_nonce`.
fn rand_unit_f32() -> f32 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mixed = (t.rotate_left(13) ^ n.wrapping_mul(0x9E37_79B9_7F4A_7C15)).wrapping_add(n);
    // Bottom 24 bits → fraction.
    let bits = (mixed & 0xFF_FFFF) as u32;
    bits as f32 / (1u32 << 24) as f32
}

fn persona_summary(persona_body: &str) -> String {
    // First ~500 chars are an OK approximation of a "summary". The
    // reflection model is stricter about needing the full persona, so
    // composer uses the full body; classifier needs less context.
    let chars = persona_body.chars().take(500).collect::<String>();
    chars
}

fn read_global_or_empty() -> String {
    memory::read_global_memory().unwrap_or_default()
}

fn read_adjustments_or_empty() -> String {
    memory::read_adjustments().unwrap_or_default()
}

/// Read the trailing `n` lines of today's history JSONL. Returns
/// empty on missing file.
async fn recent_history_slice_blocking(n: usize) -> String {
    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    tokio::task::spawn_blocking(move || {
        let p = std::path::Path::new(history::HISTORY_DIR)
            .join(format!("{today}.jsonl"));
        let body = std::fs::read_to_string(&p).unwrap_or_default();
        let mut lines: Vec<&str> = body.lines().collect();
        if lines.len() > n {
            lines = lines.split_off(lines.len() - n);
        }
        lines.join("\n")
    })
    .await
    .unwrap_or_default()
}

/// History writer task — re-exported from [`history::writer_task`].
///
/// `main.rs` spawns this; the real implementation lives in [`history`].
pub use history::writer_task as history_writer_task;
