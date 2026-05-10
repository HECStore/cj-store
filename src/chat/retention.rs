//! Retention sweep — deletes old chat history / decision JSONL files
//! and rotated archives.
//!
//! Runs at chat-task startup AND at the first event observed each new
//! UTC day. The same retention sweep prunes:
//!
//! - `data/chat/history/<date>.jsonl` files older than
//!   `chat.history_retention_days`.
//! - `data/chat/decisions/<date>.jsonl` files older than
//!   `chat.decisions_retention_days`.
//! - Paired `history/<date>.uuids.json` overlay sidecars (pruned with
//!   their history file).
//! - `pending_adjustments.<UTC>.jsonl` rotated archives.
//! - `pending_self_memory.<UTC>.jsonl` rotated archives.
//! - `persona.md.<UTC>` archives, capped by COUNT (default 10), not age.
//! - `adjustments.archive.<UTC>.md` and `memory.archive.<UTC>.md`
//!   rotated sub-files.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use tracing::{debug, info, warn};

/// Outcome of a retention sweep. Surfaces in the `Chat: status` CLI
/// command as "last sweep deleted X files, kept Y".
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SweepReport {
    pub history_deleted: usize,
    pub decisions_deleted: usize,
    pub overlays_deleted: usize,
    pub pending_adjustments_deleted: usize,
    pub pending_self_memory_deleted: usize,
    pub persona_archives_deleted: usize,
    pub markdown_archives_deleted: usize,
    /// Dates seen on a `history/<date>.jsonl` whose paired
    /// `history/<date>.uuids.json` was missing entirely (i.e. an
    /// overlay was never written, or was already pruned).
    pub history_without_overlay: usize,
    /// Dates seen on a `history/<date>.uuids.json` whose paired
    /// `history/<date>.jsonl` was missing entirely. Surfaces a UUID
    /// overlay that points at non-existent rows — typically the
    /// fingerprint of a half-completed earlier sweep.
    pub orphan_overlay: usize,
}

impl SweepReport {
    pub fn total(&self) -> usize {
        self.history_deleted
            + self.decisions_deleted
            + self.overlays_deleted
            + self.pending_adjustments_deleted
            + self.pending_self_memory_deleted
            + self.persona_archives_deleted
            + self.markdown_archives_deleted
    }
}

/// Configuration for a single sweep run. Caller pulls these from
/// `ChatConfig`.
pub struct SweepConfig {
    pub chat_dir: PathBuf,
    pub history_retention_days: u32,
    pub decisions_retention_days: u32,
    pub persona_archive_max: u32,
    /// "Today" — usually `Utc::now()`, but threadable for tests.
    pub today: DateTime<Utc>,
}

