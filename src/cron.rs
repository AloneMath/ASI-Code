//! Cron-style scheduled prompt management.
//!
//! This module handles the persistence + table-management half of the
//! Claude Code-style `CronCreate` / `CronList` / `CronDelete` UX. Jobs are
//! stored at `~/.asi/cron.json` and identified by a short auto-assigned id
//! (`cron-<n>`). The 5-field cron expression is parsed and validated up
//! front so creation fails fast when the schedule is wrong.
//!
//! The actual scheduler thread that fires jobs at their `next_fire` time
//! belongs to the agent daemon and is intentionally not in this file; it
//! consumes the same JSON store. `/cron run-once <id>` is provided here as
//! a manual fallback so users can verify a job's prompt without waiting
//! for the scheduler.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CronJob {
    pub id: String,
    pub cron: String,
    pub prompt: String,
    pub recurring: bool,
    /// Wall-clock seconds (UTC) when the job was last fired. None until
    /// the scheduler has actually run it.
    pub last_fired: Option<u64>,
    /// Wall-clock seconds (UTC) when the job was created. Used for the
    /// recurring-7-day TTL cap that mirrors Claude Code.
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CronStore {
    pub jobs: Vec<CronJob>,
    /// Monotonic id counter so `cron-<n>` ids never get reused after a job
    /// is deleted.
    pub next_id: u64,
}

impl CronStore {
    pub fn load(path: &Path) -> Self {
        if !path.exists() {
            return Self::default();
        }
        match fs::read_to_string(path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| format!("create_dir_all {}: {}", parent.display(), e))?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("serialize cron store: {}", e))?;
        fs::write(path, json).map_err(|e| format!("write {}: {}", path.display(), e))
    }

    pub fn add(&mut self, cron: String, prompt: String, recurring: bool) -> Result<&CronJob, String> {
        // Validate the cron expression up front. We don't keep the parsed
        // form on the job record because re-parsing from text on every load
        // is trivial and keeps the JSON human-editable.
        parse_cron(&cron)?;
        let id = format!("cron-{}", self.next_id + 1);
        self.next_id += 1;
        self.jobs.push(CronJob {
            id,
            cron,
            prompt,
            recurring,
            last_fired: None,
            created_at: now_secs(),
        });
        Ok(self.jobs.last().unwrap())
    }

    pub fn remove(&mut self, id: &str) -> Result<(), String> {
        let before = self.jobs.len();
        self.jobs.retain(|j| j.id != id);
        if self.jobs.len() == before {
            return Err(format!("no cron job with id '{}'", id));
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&CronJob> {
        self.jobs.iter().find(|j| j.id == id)
    }

    /// Drop expired recurring jobs (created more than 7 days ago) and
    /// non-recurring jobs whose last_fired is set. Returns ids that were
    /// removed. Public so the future scheduler thread (in `agentd`) can
    /// reuse it; not yet invoked from the REPL fast path.
    #[allow(dead_code)]
    pub fn purge_expired(&mut self, now: u64) -> Vec<String> {
        const SEVEN_DAYS_SECS: u64 = 7 * 24 * 3600;
        let mut removed = Vec::new();
        self.jobs.retain(|j| {
            if !j.recurring && j.last_fired.is_some() {
                removed.push(j.id.clone());
                return false;
            }
            if j.recurring && now.saturating_sub(j.created_at) > SEVEN_DAYS_SECS {
                removed.push(j.id.clone());
                return false;
            }
            true
        });
        removed
    }
}

/// Parsed 5-field cron expression. Each field is the explicit set of
/// matching values (after expanding wildcards, lists, ranges and steps).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedCron {
    pub minutes: Vec<u8>,    // 0..=59
    pub hours: Vec<u8>,      // 0..=23
    pub days_of_month: Vec<u8>, // 1..=31
    pub months: Vec<u8>,     // 1..=12
    pub days_of_week: Vec<u8>, // 0..=6 (Sun=0)
}

pub fn parse_cron(expr: &str) -> Result<ParsedCron, String> {
    let parts: Vec<&str> = expr.split_whitespace().collect();
    if parts.len() != 5 {
        return Err(format!(
            "expected 5 fields (M H DoM Mon DoW), got {}",
            parts.len()
        ));
    }
    Ok(ParsedCron {
        minutes: parse_field(parts[0], 0, 59, "minute")?,
        hours: parse_field(parts[1], 0, 23, "hour")?,
        days_of_month: parse_field(parts[2], 1, 31, "day-of-month")?,
        months: parse_field(parts[3], 1, 12, "month")?,
        days_of_week: parse_field(parts[4], 0, 6, "day-of-week")?,
    })
}

