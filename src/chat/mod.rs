//! # Chat module — natural-language chat AI
//!
//! Disabled by default behind `chat.enabled = false`. See `PLAN.md` for the
//! full design. This Phase 1 skeleton wires up:
//! - [`ChatEvent`](crate::messages::ChatEvent) broadcast subscription.
//! - [`ChatCommand`](crate::messages::ChatCommand) command channel.
//! - The whisper router ([`conversation::route_whisper`]) used by the bot
//!   layer to split inbound whispers between Store and Chat.
//!
//! The actual Anthropic API client, classifier, composer, persona, memory,
//! and tool-use loop are stubbed out and arrive in later phases.

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

/// Snapshot returned by `Chat: status` (PLAN §10 OPS3). Operator-facing
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
    info!(
        bot_persona = %persona_name,
        nicknames = ?persona_nicknames,
        "[Chat] persona loaded"
    );

    // Load runtime state (token meter, pause flag, backoff timers).
    let mut runtime_state = match state::ChatState::load_or_default() {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "[Chat] state.json load failed, using defaults");
            state::ChatState::default()
        }
    };

    // Run the retention sweep on startup (PLAN §11).
    if let Some(today) = retention::sweep_due_today(None) {
        let cfg = retention::SweepConfig {
            chat_dir: PathBuf::from(memory::CHAT_DIR),
            history_retention_days: 30,
            decisions_retention_days: 30,
            persona_archive_max: 10,
            today: chrono::Utc::now(),
        };
        let report = retention::run_sweep(&cfg);
        info!(
            today = %today,
            deleted = report.total(),
            "[Chat] startup retention sweep complete"
        );
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
                        };
                        let _ = respond_to.send(report);
                    }
                    Some(ChatCommand::SetPaused { paused, respond_to }) => {
                        runtime_state.paused = paused;
                        info!(paused, "[Chat] pause flag updated");
                        if let Err(e) = runtime_state.save() {
                            warn!(error = %e, "[Chat] state save failed after SetPaused");
                        }
                        let _ = respond_to.send(());
                    }
                    Some(ChatCommand::SetDryRun { dry_run, respond_to }) => {
                        runtime_state.dry_run_runtime_override = dry_run;
                        info!(dry_run, "[Chat] dry-run override updated");
                        if let Err(e) = runtime_state.save() {
                            warn!(error = %e, "[Chat] state save failed after SetDryRun");
                        }
                        let _ = respond_to.send(());
                    }
                    Some(ChatCommand::ClearModerationBackoff { respond_to }) => {
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
                        // runs only — see PLAN §4.7).
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
                            let usd = pricing.usd_for_tokens(
                                &config.classifier_model,
                                outcome.haiku_input_tokens,
                                outcome.haiku_output_tokens,
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
                        // Maintain channel sliding window.
                        if window.len() == 8 {
                            window.pop_front();
                        }
                        window.push_back(event.clone());

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

                        // Refresh persona in case the operator hand-edited.
                        // Cheap (single fs::read on every event would be
                        // wasteful — only refresh once a minute).
                        // Skipping for simplicity — persona is loaded at
                        // startup and on regenerate-persona CLI command.

                        // Process the event end-to-end.
                        let process_result = process_event(
                            &event,
                            &api_key,
                            &config,
                            &pricing,
                            &mut runtime_state,
                            &bot_username,
                            &persona_body,
                            &persona_nicknames,
                            &mut window,
                            &mut classifier_counter,
                            &mut spam_guard,
                            &mut recent_speakers,
                            &mut recent_bot_send_times,
                            &mut last_bot_send_at,
                            &in_critical_section,
                            &bot_tx,
                        ).await;
                        if let Err(e) = process_result {
                            warn!(sender = %event.sender, error = %e, "[Chat] event processing error");
                        }
                        // Persist runtime state after each event so token
                        // counters survive a crash.
                        if let Err(e) = runtime_state.save() {
                            warn!(error = %e, "[Chat] state save failed after event");
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // PLAN §2.2: handle Lagged explicitly — durable
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
    window: &mut VecDeque<ChatEvent>,
    classifier_counter: &mut classifier::PerSenderCounter,
    spam_guard: &mut conversation::SpamGuard,
    recent_speakers: &mut HashMap<String, Instant>,
    recent_bot_send_times: &mut VecDeque<Instant>,
    last_bot_send_at: &mut Option<Instant>,
    in_critical_section: &Arc<AtomicBool>,
    bot_tx: &mpsc::Sender<BotInstruction>,
) -> Result<(), String> {
    let now = Instant::now();
    // Resolve bot's live username — refuse to act if unknown (PLAN §2.4).
    let bot_name = bot_username.read().await.clone();
    let Some(bot_name) = bot_name else {
        decisions::write(
            &decisions::DecisionRecord::new("pre_filter_skip")
                .with_sender(&event.sender)
                .with_event_ts(event.recv_at)
                .with_reason("bot_username_unknown"),
        );
        return Ok(());
    };

    // §4.1 self-echo guard.
    if event.sender.eq_ignore_ascii_case(&bot_name) {
        return Ok(());
    }

    // §4.5 spam guard. Record + check.
    let spam_default_msgs = 5u32;
    let spam_default_window = 30u32;
    let spam_default_cooldown = 300u32;
    let _ = spam_guard.record(
        event,
        spam_default_msgs,
        spam_default_window,
        spam_default_cooldown,
        now,
    );
    let spam_suppressed = spam_guard.is_suppressed(&event.sender, now);

    // §4.4 direct-address detection (using persona name + nicknames).
    let directly_addressed = classifier::is_direct_address(&event.content, persona_nicknames);

    // §4.4 dyad suppression: if the channel is currently dominated by
    // two non-bot speakers, stay silent unless directly addressed.
    let window_slice: Vec<ChatEvent> = window.iter().cloned().collect();
    let class = conversation::classify_window(&window_slice);
    if !directly_addressed && matches!(class, conversation::ChannelClass::Dyad { .. }) {
        decisions::write(
            &decisions::DecisionRecord::new("pre_filter_skip")
                .with_sender(&event.sender)
                .with_event_ts(event.recv_at)
                .with_reason("dyad_active"),
        );
        return Ok(());
    }

    // §4.2 classifier gate.
    let recent_speaker = recent_speakers
        .get(&event.sender)
        .is_some_and(|t| now.duration_since(*t).as_secs() < config.recent_speaker_secs as u64);
    let gate_verdict = classifier::classifier_gate(
        event,
        Some(&bot_name),
        persona_nicknames,
        recent_speaker,
        spam_suppressed,
        config,
        classifier_counter,
        now,
        || rand_unit_f32(),
    );
    match gate_verdict {
        classifier::GateVerdict::Skip(reason) => {
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
    let classifier_req = classifier::build_request(
        &config.classifier_model,
        &persona_summary(persona_body),
        &read_adjustments_or_empty(),
        &history_slice,
        event,
        client::CacheTtl::Ephemeral1Hour,
    );
    let resp = match client::call_with_retry(api_key, &classifier_req, true).await {
        Ok(r) => r,
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
    let usd =
        pricing_table.usd_for_tokens(&config.classifier_model, resp.usage.input_tokens, resp.usage.output_tokens);
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
            .extra("respond", serde_json::Value::from(verdict.respond))
            .extra("confidence", serde_json::Value::from(verdict.confidence))
            .extra("reason", serde_json::Value::from(verdict.reason.clone()))
            .extra("urgency", serde_json::Value::from(verdict.urgency.clone())),
    );

    // AI call-out: write to pending_adjustments.jsonl, regardless of
    // respond decision.
    if let Some(ac) = &verdict.ai_callout
        && ac.detected
        && !ac.trigger.is_empty()
    {
        classifier::write_pending_adjustment(&ac.trigger, &event.sender, None);
    }

    if !verdict.respond || verdict.confidence < config.classifier_min_confidence {
        return Ok(());
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
        player_memory: None, // Phase: load when sender memory exists / Trust ≥ 1
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
        Some(0.85),
        &snapshot,
        user_content,
        tools::tool_definitions(false, false),
        client::CacheTtl::Ephemeral1Hour,
    );

    // Tool context — sender UUID is unknown for now (lazy resolve);
    // pass a placeholder that prevents per-player writes when not
    // resolvable.
    let sender_uuid = event.sender.clone(); // placeholder; tools that
                                            // need real UUID will fail
                                            // sender-binding (intended)
    let tool_ctx = tools::ToolContext {
        sender_uuid: &sender_uuid,
        cross_player_reads: false,
        history_max_bytes: 32 * 1024,
        update_bullet_max_chars: 280,
        history_search_max_days: 14,
        web_fetch_max_bytes: 256 * 1024,
        web_fetch_enabled: false,
        today: chrono::Utc::now().format("%Y-%m-%d").to_string(),
    };

    let run = match composer::run_loop(api_key, req, &tool_ctx, 5, true).await {
        Ok(r) => r,
        Err(e) => {
            decisions::write(
                &decisions::DecisionRecord::new("composer_error")
                    .with_sender(&event.sender)
                    .with_event_ts(event.recv_at)
                    .with_reason(e),
            );
            return Ok(());
        }
    };
    let composer_usd =
        pricing_table.usd_for_tokens(&config.composer_model, run.input_tokens, run.output_tokens);
    runtime_state.record_composer(&started_day, run.input_tokens, run.output_tokens, composer_usd);

    decisions::write(
        &decisions::DecisionRecord::new("composer")
            .with_sender(&event.sender)
            .with_event_ts(event.recv_at)
            .with_latency(started.elapsed().as_millis() as u64)
            .with_tokens(run.input_tokens, run.output_tokens, composer_usd)
            .extra("iterations", serde_json::Value::from(run.iterations))
            .extra("hit_cap", serde_json::Value::from(run.hit_cap))
            .extra("had_text_reply", serde_json::Value::from(run.reply.is_some())),
    );

    let Some(reply) = run.reply else {
        return Ok(());
    };
    let reply = pacing::strip_ai_tells(&reply);
    let reply = pacing::truncate_to_chat_limit(&reply, 240);
    if reply.trim().is_empty() {
        return Ok(());
    }

    // Pacing — typing delay then post-sleep recheck.
    // Single uniform draw for jitter; map to a Gaussian via rough
    // approximation (sum of two uniforms minus 1, scaled). Cheap
    // and deps-free.
    let unif = rand_unit_f32() - 0.5;
    let unif2 = rand_unit_f32() - 0.5;
    let jitter_ms = ((unif + unif2) * 250.0) as i32;
    let delay = pacing::compute_typing_delay(reply.len(), 800, 60, jitter_ms, 400, 12_000);
    tokio::time::sleep(Duration::from_millis(delay as u64)).await;

    // Recompute window-bound counters.
    let cutoff = now - Duration::from_secs(60);
    while let Some(&t) = recent_bot_send_times.front() {
        if t < cutoff {
            recent_bot_send_times.pop_front();
        } else {
            break;
        }
    }
    let secs_since_last = last_bot_send_at.map(|t| Instant::now().duration_since(t).as_secs());

    let decision = pacing::recheck_after_sleep(
        directly_addressed,
        in_critical_section.load(Ordering::Acquire),
        event.kind == ChatEventKind::Public,
        recent_bot_send_times.len() as u32,
        4,
        secs_since_last,
        6,
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
            recent_bot_send_times.push_back(Instant::now());
            *last_bot_send_at = Some(Instant::now());
            recent_speakers.insert(event.sender.clone(), Instant::now());
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