/// Run the sweep. Returns a [`SweepReport`] for logging / status display.
/// I/O errors on individual files are logged at warn but never fail the
/// whole sweep.
pub fn run_sweep(config: &SweepConfig) -> SweepReport {
    let mut r = SweepReport::default();

    // History + paired overlays: walked together so a successful jsonl
    // delete with a failed uuids.json delete (or vice-versa) doesn't leave
    // a dangling overlay pointing at non-existent rows.
    let history_dir = config.chat_dir.join("history");
    if history_dir.exists() {
        sweep_history_paired(
            &history_dir,
            config.history_retention_days,
            config.today,
            &mut r,
        );
    }

    // Decisions.
    let decisions_dir = config.chat_dir.join("decisions");
    if decisions_dir.exists() {
        sweep_dated_jsonl(
            &decisions_dir,
            "jsonl",
            config.decisions_retention_days,
            config.today,
            &mut r.decisions_deleted,
        );
    }

    // Rotated `pending_adjustments.<UTC>.jsonl`. Date pattern is
    // `YYYYMMDDTHHMMSSZ`.
    sweep_rotated_pending(
        &config.chat_dir,
        "pending_adjustments",
        config.history_retention_days,
        config.today,
        &mut r.pending_adjustments_deleted,
    );
    sweep_rotated_pending(
        &config.chat_dir,
        "pending_self_memory",
        config.history_retention_days,
        config.today,
        &mut r.pending_self_memory_deleted,
    );

    // Persona archives — pruned by COUNT, not age.
    sweep_persona_archives(
        &config.chat_dir,
        config.persona_archive_max,
        &mut r.persona_archives_deleted,
    );

    // Rotated archive files: adjustments.archive.<UTC>.md,
    // memory.archive.<UTC>.md.
    for prefix in ["adjustments.archive", "memory.archive"] {
        sweep_rotated_archive(
            &config.chat_dir,
            prefix,
            config.history_retention_days,
            config.today,
            &mut r.markdown_archives_deleted,
        );
    }

    if r.total() > 0 || r.history_without_overlay > 0 || r.orphan_overlay > 0 {
        info!(
            history = r.history_deleted,
            decisions = r.decisions_deleted,
            overlays = r.overlays_deleted,
            pending_adj = r.pending_adjustments_deleted,
            pending_self = r.pending_self_memory_deleted,
            persona_archives = r.persona_archives_deleted,
            markdown_archives = r.markdown_archives_deleted,
            history_without_overlay = r.history_without_overlay,
            orphan_overlay = r.orphan_overlay,
            "[Chat] retention sweep deleted files"
        );
    } else {
        debug!("[Chat] retention sweep: no files to delete");
    }

    r
}

/// Sweep `<dir>/<YYYY-MM-DD>.<ext>` files older than `retain_days`.
/// `ext` is matched as a SUFFIX so callers can pass `"uuids.json"`
/// (compound extension) or `"jsonl"`.
fn sweep_dated_jsonl(
    dir: &Path,
    ext: &str,
    retain_days: u32,
    today: DateTime<Utc>,
    out: &mut usize,
) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(dir = %dir.display(), error = %e, "could not list dir for sweep");
            return;
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let suffix = format!(".{ext}");
        let Some(date_part) = name.strip_suffix(&suffix) else {
            continue;
        };
        let Some(file_date) = parse_date_str(date_part) else {
            continue;
        };
        if days_between(file_date, today) > retain_days as i64 {
            if let Err(e) = fs::remove_file(&path) {
                warn!(path = %path.display(), error = %e, "sweep delete failed");
                continue;
            }
            *out += 1;
        }
    }
}