fn parse_field(field: &str, lo: u8, hi: u8, name: &str) -> Result<Vec<u8>, String> {
    let mut out: Vec<u8> = Vec::new();
    for token in field.split(',') {
        let token = token.trim();
        if token.is_empty() {
            return Err(format!("empty {} token", name));
        }
        // `*/N` or `RANGE/N`
        let (range_part, step) = if let Some((rng, step)) = token.split_once('/') {
            let s: u8 = step
                .parse()
                .map_err(|_| format!("invalid {} step '{}'", name, step))?;
            if s == 0 {
                return Err(format!("{} step must be > 0", name));
            }
            (rng, s)
        } else {
            (token, 1u8)
        };

        let (start, end) = if range_part == "*" {
            (lo, hi)
        } else if let Some((a, b)) = range_part.split_once('-') {
            let a: u8 = a
                .parse()
                .map_err(|_| format!("invalid {} value '{}'", name, a))?;
            let b: u8 = b
                .parse()
                .map_err(|_| format!("invalid {} value '{}'", name, b))?;
            (a, b)
        } else {
            let v: u8 = range_part
                .parse()
                .map_err(|_| format!("invalid {} value '{}'", name, range_part))?;
            (v, v)
        };

        if start < lo || end > hi || end < start {
            return Err(format!(
                "{} value out of range: {}..{} (allowed {}..{})",
                name, start, end, lo, hi
            ));
        }
        let mut v = start;
        while v <= end {
            if !out.contains(&v) {
                out.push(v);
            }
            v = match v.checked_add(step) {
                Some(n) => n,
                None => break,
            };
        }
    }
    out.sort_unstable();
    Ok(out)
}

fn now_secs() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Default location for the cron store: `~/.asi/cron.json`.
pub fn default_store_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)?;
    Some(home.join(".asi").join("cron.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn temp_path() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("asi-cron-test-{}-{}.json", nanos, id))
    }

    #[test]
    fn parses_wildcard_and_steps() {
        let c = parse_cron("*/15 * * * *").unwrap();
        assert_eq!(c.minutes, vec![0, 15, 30, 45]);
        assert_eq!(c.hours.len(), 24);
    }

    #[test]
    fn parses_lists_and_ranges() {
        let c = parse_cron("0 9-17 * * 1-5").unwrap();
        assert_eq!(c.minutes, vec![0]);
        assert_eq!(c.hours, (9..=17).collect::<Vec<u8>>());
        assert_eq!(c.days_of_week, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn rejects_wrong_field_count() {
        assert!(parse_cron("0 9 * *").is_err());
        assert!(parse_cron("0 9 * * * *").is_err());
    }

    #[test]
    fn rejects_out_of_range_values() {
        assert!(parse_cron("60 * * * *").is_err());
        assert!(parse_cron("* 24 * * *").is_err());
        assert!(parse_cron("* * 32 * *").is_err());
        assert!(parse_cron("* * * 13 *").is_err());
        assert!(parse_cron("* * * * 7").is_err());
    }

    #[test]
    fn rejects_zero_step_and_inverted_range() {
        assert!(parse_cron("*/0 * * * *").is_err());
        assert!(parse_cron("5-3 * * * *").is_err());
    }

    #[test]
    fn store_add_remove_persist_roundtrip() {
        let path = temp_path();
        let mut store = CronStore::load(&path);
        let job = store
            .add("*/5 * * * *".to_string(), "ping".to_string(), true)
            .unwrap();
        let id = job.id.clone();
        store.save(&path).unwrap();

        let reloaded = CronStore::load(&path);
        assert_eq!(reloaded.jobs.len(), 1);
        assert_eq!(reloaded.jobs[0].id, id);

        let mut store2 = reloaded;
        store2.remove(&id).unwrap();
        store2.save(&path).unwrap();

        let final_load = CronStore::load(&path);
        assert!(final_load.jobs.is_empty());

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn purge_expired_drops_old_recurring_and_fired_oneshots() {
        let mut store = CronStore::default();
        // Recurring, created 8 days ago.
        store.jobs.push(CronJob {
            id: "cron-1".to_string(),
            cron: "*/5 * * * *".to_string(),
            prompt: "old".to_string(),
            recurring: true,
            last_fired: None,
            created_at: 1_000_000,
        });
        // One-shot, already fired.
        store.jobs.push(CronJob {
            id: "cron-2".to_string(),
            cron: "0 9 * * *".to_string(),
            prompt: "done".to_string(),
            recurring: false,
            last_fired: Some(1_500_000),
            created_at: 1_400_000,
        });
        // Recurring, recently created.
        store.jobs.push(CronJob {
            id: "cron-3".to_string(),
            cron: "0 * * * *".to_string(),
            prompt: "fresh".to_string(),
            recurring: true,
            last_fired: None,
            created_at: 1_500_000,
        });
        // Now: 1_000_000 + 9 days
        let now = 1_000_000 + 9 * 24 * 3600;
        let removed = store.purge_expired(now);
        assert_eq!(removed.len(), 2);
        assert!(removed.contains(&"cron-1".to_string()));
        assert!(removed.contains(&"cron-2".to_string()));
        assert_eq!(store.jobs.len(), 1);
        assert_eq!(store.jobs[0].id, "cron-3");
    }

    #[test]
    fn add_rejects_invalid_cron_expression_up_front() {
        let path = temp_path();
        let mut store = CronStore::load(&path);
        let err = store
            .add("not a cron".to_string(), "x".to_string(), true)
            .unwrap_err();
        assert!(err.contains("5 fields"));
        assert!(store.jobs.is_empty());
    }
}
