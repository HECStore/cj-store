# cj-store — Chat AI Module

Reference for the natural-language chat module — a fourth top-level task,
peer to `store`, `bot`, and `cli`. Disabled by default behind
`chat.enabled = false`. For the trade-bot side see
[ARCHITECTURE.md](ARCHITECTURE.md); for whisper command parsing see
[COMMANDS.md](COMMANDS.md); for on-disk JSON shapes see
[DATA_SCHEMA.md](DATA_SCHEMA.md); for operator runbooks see
[RECOVERY.md](RECOVERY.md).

The chat module is the bot's conversational layer — an openly-AI
chatbot that plays Minecraft as a store-running player. It observes
in-game chat (public + whisper) and produces friendly, in-persona
replies via Anthropic Claude — Sonnet 4.6 for composition, Haiku 4.5
for cheap classification. The goal is helpful conversational presence:
greetings, banter, answering questions about the shop or the server,
helping players when they ask.
It maintains durable per-player and global memory across restarts,
learns from style feedback, and paces its output so it reads like a
Minecraft player chatting rather than a flood of formal-assistant prose.
The bot is *not* trying to pass as human — if asked, it answers
honestly that it is an AI.

## Hard architectural invariants

> [!CAUTION]
> The chat module **never has a handle into Store state**. It cannot read
> balances, pending orders, trade history, or pair reserves. The
> `chat_task` signature carries only `bot_tx`, channels, an
> `Arc<AtomicBool>` "busy" flag, and the bot's username — nothing more.
> A chat reply that mentions a balance or an order is a privacy leak —
> the bot has no business confirming, in public chat or even in DMs,
> what another player owns or has traded. Preserve this gap when
> extending.

Companion invariants:

- **The chat AI never queues orders.** All trade actions go through
  whisper commands handled by Store.