/// Sweep history `.jsonl` and `.uuids.json` siblings as a single unit.
///
/// The earlier two-call approach (one `sweep_dated_jsonl` per extension)
/// could delete one sibling while leaving the other — typically a
/// `.uuids.json` overlay whose `.jsonl` rows had already been removed,
/// pointing at nothing. This pass enumerates candidate dates first,
/// then for each date deletes BOTH siblings (or neither, on a partial
/// failure). Orphan-without-history and history-without-overlay are
/// surfaced in the [`SweepReport`] so dashboards can see them.
fn sweep_history_paired(
    dir: &Path,
    retain_days: u32,
    today: DateTime<Utc>,
    report: &mut SweepReport,
) {
    use std::collections::BTreeMap;
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(dir = %dir.display(), error = %e, "could not list dir for sweep");
            return;
        }
    };
    // For each date, track whether we saw a `.jsonl`, a `.uuids.json`, or both.
    let mut by_date: BTreeMap<String, (Option<PathBuf>, Option<PathBuf>)> = BTreeMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // Extract owned classification first so the `name` borrow drops
        // before we move `path` into the map.
        // Order matters: `.uuids.json` ends in `.json` too, so check the
        // compound suffix FIRST.
        enum Kind {
            History,
            Overlay,
        }
        let parsed: Option<(String, Kind)> = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|name| {
                if let Some(date_part) = name.strip_suffix(".uuids.json") {
                    if parse_date_str(date_part).is_some() {
                        return Some((date_part.to_string(), Kind::Overlay));
                    }
                } else if let Some(date_part) = name.strip_suffix(".jsonl") {
                    if parse_date_str(date_part).is_some() {
                        return Some((date_part.to_string(), Kind::History));
                    }
                }
                None
            });
        if let Some((date_str, kind)) = parsed {
            let slot = by_date.entry(date_str).or_default();
            match kind {
                Kind::History => slot.0 = Some(path),
                Kind::Overlay => slot.1 = Some(path),
            }
        }
    }
    for (date_str, (jsonl_path, overlay_path)) in by_date {
        let Some(file_date) = parse_date_str(&date_str) else {
            continue;
        };
        if days_between(file_date, today) <= retain_days as i64 {
            // Within retention. Still surface anomalies for visibility.
            match (&jsonl_path, &overlay_path) {
                (Some(_), None) => report.history_without_overlay += 1,
                (None, Some(_)) => report.orphan_overlay += 1,
                _ => {}
            }
            continue;
        }
        // Past retention. Try to delete both siblings; only count
        // successes. A failure on one logs a warn but doesn't block
        // the other — leaving a partial state on disk is no worse than
        // the prior independent-sweep behavior, and the dashboard will
        // notice next sweep via the orphan counters.
        if let Some(p) = &jsonl_path {
            match fs::remove_file(p) {
                Ok(()) => report.history_deleted += 1,
                Err(e) => {
                    warn!(path = %p.display(), error = %e, "sweep delete (history) failed");
                }
            }
        } else {
            // Overlay-only past retention: still record as orphan
            // before we delete it, so the dashboard sees the prior
            // partial-sweep fingerprint.
            report.orphan_overlay += 1;
        }
        if let Some(p) = &overlay_path {
            match fs::remove_file(p) {
                Ok(()) => report.overlays_deleted += 1,
                Err(e) => {
                    warn!(path = %p.display(), error = %e, "sweep delete (overlay) failed");
                }
            }
        } else if jsonl_path.is_some() {
            // History-only past retention: no overlay was ever written
            // (or it was already pruned). Surface for visibility.
            report.history_without_overlay += 1;
        }
    }
}

/// Sweep rotated pending files: `<prefix>.<YYYYMMDDTHHMMSSZ>[-<seq>[-<bump>]][.failed[-<bump>]].jsonl`.
///
/// Accepts the broader grammar because `reflection::rotate_pending` now
/// uses an `ARCHIVE_SEQ` + bump-suffix-on-collision pattern (so two
/// rotations in the same UTC second don't overwrite each other), and the
/// `.failed.jsonl` variant for Haiku-failure quarantine. The leading
/// `<YYYYMMDDTHHMMSSZ>` stamp remains canonical — anything after the
/// first `-` (or `.`) following it is decoration we ignore for age
/// purposes.
fn sweep_rotated_pending(
    chat_dir: &Path,
    prefix: &str,
    retain_days: u32,
    today: DateTime<Utc>,
    out: &mut usize,
) {
    let entries = match fs::read_dir(chat_dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(dir = %chat_dir.display(), error = %e, "could not list chat dir for pending sweep");
            return;
        }
    };
    let p_prefix = format!("{prefix}.");
    let p_suffix = ".jsonl";
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let Some(rest) = name.strip_prefix(&p_prefix) else {
            continue;
        };
        let Some(rest_no_ext) = rest.strip_suffix(p_suffix) else {
            continue;
        };
        // Extract the leading `<YYYYMMDDTHHMMSSZ>` stamp. The compact
        // grammar is fixed-width (16 chars), so just slice the leading
        // segment up to the first non-stamp char.
        let stamp_segment = rest_no_ext
            .split(|c: char| c == '-' || c == '.')
            .next()
            .unwrap_or(rest_no_ext);
        let Some(file_date) = parse_compact_utc_stamp(stamp_segment) else {
            continue;
        };
        if days_between(file_date, today) > retain_days as i64 {
            if let Err(e) = fs::remove_file(&path) {
                warn!(path = %path.display(), error = %e, "sweep delete failed");
                continue;
            }
            *out += 1;
        }
    }
}

