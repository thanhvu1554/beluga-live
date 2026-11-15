//! Token and cost journal: `server/data/usage.jsonl`, one line per tutor turn, utility-model call
//! or exam-ingest job, and the `usage` frame (PROTOCOL section 10).
//!
//! `costUsd` of the Claude plan (`tracked:true`) is the CLI's list-price estimate, not money spent.

use chrono::{DateTime, Duration, Local, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct UsageRec {
    /// RFC 3339, filled by `record` when empty.
    pub ts: String,
    /// `turn` (tutor), `util` (utility model), `ingest` (exam reading).
    pub kind: String,
    pub session_id: Option<String>,
    pub folder_id: Option<String>,
    pub model_id: String,
    pub provider: String,
    /// Claude plan (subscription) usage.
    pub tracked: bool,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub cost_usd: f64,
    pub note: Option<String>,
}

pub struct Usage {
    path: PathBuf,
    lock: Mutex<()>,
    /// Latest `rate_limit_event` of the Claude plan, as the `limit` object.
    limit: Mutex<Option<Value>>,
}

#[derive(Default, Clone, Copy)]
struct Sum {
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    cost: f64,
    turns: u64,
}

impl Sum {
    fn add(&mut self, r: &UsageRec) {
        self.input += r.input_tokens;
        self.output += r.output_tokens;
        self.cache_read += r.cache_read_tokens;
        self.cache_write += r.cache_write_tokens;
        self.cost += r.cost_usd;
        if r.kind == "turn" {
            self.turns += 1;
        }
    }

    fn full(&self) -> Value {
        json!({"inputTokens": self.input, "outputTokens": self.output, "cacheReadTokens": self.cache_read,
               "cacheWriteTokens": self.cache_write, "costUsd": round6(self.cost), "turns": self.turns})
    }
}

fn round6(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

fn day_of(ts: &str) -> Option<NaiveDate> {
    DateTime::parse_from_rfc3339(ts).ok().map(|d| d.with_timezone(&Local).date_naive())
}

impl Usage {
    pub fn open(data_dir: &Path) -> Usage {
        let _ = std::fs::create_dir_all(data_dir);
        Usage { path: data_dir.join("usage.jsonl"), lock: Mutex::new(()), limit: Mutex::new(None) }
    }

    pub fn record(&self, rec: &UsageRec) {
        let mut r = rec.clone();
        if r.ts.is_empty() {
            r.ts = crate::store::now_rfc3339();
        }
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{}", serde_json::to_string(&r).unwrap_or_default());
        }
    }

    /// From a stream-json `rate_limit_event.rate_limit_info`.
    pub fn set_limit(&self, info: &Value) {
        let resets = info["resetsAt"].as_i64().and_then(|s| DateTime::from_timestamp(s, 0)).map(|d| d.with_timezone(&Local).to_rfc3339());
        let mut v = json!({"status": info["status"].as_str().unwrap_or("allowed"), "type": info["rateLimitType"].as_str().unwrap_or("")});
        if let Some(r) = resets {
            v["resetsAt"] = json!(r);
        }
        if let Some(u) = info["unifiedWindows"][info["rateLimitType"].as_str().unwrap_or("")]["utilization"].as_f64() {
            v["utilization"] = json!(u);
        }
        *self.limit.lock().unwrap_or_else(|e| e.into_inner()) = Some(v);
    }

    pub fn records(&self) -> Vec<UsageRec> {
        let _g = self.lock.lock().unwrap_or_else(|e| e.into_inner());
        std::fs::read_to_string(&self.path)
            .map(|t| t.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
            .unwrap_or_default()
    }

    /// The `usage` frame. `session_id`: the phone's current session (for `claude.session`).
    pub fn report(&self, session_id: Option<&str>) -> Value {
        let today = Local::now().date_naive();
        let week_from = today - Duration::days(6);
        let (mut c_sess, mut c_today, mut c_week) = (Sum::default(), Sum::default(), Sum::default());
        let (mut o_today, mut i_today, mut i_total) = (Sum::default(), Sum::default(), Sum::default());
        for r in self.records() {
            let day = day_of(&r.ts);
            let is_today = day == Some(today);
            if r.kind == "ingest" {
                i_total.add(&r);
                if is_today {
                    i_today.add(&r);
                }
            } else if r.tracked {
                if session_id.is_some() && r.session_id.as_deref() == session_id {
                    c_sess.add(&r);
                }
                if is_today {
                    c_today.add(&r);
                }
                if day.map(|d| d >= week_from).unwrap_or(false) {
                    c_week.add(&r);
                }
            } else if is_today {
                o_today.add(&r);
            }
        }
        let mut claude = json!({"session": c_sess.full(), "today": c_today.full(), "week": c_week.full()});
        if let Some(l) = self.limit.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            claude["limit"] = l;
        }
        json!({
            "t": "usage",
            "claude": claude,
            "other": {"today": {"costUsd": round6(o_today.cost), "inputTokens": o_today.input, "outputTokens": o_today.output}},
            "ingest": {"today": {"costUsd": round6(i_today.cost)}, "total": {"costUsd": round6(i_total.cost)}},
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_splits_by_kind_and_window() {
        let d = std::env::temp_dir().join(format!("beluga-usage-{}", uuid::Uuid::new_v4()));
        let u = Usage::open(&d);
        let turn = |sid: &str, tracked: bool, cost: f64| UsageRec {
            kind: "turn".into(), session_id: Some(sid.into()), model_id: "sonnet".into(), tracked,
            input_tokens: 10, output_tokens: 5, cache_read_tokens: 100, cost_usd: cost, ..Default::default()
        };
        u.record(&turn("a", true, 0.5));
        u.record(&turn("b", true, 0.25));
        u.record(&UsageRec { ts: "2020-01-01T00:00:00+00:00".into(), ..turn("a", true, 9.0) });
        u.record(&turn("a", false, 0.01));
        u.record(&UsageRec { kind: "ingest".into(), cost_usd: 0.02, ..Default::default() });
        u.set_limit(&json!({"status":"allowed","resetsAt":1794700800,"rateLimitType":"five_hour","unifiedWindows":{"five_hour":{"utilization":0.48}}}));
        let r = u.report(Some("a"));
        assert_eq!(r["claude"]["session"]["turns"], 2, "includes the old one");
        assert_eq!(r["claude"]["session"]["costUsd"], 9.5);
        assert_eq!(r["claude"]["today"]["costUsd"], 0.75);
        assert_eq!(r["claude"]["week"]["turns"], 2);
        assert_eq!(r["claude"]["today"]["cacheReadTokens"], 200);
        assert_eq!(r["other"]["today"]["costUsd"], 0.01);
        assert_eq!(r["ingest"]["total"]["costUsd"], 0.02);
        assert_eq!(r["claude"]["limit"]["type"], "five_hour");
        assert_eq!(r["claude"]["limit"]["utilization"], 0.48);
        assert!(r["claude"]["limit"]["resetsAt"].as_str().unwrap().starts_with("2026"));
    }
}