- **Whispers are routed once, at the bot layer** ([§ Whisper
  routing](#whisper-routing)) — so a freeform DM never reaches Store and
  gets `"Unknown command"` back while also being answered by chat.
- **`persona.md` is operator-managed.** No tool exposed to the composer
  can write it. The persona is a style guide (voice, tempo, slang) that
  defines the bot's character; an LLM-writable persona would drift
  into inconsistent, off-tone replies and erode trust with regulars.

## Module shape

```text
src/chat/
  mod.rs            chat_task entry point + per-event orchestration
  client.rs         Anthropic API client (reqwest, prompt caching, retry, rate limits)
  classifier.rs     Haiku-based "should I respond?" pre-filter
  composer.rs       Sonnet 4.6 reply generator + tool-use loop
  tools.rs          Tool handlers (memory r/w, history search, web fetch/search)
  memory.rs         Global + per-player memory file I/O, trust derivation
  history.rs        Daily JSONL chat log: writer task, search
  conversation.rs   Whisper router, direct-address detection, spam guard
  persona.rs        Persona generation + load (read-only at runtime)
  pacing.rs         Typing delay, active hours, AI-tells stripping
  state.rs          Runtime state file (token meter, backoffs, last-replied)
  reflection.rs     AI call-out reflection pass (validates lessons before commit)
  decisions.rs      Daily decision-log JSONL writer
  retention.rs      Daily sweep: prune old history/decisions/archives
  pricing.rs        Per-model price table (loaded from data/chat/pricing.json)
  web.rs            web_fetch SSRF defenses + streaming size enforcement
```

`chat_task` runs as a regular `tokio::spawn` (Send-friendly) — unlike
`bot_task` it never touches Azalea internals, so it does not need a
`LocalSet`.

## Runtime topology

The chat module lives next to Bot, Store, and CLI. Channels are owned by
[main.rs](src/main.rs); clones are distributed to the tasks that need them.

```text
                     ┌─────────────────────────┐
                     │      Bot task           │
                     │   (parses chat lines)   │
                     └──────┬───────────┬──────┘
                            │           │
                broadcast::Sender   mpsc::Sender
                <ChatEvent>         <ChatEvent>
                            │           │
                            │           ▼
                            │    ┌──────────────────────┐
                            │    │ history-writer task  │
                            │    │ (durable JSONL)      │
                            │    └──────────────────────┘
                            ▼
                    ┌─────────────────┐         ┌──────────────────┐
                    │   chat_task     │ ──────▶ │  bot_tx (shared) │
                    │  (decisions,    │ SendChat│  with Store      │
                    │   compose,      │ Whisper │                  │
                    │   pacing)       │         └──────────────────┘
                    └────────┬────────┘
                             ▲
                             │ ChatCommand
                             │
                       ┌──────────────┐
                       │   CLI task   │
                       └──────────────┘
```

Channels:

- `broadcast::Sender<ChatEvent>` (capacity 2048) — Bot → chat decision
  pipeline. Capacity is large enough to absorb player-flood bursts;
  `RecvError::Lagged(n)` is logged and recorded in the decision log,
  but it never starves durable history.
- `mpsc::Sender<HistoryItem>` (capacity 4096) — Bot → history-writer task,
  parallel to the broadcast. `HistoryItem` is the private wrapper enum
  in `chat::history` with two variants: `Inbound(ChatEvent)` for observed
  chat lines and `BotOut { … }` for lines the bot itself emitted (chat
  or whisper). **Publishers use `try_send`, never `await`** — a hostile
  flooder must not be able to block `bot_task` by filling this channel.
  Inbound publishers and the bot-out publisher (`enqueue_bot_output`)
  share the same drop discipline: on `try_send` failure the publisher
  increments `state.history_drops_today` and logs at most once per
  minute.
- `mpsc::Sender<BotInstruction>` — shared with Store. Chat sends only
  `SendChat` (public chat) and `Whisper` (DM); Store sends everything
  else. Single bot serializer keeps trade flows and chat replies from
  racing the same chat slot.
- `mpsc::Sender<ChatCommand>` (capacity 64) — CLI → chat. Status,
  pause/resume, dry-run toggle, reflection, regeneration, GDPR
  forget-player. See [§ CLI commands](#cli-commands).
- `Arc<AtomicBool> in_critical_section` — Bot writes `true` while a
  trade is in flight or while `current_trade.json` is non-terminal;
  chat reads it. Set bit ⇒ public chat is suppressed and whisper
  replies are deferred up to 30 s.
- `Arc<RwLock<Option<String>>> bot_username` — populated on
  `Event::Init` from `client.profile().name`, reset to `None` on
  `Event::Disconnect`. When the live Arc is `None` the orchestrator
  falls back to `state.last_known_bot_username` (seeded from disk at
  startup) so events arriving in the post-disconnect / pre-Init window
  are processed instead of dropped with `bot_username_unknown`. Only
  events that find neither the live nor the cached value are skipped.

### Panic isolation

`chat_task` is launched inside `tokio::spawn(async move { … })` whose
`JoinError` is caught and logged. A chat panic logs and exits but
**does not tear down the trade bot**.

### Quick exit when disabled

If `config.chat.enabled = false` (the default), `chat_task` drops every
channel and returns immediately at startup, so trade-only operators pay
zero CPU and require no Anthropic API key. The history writer also
exits immediately under the same condition.

### Shutdown order

CLI → Store → Bot ack stays the same as the trade-only path. The CLI
holds a `chat_cmd_tx` clone and additionally:

1. Sends `ChatCommand::Shutdown { ack }` and awaits the ack.
2. Chat task drains the broadcast, persists `state.json`, drops
   `bot_tx`, and acks.
3. The history writer drains `history_tx` after it is dropped, then
   exits — guaranteeing no event is lost during shutdown.

## Whisper routing

Refines the trade-bot pipe at
[src/bot/mod.rs](src/bot/mod.rs). The Store's
[handle_player_command](src/store/handlers/player.rs) replies
`"Unknown command 'X'"` to non-command whispers, so a freeform DM must
**never** reach Store while chat is enabled — otherwise the player gets
both a chat reply and an "Unknown command" hint.

Routing rules at the bot layer, in order
([conversation::route_whisper](src/chat/conversation.rs)):

1. **Chat disabled or in dry-run** → existing behavior preserved: every
   whisper goes to Store. Trade-only operators see no UX regression.
   Dry-run still composes to the decision log without sending.
2. **Normalization** — NFKC, trim, collapse inner whitespace runs. This
   defeats Unicode-smuggling (` buy`, zero-width joiners, fullwidth
   `Ｂｕｙ`).
3. **Empty short-circuit** — empty, sigil-only, or shorter than 2
   characters → dropped silently; written to history, routed nowhere.
4. **Sigil rule** — a single leading `!` or `/` is stripped only when
   followed by an ASCII letter. Multiple leading sigils (`!!buy`,
   `!/buy`) route to chat directly.
5. **Token check** — first whitespace-delimited token, lowercased; if it
   appears in `chat.command_prefixes` (default: `buy`, `sell`,
   `deposit`, `withdraw`, `price`, `balance`, `pay`, `help`, `status`)
   → Store only.
6. **Fuzzy-typo rescue** — token within Levenshtein distance ≤
   `chat.command_typo_max_distance` (default 2) of any prefix AND the
   message is "command-shaped" (≤ 3 tokens, alphanumeric-only) →
   Store. Preserves the trade-UX where a typo whisper gets a hint
   instead of being silently absorbed by chat.
7. **Otherwise** → chat only.

Public chat events go only to chat. Operators who customize
`chat.command_prefixes` get a config-validator warning if any verb
recognized by `parse_command` is missing from the list (it would
silently fall through to chat).

## Per-event decision pipeline

```text
publisher (bot_task)
  parses chat line and:
  • try_send  → history_tx → history-writer task (durable, best-effort)
  • broadcast → chat_events_tx (decision pipeline)

chat_task (subscriber):
  ChatEvent
    │
    ▼
  conversation.rs (local; no LLM, no Mojang)
    ├── self-echo / system pseudo-sender / blocklist / pause     ── drop
    ├── moderation backoff active                                ── drop
    ├── outside active hours (public events only)                ── drop
    ├── spam guard (per-sender sliding window)                   ── suppress
    ├── reply-to-other-speaker (unless addressed)                ── drop
    ▼
  classifier.rs (Haiku gate)
    ├── per-sender classifier cap (default 3/min)                ── skip
    ├── sample-rate roll on undirected public chat               ── skip
    │   (whispers / direct address / questions /
    │    recent-speaker continuations bypass the roll)
    ├── classifier daily cap                                     ── skip
    ├── Haiku call → JSON verdict                                ── parse
    ├── ai_callout.detected → pending_adjustments.jsonl
    ├── respond=false OR confidence<min                          ── drop
    ▼
  composer.rs (Sonnet 4.6 + tool-use loop)
    ├── composer-throttle backoff (after upstream 429/5xx)       ── skip
    ├── system prompt: rules + persona + memory + adjustments
    │                  + per-player memory + history slice + event
    ├── tool calls: read_my_memory / read_player_memory /
    │               update_player_memory / update_self_memory /
    │               read_today_history / search_history /
    │               web_search / web_fetch
    ├── max iterations: composer_max_tool_iterations (default 5)
    ▼
  pacing.rs
    ├── strip_ai_tells + truncate to composer_max_chars
    ├── compute_typing_delay (base + per_char + Gaussian jitter)
    ├── tokio::time::sleep
    ├── recheck_after_sleep: max_replies_per_minute,
    │                        min_silence_secs (public-chat &
    │                          non-direct only; whispers + direct
    │                          addresses bypass),
    │                        in_critical_section (always applied)
    ▼
  BotInstruction::SendChat (public) or Whisper (DM)
    ▼
  decisions.rs append (every step writes a decision JSONL entry)
```

Every step writes to `data/chat/decisions/<date>.jsonl` so an operator
can later answer "why did the bot say (or not say) that?"

### Why history is published, not subscribed

Tokio broadcast channels drop oldest on overflow. If history were
written by the chat-task subscriber, a long composer call could shadow
events past the buffer and silently lose them. Splitting durable
history onto its own task fed by `mpsc::Sender<ChatEvent>` (with
`try_send`) keeps history persistence independent of decision-pipeline
slowdowns. Broadcast lag affects only decision speed; history JSONL is
authoritative.

### Concurrent-message policy

The composer call is sequential: at most one composer in flight per
chat task. Events arriving during composition are accumulated in the
broadcast buffer and the durable history JSONL. On composer completion,
the chat task drains the broadcast and re-runs the local pre-filter on
every accumulated event in arrival order, then selects an event to
advance using this priority:

1. Most-recent surviving event that is **directly addressed to the
   bot**.
2. Otherwise the most-recent surviving event.

A 10-second composer call can shadow a direct address that arrived
3 seconds in. Without explicit prioritization the bot would silently
ignore "Steve, you online?" in favor of generic backlog chatter.

### Addressee detection

| Class       | Trigger                                                                                                  | Effect                                            |
| ----------- | -------------------------------------------------------------------------------------------------------- | ------------------------------------------------- |
| Direct      | Whole-word match on bot username or any nickname in `persona.md` `Nicknames:` line                       | Bypass silence guards; raise priority             |
| Reply-to    | `@<name>` or `<name>,`/`<name>:` in first 16 chars where `<name>` is the most recent non-self speaker    | Stay silent unless `<name>` is the bot            |

There is no hard "dyad" pre-filter. The classifier (Haiku) sees the
recent window and persona, and is instructed to:

- treat the bot as a participant whenever the bot itself was a recent
  speaker (the bot is part of its own 1-on-1 — never silent on those);
- default to staying out of 1-on-1s between two OTHER players, but
  chime in when the bot has something genuinely worth adding (a useful
  fact, a callback, a relevant joke, correction, opinion);
- lean toward responding when a message contains something genuinely
  interesting that the persona has an opinion on, even without direct
  address.

**Dictionary downgrade.** If the bot's username appears in
`data/chat/common_words.txt`, a bare-word match is downgraded — it must
either start the message or be preceded by `@`. This stops "the sky is
nice today" from registering as an address to a bot named `Sky`. The
persona-generation prompt rejects names in `common_words.txt`; existing
personas with conflicted names log a startup warning suggesting
regeneration.

### Spam guard

Per-sender sliding-window counters in
[conversation::SpamGuard](src/chat/conversation.rs):

- `> spam_msgs_per_window` (default 5) events in `spam_window_secs`
  (default 30) → suppress responses to that sender for
  `spam_cooldown_secs` (default 300).
- Repeated near-identical content (Levenshtein ratio ≥ 0.9) within
  60 s → same suppression.
- Sender on operator-managed `data/chat/blocklist.txt` → permanent
  suppression.

Output is symmetric: `pacing::min_silence_secs` is a hard floor and
`pacing::max_replies_per_minute` (default 4) caps the bot-side rate
regardless of incoming volume.

### System pseudo-sender filter

Most servers post automated lines (welcome banners, broadcasts, death
messages, plugin output). They look like chat events but cannot be
UUID-resolved and must not burn classifier tokens
([conversation::is_system_pseudo_sender](src/chat/conversation.rs)).

`is_system_pseudo_sender(name)` returns true if any of:

- `name` does not match the Mojang username shape
  `^[A-Za-z0-9_]{3,16}$` (catches `[Server]`, `[Console]`,
  `Server-Bot`, etc.).
- `name` matches a regex in `data/chat/system_senders_re.txt`.
  Recommended seeds: `^\[.*\]$`, `^Server$`,
  `^(Console|EssentialsX|AnnouncerPlus|Discord(SRV)?)$`. Regex form is
  preferred to exact matching because a real Mojang account named
  `Server` could squat the exact match.
- `name` is in `data/chat/system_senders.txt` (operator-managed exact
  list). Default empty; any entry that is also a valid Mojang username
  shape logs a startup warning.

### Moderation-event parser

The chat module monitors public chat content for moderation events
addressed to the bot. Patterns are operator-configurable in
`data/chat/moderation_patterns.txt`; defaults include:

```text
^You have been muted
^You have been (temp(orarily)? )?banned
^\[Mod(erator)?\] .* (-> |whispers to )?<bot_username>
```

When a pattern matches an inbound line addressing the bot, the chat
module enters **long backoff** (default 24 h, `chat.moderation_backoff_secs`)
— it observes and logs but does not classify or compose. Cleared by the
`Chat: resume after moderation backoff` CLI command.

System-pseudo events are still written to history JSONL — they are
useful context for the composer when it does decide to act — but never
trigger a response.

### AI call-out detection

The classifier's output schema includes `ai_callout: { detected, trigger }`.
When `detected = true`, the classifier-stage post-processor appends a
draft entry to `data/chat/pending_adjustments.jsonl` (NOT directly to
`adjustments.md`).

A separate Haiku-driven **reflection pass**
([reflection::run_pass](src/chat/reflection.rs)) reads the pending file
and produces consolidated, paraphrased lessons. Hardening:

- Every `trigger` value is wrapped in a fresh-nonce
  `<untrusted_chat_…>` block before being shown to the model.
- The reflection system prompt forbids verbatim copying of
  untrusted-tagged text.
- Multi-axis validator (every check must pass):
  1. **Substring overlap** ≤ 40 % of the lesson is literal text from
     any trigger.
  2. **Source diversity** — at least
     `chat.reflection_min_distinct_triggers` triggers (default 3) from
     at least `chat.reflection_min_distinct_senders` senders (default
     3).
  3. **Sender quality** — each contributing sender must have Trust ≥ 1
     (≥ 3 prior bot-replied interactions across ≥ 2 distinct UTC days).
- Crash recovery: write `adjustments.md.tmp` via `write_atomic`, rename
  the pending file to `pending_adjustments.<UTC>.jsonl`, confirm. A
  `.tmp` orphan at startup is treated as not-applied. A timestamped
  pending file with mtime newer than `adjustments.md` logs a warning
  and skips re-running on the same batch.

Trigger conditions (any one fires; once per
`chat.reflection_min_interval_secs`, default 3600):

- `pending_adjustments.jsonl` reached `chat.reflection_max_pending`
  (default 5) AND from at least `chat.reflection_min_distinct_senders`
  senders.
- Pending file non-empty AND chat task idle for
  `chat.reflection_idle_trigger_secs` (default 900) AND the
  distinct-senders requirement above.
- Operator command `Chat: run reflection now` (bypasses the
  distinct-senders requirement).

Operator-triggered runs use a permissive validator (`min_distinct_*= 1`,
all senders treated as Trust 3) — the operator decides.

### Pacing post-process

After the composer returns a string ([pacing.rs](src/chat/pacing.rs)):

1. **Reasoning strip via `pacing::strip_reasoning`.** Defensive backstop
   for chain-of-thought leaks: the composer system prompt forbids
   `<thinking>...</thinking>` / `<reasoning>...</reasoning>` blocks and
   `Thinking:` / `Reasoning:` preamble lines, but a thinking-capable
   model (currently `claude-sonnet-4-6`) occasionally emits them anyway,
   in full or partially mixed with the real reply. This pass excises
   them before any later step runs:
   - Recognized container tags (case-insensitive): `thinking`, `think`,
     `reasoning`, `reason`, `analysis`, `scratchpad`, `monologue`. The
     opening tag, closing tag, and everything between them are dropped.
   - Unclosed opening tag → drop from the tag to end-of-input. The
     model produced reasoning and never reached a real reply; better
     silent than half-leaked thoughts.
   - Whole lines starting with one of `thinking:`, `reasoning:`,
     `analysis:`, `internal:`, `internal monologue:`, `scratchpad:`
     (after trimming leading whitespace, case-insensitive) are dropped.
2. Strip telltale tokens via `pacing::strip_ai_tells`, then apply
   operator regex in `data/chat/strip_patterns.txt`. If persona declares
   "lowercase-by-default", lowercase first-of-sentence.
   - Literal-substring strip using `pacing::BUILT_IN_AI_TELLS`, which
     is exactly six entries: `"As an AI"`, `"as an AI"`, `"I cannot"`,
     `"I'm Claude"`, `"I am Claude"`, `"language model"`.
   - Unicode normalization hard-coded inside `strip_ai_tells` (not in
     the seed list): smart quotes `\u{201c}\u{201d}\u{2018}\u{2019}` →
     ASCII `"`/`'`; em-dash `\u{2014}` → `" - "`; en-dash `\u{2013}` →
     `"-"`.
3. Truncate at `composer_max_chars` (default 240; Minecraft chat allows
   256 with margin for the username prefix).
4. If empty after stripping → silent.
5. **Leading-slash guard.** If the trimmed reply begins with `/` it is
   dropped unconditionally with `decisions.jsonl` reason
   `leading_slash`. The chat module is not a command surface — all
   trade actions go through the Store whisper pipeline — so a
   composer-emitted `/tp accept`, `/msg`, `/kill`, etc. must never
   reach the server. Hardcoded, not configurable.
6. Compute typing delay:
   `delay = clamp(base + per_char × len + Gaussian(0, σ),
                  typing_delay_floor_ms, typing_delay_max_ms)`. The
   Gaussian can yield negative values; the floor (default 400 ms)
   keeps replies from arriving instantly.
7. `tokio::time::sleep(delay)`.
8. **Post-sleep recheck** (the slept reply might be stale by the time
   it would send):
   - `max_replies_per_minute` — always applied.
   - `min_silence_secs` — applied only to public-chat replies that are
     NOT directly addressed; whispers (DMs / `is_public_chat == false`)
     and direct addresses both bypass the floor. The gate exists to
     dampen public-chat noise, not throttle one-to-one conversations,
     so an unrelated bot send during the typing-delay sleep cannot
     silently drop a directly-addressed reply or a DM.
   - `in_critical_section.load(Acquire)` — always applied. A composer
     started before a trade and finishing during it must not fire chat
     mid-trade-step.
9. `BotInstruction::SendChat` (public) or `Whisper` (DM).
10. Update `last_bot_send_at`, append the bot's own line to history
    JSONL with `is_bot: true`.

## Proactive thread continuation

Disabled by default behind `chat.proactive_threading_enabled = false`.
When enabled, the chat task adds a third arm to its `tokio::select!`:
a periodic [`tokio::time::interval`](https://docs.rs/tokio) tick that
may initiate a composer turn even when no inbound chat event arrived.
The motivation is conversational: when the bot has been chatting with
a player and the partner goes quiet, the bot can ask a follow-up,
share an opinion, or change the subject — driving the thread instead
of only reacting.

Gate stack (every gate must pass before the composer fires):

1. **Pause / critical-section / model-404 / composer-throttle** — same
   global short-circuits as the per-event pipeline. Skip with the
   same decision-log reasons.
2. **`evaluate_proactive_tick`**
   ([conversation.rs](src/chat/conversation.rs)) — pure function with
   four sub-gates:
   - `secs_since_last_bot_send ≥ proactive_min_secs_since_bot`
     (default 30) — bot has been silent long enough that another line
     wouldn't be flooding.
   - The most-recent non-bot, non-system event in the recent window
     identifies the partner.
   - `partner_secs_ago ≥ proactive_min_secs_since_partner`
     (default 15) — let a real reply land first if one is coming.
   - `partner_secs_ago ≤ proactive_max_secs_since_partner`
     (default 300, i.e. 5 min) — above this the conversation is dead
     and proactive ticks shouldn't try to revive it.
   - Probability roll: `random < proactive_probability_pct / 100`
     (default 20 %). Most ticks that pass every other gate still stay
     silent; the roll prevents the bot from feeling clockwork.
3. **Decision log.** Both Skip and Fire write to
   `data/chat/decisions/<day>.jsonl` with `kind: "proactive_skip"` or
   `kind: "proactive_fire_pending"`. Skip records carry `reason` —
   one of `bot_never_spoke`, `bot_too_recent`, `no_partner_in_window`,
   `partner_too_recent`, `partner_too_stale`, `probability_roll` —
   so operators can audit how often gates would fire and tune
   thresholds before any tokens are spent.

Status today: gate evaluation, decision logging, and tick wiring are
in place. The composer-side dispatch (loading the partner's per-player
memory, building a synthetic user message that tells the model the
conversation state, running pacing/send) is the next slice of work
and currently logs `proactive_fire_pending` instead of calling
Anthropic. With `proactive_threading_enabled = false` (the default),
the entire arm resolves to `pending()` and adds zero overhead.

| Knob                                | Default | What it does                                                                 |
| ----------------------------------- | ------- | ---------------------------------------------------------------------------- |
| `proactive_threading_enabled`       | `false` | Master switch.                                                                |
| `proactive_tick_secs`               | 30      | Seconds between gate-evaluation checks.                                       |
| `proactive_min_secs_since_bot`      | 30      | Bot must have been silent for at least this long.                             |
| `proactive_min_secs_since_partner`  | 15      | Partner's last message must be at least this old (let real replies land).     |
| `proactive_max_secs_since_partner`  | 300     | Above this, the convo is dead — don't try to revive.                          |
| `proactive_probability_pct`         | 20      | Probability (0-100) that an otherwise-passing tick actually fires.            |

## Memory model

Three layers, each loaded into the composer system prompt.

### Global memory — `memory.md`

LLM-writable via `update_self_memory`. Contains server name + rules,
notable events (operator-seeded), the bot's stable in-world facts
(shop location, what it sells, base coords if shareable, recurring
events it runs) consistent with the persona, and a hard "never claim /
never do" list (no fabricated real-world physical facts, no leaking
other players' data, etc.).

**Commit posture: aggressive.** The composer system prompt instructs
the model to call `update_self_memory` / `update_player_memory`
generously — any time a player shares something fun, insightful, or
worth remembering, and *always* when a player explicitly asks
("remember that…", "don't forget…", "call me X from now on"). Memory
is what makes future conversations richer; the daily cap is the only
soft brake, and most days it goes unused. The growth/poisoning controls
below let us be aggressive about commit volume without letting bad
data accumulate.

Growth + poisoning controls:

- **Section allow-list.** `update_self_memory` writes only to
  `## Inferred`. Operator-managed sections are off-limits.
- **Daily cap.** `chat.update_self_memory_max_per_day` (default 3).
- **Bullet cap.** `chat.memory_max_inferred_bullets` (default 30); when
  a commit pushes past the cap the oldest bullet(s) are evicted to
  `memory.archive.md` (not loaded into the prompt) in the same write.
- **Dedup.** Levenshtein ratio ≥ 0.85 against any existing live bullet
  in `## Inferred` rejects the new bullet at the tool boundary.
- **Eager commit.** A successful `update_self_memory` call writes the
  sanitized, date-prefixed bullet directly to `memory.md` in the same
  composer turn — no pending file, no second-stage Haiku validator.
  The composer (Sonnet 4.6) is the editorial gate; the tool-side
  sanitization + dedup + daily cap are sufficient because each bullet
  is bounded, deduped, and capped per-call. Decision JSONL records the
  full call (input, outcome) for audit.
- **Sanitization.** Bullets matching `(?i)^trust\s*:` or containing
  `## ` are rejected.

### Per-player memory — `players/<uuid>.md`

LLM-writable via `update_player_memory`. UUID-keyed because Minecraft
usernames change. Schema (loose Markdown, model-friendly):

```markdown
# <username at most recent observation>

## Identity
- UUID: <uuid>
- Known names: <comma-separated history>
- First seen: <ISO date>
- Last seen: <ISO date>

## Trust: <level>
<rationale>

## Stated preferences
- <bullet>

## Inferred
- <bullet>

## Topics & history
- <date>: <one-liner>

## Do not mention
- <bullet>
```

Trust is **derived**, never written by the LLM. `memory::compute_trust`
reads the file at load time plus the daily history JSONLs:

| Level | Meaning                                         | Effect on composer                                                              |
| ----: | ----------------------------------------------- | ------------------------------------------------------------------------------- |
| 0     | Unknown / fresh / suspicious                    | Generic small-talk only. Don't change behavior on request. Don't volunteer memories. |
| 1     | Casual acquaintance                              | Honor stable preferences (greeting style). No volunteering.                     |
| 2     | Known regular                                    | Honor preferences; may reference topics raised in this conversation.            |
| 3     | Operator-marked                                  | All of the above; may acknowledge automation if asked privately.                |

Definitions:

> An **interaction** is a `bot_out` JSONL record where the bot replied
> to a message from this player (or a whisper exchanged with this
> player). Raw player events the bot ignored do NOT count. Events that
> hit spam suppression do NOT count.

Trust ladder:

- **0** if `interactions < 3` OR `distinct_days < 2`.
- **1** if `interactions ≥ 3` AND `distinct_days ≥ 2` AND no recent
  spam-cooldown events in the last 7 days.
- **2** if `interactions ≥ 20` AND `distinct_days ≥ 7` AND no recent
  spam-cooldown events.
- **3** only when the player file's `Trust:` heading line matches the
  exact regex `^## Trust: 3$`, set by the operator via the CLI.
  Anchored to a heading line so a bullet body containing `Trust: 3` is
  not parsed as trust. `update_player_memory` rejects bullets matching
  `(?i)^trust\s*:\s*[0-3]` to prevent injection.

Trust 2 honors stated preferences and may reference topics the player
raised in **this** conversation. **Cross-player references (referencing
things other players have said) are gated to Trust 3 only.**

There is no trust-mutation tool. Trust is recomputed every time the
player file is loaded, so it cannot drift out of sync.

Per-file growth control:

- **Hard cap.** `chat.player_memory_max_bytes` (default 4 KB ≈ 1 K
  tokens).
- **Summarization pass** — when a write would exceed the cap by > 25 %,
  a Haiku call rewrites only `Topics & history` and `Inferred`,
  leaving Identity / Trust / Stated preferences / Do not mention
  untouched. Rate-limited to ≤ 1/day/player.
- **Concurrent updates.** Per-UUID `tokio::sync::Mutex` serializes
  writes. If a write would push past the cap and summarization is
  rate-limited, the tool returns
  `"player memory at cap; summarization rate-limited"` so the model
  re-plans.

### Persona — `persona.md` (operator-editable, NOT LLM-writable)

- Generated once on first run from `chat.persona_seed` via a one-shot
  composer-model call: name, age range, region/timezone bias, hobbies, vocabulary
  tics, typo rate, capitalization habits, emoji frequency,
  sentence-length distribution.
- **Generation timing.** `chat_task` blocks on a synchronous
  generation call before processing its first event. On failure the
  task logs and self-disables for the rest of the process — operator
  fixes the API key / quota / network and restarts.
- **Seed sanitization** (`persona::validate_seed`). The seed is
  rejected at config-load time if it contains any control char, `<`,
  `>`, backtick, `</`, `<!--`, `&#`, or any string matching
  `(?i)(ignore|disregard|system|assistant|user)\s*[:>]`.
- **Seed isolation.** The seed is stored separately in
  `data/chat/persona.seed`, never inside `persona.md`. Only a SHA-256
  hash of the seed is recorded in `persona.md` for "regenerated with
  same seed" checks.
- **Trust-block escaping.** Inside `persona.md`, persona text goes
  through one round of angle-bracket HTML-encoding so a generated
  persona that happened to include literal `</something>` cannot
  synthetically close anything.

### Adjustments — `adjustments.md` (reflection-pass only)

Append-only on the wire, bounded in size because every byte rides in
every composer prompt. Schema:

```markdown
- 2026-04-26 | trigger: "you sound like an AI" | lesson: don't use semicolons; keep replies under 12 words
```

- **Hard cap** `chat.adjustments_max_bullets` (default 50). Oldest
  bullets move to `adjustments.archive.md` (not loaded) when exceeded.
- **Dedup** Levenshtein ratio ≥ 0.85 → drop new bullet, bump existing
  date to today.
- **Composer never writes adjustments directly.** It only emits the
  classifier `ai_callout.trigger` signal; only the reflection pass
  writes the live file.

### Cache strategy

Per-player memory blocks are NOT marked for caching even though they
are static per addressee. With N regulars, every change of addressee
invalidates the cache for the next caller, and the 5-min ephemeral TTL
means cache hits are dominated by burst conversations with the same
person. Phase-after-phase-4 measurement decides whether to add
per-player caching. Cache breakpoints today:

- After persona block.
- After `memory.md` block.
- After `adjustments.md` block.

Splitting at adjustments isolates reflection-pass mutations from
persona/memory cache. The composer takes a **byte-for-byte snapshot**
of `memory.md` and `adjustments.md` at the start of the call and
reuses it for every iteration of the tool-use loop, so a concurrent
reflection write does not invalidate the cache mid-call.

## Tools exposed to the composer

Anthropic tool use ([tools.rs](src/chat/tools.rs)). All disk writes go
through [`fsutil::write_atomic`](src/fsutil.rs) (or append for JSONL).

**Tool-use posture: eager.** The composer system prompt instructs the
model to reach for tools first-resort rather than last-resort. When a
player asks "look this up" / "what's the current X" / "check this
URL" / "find Y" — `web_search` and `web_fetch` are the right answer,
not "I can't browse." Same for `search_history` when a player
references something said before. The tool descriptions in
[tools.rs](src/chat/tools.rs) mirror this framing so the schemas the
model sees agree with the system prompt; the daily caps
(`web_fetch_daily_max`, etc.) are the brake, not the tool description.

| Tool                   | Inputs                                  | Behavior                                                                                                                                                                                       |
| ---------------------- | --------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `read_my_memory`       | —                                       | Returns full `memory.md`. Usually redundant with the system prompt; provided for explicit re-fetch.                                                                                            |
| `read_player_memory`   | `uuid` OR `username`                    | **Cross-player firewall**: returns `"access denied"` unless `resolved_uuid == current_event.sender.uuid`. Operator can flip `chat.cross_player_reads = true` for trusted single-tenant servers. |
| `update_player_memory` | `uuid`, `section`, `bullet`             | **Sender binding**: `uuid` argument must equal sender's UUID; cross-player writes rejected unconditionally (no override). Section allow-list: `Stated preferences`, `Inferred`, `Topics & history`, `Do not mention`. Bullet sanitized; ISO-date-prefixed. |
| `update_self_memory`   | `bullet`                                | Eagerly commits a bullet under `## Inferred` of `memory.md` in the same composer turn (no pending-file stage). Daily cap, Levenshtein dedup against `## Inferred`, and sanitization gate the write; bullets evicted past `memory_max_inferred_bullets` roll over to `memory.archive.md`. |
| `read_today_history`   | `since_event_ts?`, `limit_lines?`       | Returns today's JSONL, capped at `chat.tools_history_max_bytes` (default 32 KB ≈ 8 K input tokens). Pagination via `since_event_ts`. Most recent first.                                        |
| `search_history`       | `query`, `days_back`                    | Substring search **scoped strictly to `data/chat/history/*.jsonl`**. Returns up to 50 matches, each capped at 1 KB. Streaming line scan via `tokio::task::spawn_blocking`.                     |
| `web_search`           | `query`                                 | Anthropic native server-tool `web_search_20250305` if available; otherwise returns `"not available"`.                                                                                          |
| `web_fetch`            | `url`                                   | Single GET, max `chat.web_fetch_max_bytes` (default 256 KB), 5 s timeout, plain-text. SSRF + size hardening — see [§ web_fetch hardening](#web_fetch-hardening). Disabled by default.          |

> [!IMPORTANT]
> `update_persona`, any `delete_*`, and `read_decisions` are **not**
> exposed. The composer cannot self-modify persona, cannot erase
> memory, and cannot read its own decision log (which would let it
> "learn to lie better" across sessions).

### web_fetch hardening

[web.rs](src/chat/web.rs) — naive SSRF mitigations are notoriously
incomplete; defenses cover:

- **URL parsing** rejects non-`http(s)` schemes, hostnames in numeric
  forms (decimal `2130706433`, octal `017700000001`, hex `0x7f000001`),
  zero-page hosts (`0.0.0.0/8`), and userinfo (`user@host`).
- **Custom resolver** — a `reqwest::dns::Resolve` impl resolves once,
  IPs are validated, and the connection is **pinned to the validated
  IP** via `ClientBuilder::resolve_to_addrs`. Closes the
  DNS-rebinding TOCTOU window: the connect goes to the IP we vetted,
  not whatever the OS resolver returns at connect time.
- **IP deny-list**: `127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`,
  `192.168.0.0/16`, `169.254.0.0/16` (link-local + cloud metadata
  `169.254.169.254`), `100.64.0.0/10`, `224.0.0.0/4`, `0.0.0.0/8`,
  `::1/128`, `::/128`, `fc00::/7`, `fe80::/10`, `ff00::/8`,
  `64:ff9b::/96`, IPv4-mapped IPv6, and the GCP DNS name
  `metadata.google.internal`.
- **Redirects disabled**: `redirect::Policy::none()`. 3xx responses
  are followed manually after re-running the parse + resolve +
  deny-list path; capped at 3 hops.
- **Streaming size enforcement**: response body read with
  `Response::chunk()` in a loop with a running byte counter; the
  connection aborts as soon as the total exceeds `web_fetch_max_bytes`.
  `Content-Encoding` other than `identity` is rejected (defends
  against zip-bombs); the cap is post-decompression.
- **Cost guard**: `web_fetch` is daily-budgeted via
  `chat.web_fetch_daily_max` (default 50). At cap, the tool returns
  `"rate limited"` without making the call.
- **Logging**: requests log host + status, not the full URL — query
  strings may contain operator-pasted secrets.

## Anthropic API client

[client.rs](src/chat/client.rs).

- **`reqwest`** (already in deps; no new dependency).
- **API key** from `chat.api_key_env` (default `ANTHROPIC_API_KEY`).
  Wrapped in a hand-rolled secret newtype whose `Debug` prints `***`.
  Request URLs are logged but not headers; on error paths only `status`
  + a sanitized message reach the log — never the request or response
  body, which on 401 may include key fragments.
- **Request shape**: standard `/v1/messages` with
  `anthropic-version: 2023-06-01`.
- **Cache TTL**: 1-hour ephemeral cache (beta header
  `extended-cache-ttl-2025-04-11`). The default 5-minute TTL would
  force cache writes on every quiet-period composer call with no hit;
  the 1-hour variant costs 2× write but amortizes 12× longer. Falls
  back to 5-minute with a one-time warning if the beta is rejected.
- **Temperature** is configured per-call via `chat.composer_temperature`
  (default `0.8`) and `chat.classifier_temperature` (default `0.0`).
  The composer wants enough variation to keep the persona voice from
  flattening across replies; the classifier emits a JSON object and
  rewards determinism. Both are `Option<f32>` — `null` in JSON omits
  the temperature field, falling back to the model's API default
  (1.0 for the current Sonnet / Opus / Haiku tiers).
  - **Opus carve-out**: any model ID containing `"opus"` runs without
    an explicit temperature regardless of the configured value. The
    Opus 4.x family interacts poorly with explicit temperature in
    combination with its reasoning behavior; sending `None` lets the
    API default settle that. [`client::effective_temperature`](src/chat/client.rs)
    is the single canonical resolver — composer and classifier
    `build_request` both route through it.
  - Values outside Anthropic's accepted `[0.0, 1.0]` range are clamped
    at request time, so a misconfig surfaces as a slightly-different
    sample rather than a 400 from the API.
- **Client-side rate limiter**: per-model token bucket with both RPM
  and ITPM (input-tokens-per-minute) accounting:

  | Knob                          | Default   |
  | ----------------------------- | --------- |
  | `composer_rpm_max`            | 20        |
  | `classifier_rpm_max`          | 40        |
  | `composer_itpm_max`           | 25 000    |
  | `classifier_itpm_max`         | 40 000    |
  | `rate_limit_wait_max_secs`    | 5         |

  An empty bucket awaits up to `rate_limit_wait_max_secs` before
  erroring out, preventing 429 spirals that would eat the retry
  budget and drop events from the broadcast.
- **Retry policy**: exponential backoff on 429/500/502/503/504, capped
  at 3 attempts, total wall-clock 30 s. **Model-deprecation (404)** is
  non-retryable — log + self-disable composer for 1 h, then retry once.
- **Composer-throttle backoff** (after retries exhaust): a 429/5xx that
  blew through the in-call retry budget engages a short
  `state.composer_throttle_backoff_until` cooldown
  (`composer_throttle_backoff_secs`, default 60 s). The chat task keeps
  classifying events while the timer is set, but composer dispatch
  short-circuits with `kind: "composer_throttle_backoff"` so the next
  event doesn't immediately re-race the same throttled bucket. The
  cooldown auto-clears the first time an event is processed past the
  recorded timestamp.
- **Token meter**. Every response's `usage.input_tokens` and
  `usage.output_tokens` are added to per-day counters in `state.json`.
  **Lazy reset, in-flight attribution**: on every increment, compare
  `state.last_meter_day_utc` against `today_utc()`; if different, zero
  counters and update the date BEFORE adding new usage. Comparing
  calendar days (not durations) is monotonic-clock-jump-safe. **Tokens
  are attributed to the day in which the call STARTED**, not finished.
- **Hard caps from config**: `daily_input_token_cap`,
  `daily_output_token_cap`, `daily_classifier_token_cap`. Composer cap
  short-circuits the composer; classifier keeps running until its own
  cap trips, after which the local pre-filter writes `reason:
  "classifier_daily_cap"` and skips both stages.
- **USD cap and startup estimate**. Tokens are the wrong unit for
  budget decisions:

  - `chat.daily_dollar_cap_usd` (default $5.00). The more conservative
    of {token cap, USD cap} wins.
  - **Validation**: `daily_dollar_cap_usd > 30.0` requires
    `chat.acknowledge_high_spend = true`. Soft-fence against accidentally
    enabling high-spend models.
  - **Startup spend-estimate log line** (INFO level):
    `[Chat] daily caps: input=2M tokens (~$30/day), output=200K tokens
     (~$15/day), classifier=500K tokens (~$0.50/day), USD cap: $5.00.
     Effective daily ceiling: $5.00.` Uses the live `pricing.json` so
    it stays accurate as rates change.
- **Cost estimation** uses
  [`pricing::PricingTable`](src/chat/pricing.rs) loaded from
  `data/chat/pricing.json` — defaults shipped with the binary,
  operator-overridable. Price changes don't require a code release.

## Bot username sharing + reconnect lifecycle

The chat module references the bot's own Minecraft username for two
load-bearing checks: self-echo and direct-address detection. The
display name is decided by the Mojang account profile and is only known
after login, so it lives in
`Arc<tokio::sync::RwLock<Option<String>>> bot_username`.

- **`Event::Init`** populates it from `client.profile().name`.
- **`Event::Disconnect`** resets it to `None` — mirroring how the
  client handle is cleared. Closes the hole where in-flight composer
  calls would compose under the old username during a reconnect window.
- **State persistence** — the last-known username is written to
  `data/chat/state.json` on every change. On chat-task startup, this
  cached value is used as a tentative self-echo filter and history
  backfill key for events that arrived during the
  username-unknown window. As soon as `Event::Init` confirms, any
  divergence triggers a warning and the new value wins.
- **In-flight composer cancellation** — `chat_task` holds a
  `tokio_util::sync::CancellationToken` for the live composer call.
  On `ChatCommand::BotDisconnected` the token fires; tokens billed up
  to abort still count against the daily cap.
- **`is_bot` history tag** — every line written by the bot's own
  `SendChat`/`Whisper` is tagged `"is_bot": true`, independent of
  sender comparison. Events from the pre-username-known window are
  still attributed to the bot when the model later searches history.

Until `Some(name)` is in the lock, `chat_task` forwards events to the
history writer (durable logging) and uses `state.last_known_bot_username`
as a tentative identity for self-echo and direct-address checks. The
authoritative `Event::Init` value overwrites the lock once the handshake
completes; events that find neither a live nor a cached username are
skipped (decision log: `reason: "bot_username_unknown"`).

System-shaped senders (anything that fails the Mojang `[A-Za-z0-9_]{3,16}`
shape — `[Server]`, `[CONSOLE]`, a literal `"1"` proxy tag) are filtered
*before* the bot-username check so server broadcasts in that pre-Init
window aren't spuriously logged as `bot_username_unknown`. Join
broadcasts on those system senders get an extra salvage step in
`bot::parse_chat_line`: if the content carries a join cue (`joined`,
`connected`, `welcome`, `+ <name>`, `[+] <name>`) and contains a
Mojang-shaped username, the event is rewritten to look like a public
chat line from the joining player with the synthetic content
`*just joined the server*`. The classifier prompt knows about that
literal marker and decides whether a greeting is appropriate.

Persona's declared nickname list lives in `persona.md` and is loaded at
chat startup; the canonical username comes from the live login.

## On-disk layout

All non-JSONL writes go through
[`fsutil::write_atomic`](src/fsutil.rs). JSONL files are append-only
and use `OpenOptions::new().append(true)` + a single `write_all(line)`
per record — single-process atomicity is sufficient because the
history-writer task is the only writer to its files.

```text
data/
  chat/
    memory.md                     ← global self/server/world memory (LLM-writable via update_self_memory)
    memory.archive.md             ← archived oldest bullets when memory.md exceeds the cap
    persona.md                    ← locked persona profile (operator-editable, NOT LLM-writable)
    persona.seed                  ← raw persona seed (separate from persona.md)
    persona.md.<UTC>              ← rotated archives from operator regenerate (max 10)
    adjustments.md                ← reflection-pass-only learnings from AI call-outs
    adjustments.archive.md        ← archived oldest bullets when adjustments.md exceeds the cap
    state.json                    ← runtime state mirror (last_replied_at, token meter, etc.)
    pending_adjustments.jsonl     ← classifier-stage call-out drafts awaiting reflection
    operator_audit.jsonl          ← Trust-3 toggles, GDPR forget-player events
    blocklist.txt                 ← operator-managed: senders to ignore entirely
    common_words.txt              ← words that downgrade bare-word direct-address (e.g. "Sky")
    moderation_patterns.txt       ← regex per line: muted/banned moderation events
    strip_patterns.txt            ← regex per line: AI-tell stripping
    system_senders.txt            ← exact-match system-pseudo-sender list (default empty)
    system_senders_re.txt         ← regex-match system-pseudo-sender list (preferred)
    pricing.json                  ← per-model token-cost table (operator-overridable)
    players/
      <uuid>.md                   ← per-player memory (UUID, not username — survives renames)
      _index.json                 ← {username_lc: uuid} convenience map, rebuilt on load
    history/
      YYYY-MM-DD.jsonl            ← every observed chat line + bot output, one JSON object per line
      YYYY-MM-DD.uuids.json       ← ts → uuid overlay sidecar from background resolution
    decisions/
      YYYY-MM-DD.jsonl            ← classifier verdicts + composer calls (cost, latency, reasoning)
```

> [!IMPORTANT]
> `data/chat/` contains plaintext player conversation history. Add it
> to `.gitignore` before publishing. On Unix `chmod 700 data/chat/`;
> on Windows restrict ACLs.

### Field-level caps

JSONL records truncate **field payloads** before serialization, never
the line itself (cutting mid-UTF-8 or mid-string-escape produces
unparseable JSON):

| Field                                         | Cap                                       |
| --------------------------------------------- | ----------------------------------------- |
| `event.content`                               | 4 KB                                      |
| `tool_result` content in decision JSONL       | `chat.tools_history_max_bytes` (32 KB)    |
| `web_fetch` body summary                      | 8 KB                                      |
| `error_message` field on failure entries      | 1 KB                                      |

When a field is truncated, sibling field `truncated_<field>: true` is
added in the same object. If the whole serialized record would exceed
`chat.history_max_line_bytes` (default 64 KB) even after field caps,
the line is dropped and a single
`{ts, kind: "dropped", reason: "oversize", original_kind: …, size: N}`
record is written instead.

### UUID resolution & history keying

`ChatEvent.sender` is a Minecraft username; per-player memory is
UUID-keyed. Naively resolving every public-chat event through Mojang
would burn the 600-req/10-min rate limit on a busy server within
minutes.

- **History JSONL records both fields** — `sender` always present,
  `uuid` lazy (`null` unless already known).
- **Lookup order** on each event: in-process `username_lc → uuid`
  cache → `_index.json` on disk → Mojang API.
- **Resolution is required only when** the bot decides to act
  (composer about to be called, or `update_player_memory` invoked).
  Pre-filter and classifier use the username string only.
- **Background resolver** spawned by `chat_task` drains a bounded
  `VecDeque<String>` at ≤ 30 req/min and patches the corresponding
  history JSONL records via `history/<date>.uuids.json` overlay
  sidecars. Search joins the overlay at read time. The queue is
  bounded at `chat.uuid_resolve_queue_max` (default 1024); when full,
  oldest entries drop — chat replies do on-demand resolution at the
  composer stage, so a dropped background enqueue only delays sidecar
  backfill, never blocks a reply.
- **System pseudo-senders** are never queued for resolution.

### `state.json`

Single JSON object, atomically rewritten on every change.
Operator-editable when the chat task is stopped. **NOT** LLM-writable.

```json
{
  "version": 1,
  "last_meter_day_utc": "2026-04-26",
  "tokens_today": {
    "composer_input": 0, "composer_output": 0,
    "classifier_input": 0, "classifier_output": 0,
    "estimated_usd": 0.0
  },
  "last_known_bot_username": "<name>|null",
  "paused": false,
  "dry_run_runtime_override": false,
  "moderation_backoff_until": "<UTC ISO>|null",
  "model_404_backoff_until": "<UTC ISO>|null",
  "composer_throttle_backoff_until": "<UTC ISO>|null",
  "persona_regen_cooldown_until": "<UTC ISO>|null",
  "history_drops_today": 0
}
```

`version` allows migration on schema changes; the loader refuses to
start on unknown versions. Operator playbook entries:

- **Reset daily caps** (e.g. for testing): set `tokens_today` fields
  to 0 and `last_meter_day_utc` to today.
- **Clear moderation backoff**: set `moderation_backoff_until` to null,
  or use `Chat: resume after moderation backoff`.
- **Clear composer-throttle backoff**: set `composer_throttle_backoff_until`
  to null. The chat task auto-clears it on the next event whose timestamp
  is past the recorded value, so manual edits are only needed when an
  operator wants composer dispatch resumed *immediately* before the
  configured `composer_throttle_backoff_secs` window has elapsed.
- **Clear persona-regen cooldown**: set `persona_regen_cooldown_until`
  to null.
- **Force re-resolution of bot username**: set
  `last_known_bot_username` to null.

### Why these formats

- **Markdown** for memory/persona/adjustments — human-editable, easy to
  grep, the LLM produces structured Markdown natively without
  serialization friction. Operators can hand-edit at any time.
- **JSONL** for history/decisions — append-only crash safety,
  line-by-line searchable, easy to slice by date range, no parser
  required to inspect.
- **UUID-keyed per-player files** — usernames change in Minecraft;
  UUID is the only stable identity, and matches the keying scheme
  used by [`store::utils::resolve_user_uuid`](src/store/utils.rs).
- **`_index.json`** is a derived map and may be rebuilt from the
  `players/` directory; corruption is recoverable by deletion +
  restart.

### Retention sweep

[`retention::run_sweep`](src/chat/retention.rs) runs at chat-task
startup and at the first event observed each new UTC day:

- `history/<date>.jsonl` older than `chat.history_retention_days`
  (default 30) — deleted along with paired `<date>.uuids.json`
  sidecars.
- `decisions/<date>.jsonl` older than `chat.decisions_retention_days`
  (default 30).
- `pending_adjustments.<UTC>.jsonl` older than the same retention
  window. (`pending_self_memory.<UTC>.jsonl` archives are vestigial —
  `update_self_memory` now commits eagerly to `memory.md`, so no new
  rotated archives are produced; the sweep still cleans any leftover
  pre-migration files.)
- `persona.md.<UTC>` archives, capped by **count**
  (`chat.persona_archive_max`, default 10) rather than age.
- `adjustments.archive.md` and `memory.archive.md` rotation: when they
  grow past `chat.archive_max_bytes` (default 1 MB), they rotate to
  dated sub-files (`adjustments.archive.<UTC>.md`), then the dated
  files are governed by the standard retention sweep.

## CLI commands

Gated on `config.chat.enabled`. Send via `chat_cmd_tx` from
[`src/cli.rs`](src/cli.rs):

| Entry                                            | Behavior                                                                                                                               |
| ------------------------------------------------ | -------------------------------------------------------------------------------------------------------------------------------------- |
| `Chat: status`                                   | Snapshot: enabled/paused/dry-run flags; today's input/output/classifier tokens vs caps with USD; last composer call ts + cost; current backoff state (moderation, model deprecation); `pending_adjustments` count; last persona regeneration date; `in_critical_section` duration; history drop counter. |
| `Chat: toggle dry-run`                           | Runtime override of `chat.dry_run`. Persists in `state.json`.                                                                          |
| `Chat: show today's decision log (last N)`       | Tails today's `data/chat/decisions/<today>.jsonl`.                                                                                     |
| `Chat: show token spend today`                   | Same numbers as `status` in compact form.                                                                                              |
| `Chat: replay event <event_ts>`                  | Re-renders the system prompt that would be sent for the given history line. No API call.                                              |
| `Chat: reset player memory <username>`           | Deletes `data/chat/players/<uuid>.md` after confirmation.                                                                              |
| `Chat: set operator trust <username>`            | Writes `## Trust: 3` heading. Prints the player's last 5 history lines + current memory file as a sanity check before. Records to `operator_audit.jsonl`. Sets `trust3_expires_at` (default `now + chat.trust3_max_days`, 30 days). |
| `Chat: clear operator trust <username>`          | Removes the `## Trust: 3` heading, restoring derived trust.                                                                            |
| `Chat: dump player memory <username>`            | Prints `players/<uuid>.md` to stdout.                                                                                                  |
| `Chat: regenerate persona`                       | One-shot; requires confirmation + 24 h cooldown. Archives prior persona to `persona.md.<UTC>`.                                         |
| `Chat: run reflection now`                       | Consumes `pending_adjustments.jsonl` immediately. Permissive validator (operator decides).                                             |
| `Chat: forget player <username>`                 | GDPR: purges per-player file + history JSONL records + decisions JSONL records + overlay sidecars. Confirmation prompt; logged to operator audit. |
| `Chat: resume after moderation backoff`          | Clears `state.moderation_backoff_until`.                                                                                               |
| `Chat: pause` / `Chat: resume`                   | Toggles `state.paused`, observed at the top of the chat decision pipeline.                                                              |

## Behavior guards and threat mitigations

The bot is openly an AI, so the goal of these guards is **good chat
hygiene + safety**, not detection-evasion. Some columns (typing
delay, typo rate) exist to keep replies tonally appropriate for a
Minecraft chat — paragraph-length formal-assistant replies feel as
out-of-place as zero-typo perfect grammar. Others (rate caps, prompt
injection defense, SSRF defense) are real safety/cost guards.

| Concern                         | Mitigation                                                                                                                                                                |
| ------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Instant replies feel robotic    | Typing delay (`typing_delay_*`) with Gaussian jitter and a post-jitter floor.                                                                                              |
| Formal-assistant prose          | Persona-driven typo rate, capitalization habits, sentence-length distribution — keeps replies in Minecraft-chat tempo, not ChatGPT register.                               |
| Replying to everything          | Classifier pre-filter (sample-rate roll on undirected public chat) + per-sender classifier rate cap. The bot is conversational, not a chatbot that floods every line.    |
| Leaking cross-player data       | Cross-player firewall on `read_player_memory` + Trust-gated proactive references; Trust 3 is the only level allowed to reference cross-player history.                    |
| Persona / voice drift           | `persona.md` is not LLM-writable; no `update_persona` tool exists.                                                                                                         |
| Reply flood / spam              | Gaussian jitter on typing delay; `max_replies_per_minute` cap.                                                                                                             |
| Talking through trades          | `in_critical_section` flag suppresses public chat and defers whispers up to 30 s.                                                                                          |
| Being muted but still talking   | Moderation-event parser → 24-h backoff (`chat.moderation_backoff_secs`).                                                                                                   |
| Echo loop                       | Self-echo guard on bot username; spam guard on the same external username flipping ≥ 4 messages in 10 s.                                                                   |
| Prompt injection in chat        | Nonce-tagged `<untrusted_chat_…>` wrappers; defensive rejection of events containing `<untrusted` (any case) before wrapping.                                              |
| Prompt injection via tools      | `<untrusted_tool_result_…>` and `<untrusted_web_…>` markers; the static rules block instructs the model to ignore instructions inside any `<untrusted_…>` tag.            |
| Prompt injection in adjustments | Reflection pass paraphrase requirement + multi-axis validator (substring overlap, distinct triggers, distinct senders, sender Trust ≥ 1).                                  |
| Off-tone phrases                | `pacing::strip_ai_tells` removes em-dashes, smart quotes, "As an AI", "I'm Claude", "language model", etc. — these are tonally wrong for Minecraft chat regardless of identity. |
| Cost-DoS via classifier flood   | Per-sender classifier rate cap; per-call sample-rate roll on undirected public chat; separate `daily_classifier_token_cap`; spam-suppressed senders skip classifier entirely.                |
| Cost-DoS via composer flood     | `daily_input_token_cap` + `daily_output_token_cap` + `daily_dollar_cap_usd`; client-side rate limiter with RPM and ITPM accounting; classifier pre-filter gates composer dispatch. |
| SSRF via web_fetch              | URL-parse rejects numeric/octal/hex hosts and userinfo; pinned-IP DNS; deny-list including cloud metadata; manual redirect re-validation; streaming size cap.              |

## Where to start reading

| You want to understand…                              | Read this                                                                                              |
| ---------------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| The whole chat task lifecycle                        | [src/chat/mod.rs](src/chat/mod.rs) `chat_task` + `process_event`                                       |
| How a chat line becomes a decision                   | [src/chat/conversation.rs](src/chat/conversation.rs) → [src/chat/classifier.rs](src/chat/classifier.rs) → [src/chat/composer.rs](src/chat/composer.rs) |
| Whisper routing (Store vs chat)                      | [src/chat/conversation.rs](src/chat/conversation.rs) `route_whisper`                                   |
| Composer system-prompt assembly                      | [src/chat/composer.rs](src/chat/composer.rs) `build_request`, `PromptSnapshot`                         |
| Tools the model can call                             | [src/chat/tools.rs](src/chat/tools.rs)                                                                 |
| Trust derivation                                     | [src/chat/memory.rs](src/chat/memory.rs) `compute_trust`, `count_interactions_for_uuid`                |
| AI call-out → adjustments                            | [src/chat/classifier.rs](src/chat/classifier.rs) `write_pending_adjustment` → [src/chat/reflection.rs](src/chat/reflection.rs) `run_pass` |
| Anthropic API client + rate limiter                  | [src/chat/client.rs](src/chat/client.rs)                                                               |
| Pacing post-process and post-sleep recheck           | [src/chat/pacing.rs](src/chat/pacing.rs) `compute_typing_delay`, `recheck_after_sleep`                 |
| web_fetch SSRF defenses                              | [src/chat/web.rs](src/chat/web.rs)                                                                     |
| Daily retention sweep                                | [src/chat/retention.rs](src/chat/retention.rs) `run_sweep`                                             |
| Config schema + defaults                             | [src/config.rs](src/config.rs) `ChatConfig`                                                            |
| Inter-task message types                             | [src/messages.rs](src/messages.rs) `ChatEvent`, `ChatCommand`, `BotInstruction::SendChat`              |