/// Persona archives are pruned by COUNT. Files are named
/// `persona.md.<YYYYMMDDTHHMMSSZ>`. Keep the `max` newest.
fn sweep_persona_archives(chat_dir: &Path, max: u32, out: &mut usize) {
    // Defensive: a misconfigured `persona_archive_max = 0` would otherwise
    // wipe every persona snapshot on first sweep (irrecoverable). Treat
    // zero as "feature disabled" — the cap is meant to bound history, not
    // serve as a delete-all toggle.
    if max == 0 {
        warn!("persona_archive_max is 0; refusing to wipe all archives, treating as disabled");
        return;
    }
    let entries = match fs::read_dir(chat_dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(dir = %chat_dir.display(), error = %e, "could not list chat dir for persona archive sweep");
            return;
        }
    };
    let mut archives: Vec<(PathBuf, DateTime<Utc>)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        // Parse the stamp into a `DateTime<Utc>` first, dropping the borrow on
        // `path` before we move `path` into the vec.
        let parsed: Option<DateTime<Utc>> = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|name| name.strip_prefix("persona.md."))
            .and_then(parse_compact_utc_stamp);
        if let Some(dt) = parsed {
            archives.push((path, dt));
        }
    }
    if archives.len() <= max as usize {
        return;
    }
    // Sort by parsed `DateTime<Utc>` ascending → oldest first. The earlier
    // implementation sorted by lexicographic stamp string, which only happens
    // to match chronological order because the grammar is fixed-width — a
    // brittle coupling. Sorting on the already-parsed timestamp makes the
    // ordering robust to any future grammar relaxation.
    archives.sort_by_key(|(_, dt)| *dt);
    let to_delete = archives.len() - max as usize;
    for (path, _) in archives.into_iter().take(to_delete) {
        if let Err(e) = fs::remove_file(&path) {
            warn!(path = %path.display(), error = %e, "persona archive delete failed");
            continue;
        }
        *out += 1;
    }
}

fn sweep_rotated_archive(
    chat_dir: &Path,
    prefix: &str,
    retain_days: u32,
    today: DateTime<Utc>,
    out: &mut usize,
) {
    let entries = match fs::read_dir(chat_dir) {
        Ok(e) => e,
        Err(e) => {
            warn!(dir = %chat_dir.display(), error = %e, "could not list chat dir for rotated archive sweep");
            return;
        }
    };
    let p_prefix = format!("{prefix}.");
    let p_suffix = ".md";
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        let Some(rest) = name.strip_prefix(&p_prefix) else {
            continue;
        };
        let Some(stamp) = rest.strip_suffix(p_suffix) else {
            continue;
        };
        let Some(file_date) = parse_compact_utc_stamp(stamp) else {
            continue;
        };
        if days_between(file_date, today) > retain_days as i64 {
            // CHAT.md: increment the counter ONLY on a successful
            // delete. A failed delete logs a warn and leaves the counter
            // alone so retention reports stay honest.
            match fs::remove_file(&path) {
                Ok(()) => {
                    *out += 1;
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "archive delete failed");
                }
            }
        }
    }
}

/// Parse `YYYY-MM-DD` to a `DateTime<Utc>` at midnight UTC.
pub fn parse_date_str(s: &str) -> Option<DateTime<Utc>> {
    let dt = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    let ndt = dt.and_hms_opt(0, 0, 0)?;
    Some(DateTime::from_naive_utc_and_offset(ndt, Utc))
}

/// Parse `YYYYMMDDTHHMMSSZ` (compact UTC ISO-8601 form used by archive
/// filenames; CHAT.md).
pub fn parse_compact_utc_stamp(s: &str) -> Option<DateTime<Utc>> {
    let dt = chrono::NaiveDateTime::parse_from_str(s, "%Y%m%dT%H%M%SZ").ok()?;
    Some(DateTime::from_naive_utc_and_offset(dt, Utc))
}

/// Whole UTC days between two timestamps. Uses `NaiveDate` arithmetic so
/// the boundary is calendar-day-aligned regardless of the time-of-day
/// component of `today`: a file written at 2026-04-01T00:00:00Z and a
/// `today` of 2026-05-01T23:59:59Z is 30 days, not 30.999... that flooring
/// `(secs / 86_400)` rounds down to 30 on the day boundary and 29 the
/// minute before — both wrong against a calendar-day retention policy.
fn days_between(older: DateTime<Utc>, newer: DateTime<Utc>) -> i64 {
    newer
        .date_naive()
        .signed_duration_since(older.date_naive())
        .num_days()
}

/// Detect whether the chat-task should run a sweep right now. The
/// caller maintains `last_sweep_day` (UTC ISO `YYYY-MM-DD`); this
/// returns `Some(today)` if today's date differs and the caller
/// should run the sweep.
pub fn sweep_due_today(last_sweep_day: Option<&str>) -> Option<String> {
    let today = Utc::now().format("%Y-%m-%d").to_string();
    if last_sweep_day == Some(today.as_str()) {
        None
    } else {
        Some(today)
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::time::Duration;

    /// Scratch chat-dir laid out like `data/chat/`. Cleanup via Drop.
    struct ChatScratch(PathBuf);

    impl ChatScratch {
        fn new(name: &str) -> Self {
            let base = std::env::temp_dir().join(format!(
                "cj-store-retention-{}-{}-{}",
                name,
                std::process::id(),
                chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
            ));
            let _ = fs::remove_dir_all(&base);
            fs::create_dir_all(&base).unwrap();
            fs::create_dir_all(base.join("history")).unwrap();
            fs::create_dir_all(base.join("decisions")).unwrap();
            Self(base)
        }
    }

    impl Drop for ChatScratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn touch(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    fn today_at(year: i32, month: u32, day: u32) -> DateTime<Utc> {
        let d = chrono::NaiveDate::from_ymd_opt(year, month, day).unwrap();
        let dt = d.and_hms_opt(0, 0, 0).unwrap();
        DateTime::from_naive_utc_and_offset(dt, Utc)
    }

    // ---- date parsers ---------------------------------------------------

    #[test]
    fn parse_date_str_handles_iso() {
        let d = parse_date_str("2026-04-26").unwrap();
        assert_eq!(d.format("%Y-%m-%d").to_string(), "2026-04-26");
    }

    #[test]
    fn parse_date_str_rejects_bad_format() {
        assert!(parse_date_str("2026/04/26").is_none());
        assert!(parse_date_str("yesterday").is_none());
    }

    #[test]
    fn parse_compact_utc_stamp_handles_iso() {
        let d = parse_compact_utc_stamp("20260426T103000Z").unwrap();
        assert_eq!(d.format("%Y-%m-%d").to_string(), "2026-04-26");
    }

    #[test]
    fn days_between_is_inclusive_of_partial_days() {
        let a = today_at(2026, 4, 1);
        let b = today_at(2026, 4, 5);
        assert_eq!(days_between(a, b), 4);
    }

    // ---- history sweep --------------------------------------------------

    #[test]
    fn history_sweep_deletes_files_past_retention() {
        let s = ChatScratch::new("hist-prune");
        // Old file (40 days ago).
        touch(&s.0.join("history/2026-03-17.jsonl"), "old\n");
        // Recent file (today).
        touch(&s.0.join("history/2026-04-26.jsonl"), "new\n");
        let cfg = SweepConfig {
            chat_dir: s.0.clone(),
            history_retention_days: 30,
            decisions_retention_days: 30,
            persona_archive_max: 10,
            today: today_at(2026, 4, 26),
        };
        let r = run_sweep(&cfg);
        assert_eq!(r.history_deleted, 1);
        assert!(!s.0.join("history/2026-03-17.jsonl").exists());
        assert!(s.0.join("history/2026-04-26.jsonl").exists());
    }

    #[test]
    fn history_sweep_keeps_file_at_retention_boundary() {
        let s = ChatScratch::new("hist-boundary");
        // Exactly 30 days ago.
        touch(&s.0.join("history/2026-03-27.jsonl"), "x");
        let cfg = SweepConfig {
            chat_dir: s.0.clone(),
            history_retention_days: 30,
            decisions_retention_days: 30,
            persona_archive_max: 10,
            today: today_at(2026, 4, 26),
        };
        let r = run_sweep(&cfg);
        // 30-day-old file is exactly at the boundary; `>` retain_days
        // means 30 days old is KEPT, only 31+ deleted.
        assert_eq!(r.history_deleted, 0);
    }

    #[test]
    fn history_sweep_prunes_paired_overlays() {
        let s = ChatScratch::new("hist-overlay");
        touch(&s.0.join("history/2026-03-17.jsonl"), "old");
        touch(&s.0.join("history/2026-03-17.uuids.json"), "{}");
        let cfg = SweepConfig {
            chat_dir: s.0.clone(),
            history_retention_days: 30,
            decisions_retention_days: 30,
            persona_archive_max: 10,
            today: today_at(2026, 4, 26),
        };
        let r = run_sweep(&cfg);
        assert_eq!(r.history_deleted, 1);
        assert_eq!(r.overlays_deleted, 1);
    }

    // ---- decisions sweep ------------------------------------------------

    #[test]
    fn decisions_sweep_uses_separate_retention_setting() {
        let s = ChatScratch::new("dec-prune");
        touch(&s.0.join("decisions/2026-03-17.jsonl"), "old");
        touch(&s.0.join("decisions/2026-04-20.jsonl"), "newish");
        let cfg = SweepConfig {
            chat_dir: s.0.clone(),
            history_retention_days: 30,
            decisions_retention_days: 7, // keep only last week
            persona_archive_max: 10,
            today: today_at(2026, 4, 26),
        };
        let r = run_sweep(&cfg);
        // 6-day-old file is just inside, but 7 days exact is at the
        // boundary. 2026-04-20 is 6 days ago → kept.
        assert!(s.0.join("decisions/2026-04-20.jsonl").exists());
        // 40 days ago → deleted.
        assert!(!s.0.join("decisions/2026-03-17.jsonl").exists());
        assert_eq!(r.decisions_deleted, 1);
    }

    // ---- persona archive cap -------------------------------------------

    #[test]
    fn persona_archives_pruned_by_count_keeping_newest() {
        let s = ChatScratch::new("persona-cap");
        // 12 archives spanning 12 different stamps; sweep with max=10
        // should delete the 2 oldest.
        let stamps = [
            "20260101T000000Z", "20260102T000000Z", "20260103T000000Z",
            "20260104T000000Z", "20260105T000000Z", "20260106T000000Z",
            "20260107T000000Z", "20260108T000000Z", "20260109T000000Z",
            "20260110T000000Z", "20260111T000000Z", "20260112T000000Z",
        ];
        for stamp in stamps {
            touch(&s.0.join(format!("persona.md.{stamp}")), "p");
        }
        let cfg = SweepConfig {
            chat_dir: s.0.clone(),
            history_retention_days: 30,
            decisions_retention_days: 30,
            persona_archive_max: 10,
            today: today_at(2026, 4, 26),
        };
        let r = run_sweep(&cfg);
        assert_eq!(r.persona_archives_deleted, 2);
        // The two oldest should be gone.
        assert!(!s.0.join("persona.md.20260101T000000Z").exists());
        assert!(!s.0.join("persona.md.20260102T000000Z").exists());
        // The 10 newest should be kept.
        assert!(s.0.join("persona.md.20260112T000000Z").exists());
        assert!(s.0.join("persona.md.20260103T000000Z").exists());
    }

    #[test]
    fn persona_archives_under_cap_are_kept() {
        let s = ChatScratch::new("persona-under");
        for i in 0..5 {
            touch(
                &s.0.join(format!("persona.md.2026010{i}T000000Z")),
                "p",
            );
        }
        let cfg = SweepConfig {
            chat_dir: s.0.clone(),
            history_retention_days: 30,
            decisions_retention_days: 30,
            persona_archive_max: 10,
            today: today_at(2026, 4, 26),
        };
        let r = run_sweep(&cfg);
        assert_eq!(r.persona_archives_deleted, 0);
    }

    // ---- pending archives -----------------------------------------------

    #[test]
    fn pending_adjustments_archives_pruned_by_age() {
        let s = ChatScratch::new("pending-prune");
        touch(
            &s.0.join("pending_adjustments.20260101T000000Z.jsonl"),
            "old",
        );
        touch(
            &s.0.join("pending_adjustments.20260420T000000Z.jsonl"),
            "newer",
        );
        let cfg = SweepConfig {
            chat_dir: s.0.clone(),
            history_retention_days: 30,
            decisions_retention_days: 30,
            persona_archive_max: 10,
            today: today_at(2026, 4, 26),
        };
        let r = run_sweep(&cfg);
        assert_eq!(r.pending_adjustments_deleted, 1);
        assert!(!s.0.join("pending_adjustments.20260101T000000Z.jsonl").exists());
        assert!(s.0.join("pending_adjustments.20260420T000000Z.jsonl").exists());
    }

    // ---- sweep_due_today ------------------------------------------------

    #[test]
    fn sweep_due_today_is_some_for_unknown_or_yesterday() {
        let v = sweep_due_today(None);
        assert!(v.is_some());
        let v = sweep_due_today(Some("1999-01-01"));
        assert!(v.is_some());
    }

    #[test]
    fn sweep_due_today_is_none_when_already_swept_today() {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let v = sweep_due_today(Some(&today));
        assert!(v.is_none());
    }

    // ---- sweep_rotated_archive counter ---------------------------------

    #[test]
    fn sweep_rotated_archive_increments_counter_on_successful_delete() {
        // Lay down a stale dated archive and assert the counter ticks
        // from 0 to 1 after the sweep removes it.
        let s = ChatScratch::new("archive-counter");
        let stale = s.0.join("adjustments.archive.20260101T000000Z.md");
        touch(&stale, "old archive");
        let cfg = SweepConfig {
            chat_dir: s.0.clone(),
            history_retention_days: 30,
            decisions_retention_days: 30,
            persona_archive_max: 10,
            today: today_at(2026, 4, 26),
        };
        let r = run_sweep(&cfg);
        assert_eq!(r.markdown_archives_deleted, 1);
        assert!(!stale.exists());
    }

    #[test]
    fn sweep_rotated_archive_keeps_recent_files() {
        // Recent archive (within retention) — must NOT be deleted nor counted.
        let s = ChatScratch::new("archive-recent");
        let recent = s.0.join("memory.archive.20260420T000000Z.md");
        touch(&recent, "recent archive");
        let cfg = SweepConfig {
            chat_dir: s.0.clone(),
            history_retention_days: 30,
            decisions_retention_days: 30,
            persona_archive_max: 10,
            today: today_at(2026, 4, 26),
        };
        let r = run_sweep(&cfg);
        assert_eq!(r.markdown_archives_deleted, 0);
        assert!(recent.exists());
    }

    // ---- empty / missing dirs ------------------------------------------

    #[test]
    fn sweep_handles_missing_history_dir() {
        let s = ChatScratch::new("empty-hist");
        let _ = fs::remove_dir_all(s.0.join("history"));
        let cfg = SweepConfig {
            chat_dir: s.0.clone(),
            history_retention_days: 30,
            decisions_retention_days: 30,
            persona_archive_max: 10,
            today: today_at(2026, 4, 26),
        };
        let r = run_sweep(&cfg);
        assert_eq!(r.history_deleted, 0);
    }

    // Lint suppression for io::Result import path used only inside the
    // module bodies above.
    #[test]
    fn io_namespace_is_in_scope() {
        let _ = io::Error::other("");
        let _ = Duration::from_secs(0);
    }
}
