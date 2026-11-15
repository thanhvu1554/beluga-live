//! Home-screen greeting (PROTOCOL section 15): one short line from the utility model, cached
//! for 3 hours (and per part of the day), with a built-in fallback list.

use crate::memory::{self, Memory};
use crate::session::Deps;
use crate::util_llm::{self, UtilReq};
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const DOODLES: &[&str] = &["cat_walk", "cat_sleep", "whale_swim", "bird_hop", "snail", "paper_plane"];
const CACHE_FOR: Duration = Duration::from_secs(3 * 3600);

#[derive(Default)]
pub struct GreetingCache {
    slot: Mutex<Option<(Instant, String, Value)>>,
}

/// `khuya` (0-4h), `sang` (5-10h), `trua` (11-13h), `chieu` (14-17h), `toi` (18-23h).
pub fn part_of_day(local_time: &str) -> &'static str {
    let hour: u32 = local_time.split('T').nth(1).and_then(|t| t.get(0..2)).and_then(|h| h.parse().ok()).unwrap_or(12);
    match hour {
        0..=4 => "khuya",
        5..=10 => "sang",
        11..=13 => "trua",
        14..=17 => "chieu",
        _ => "toi",
    }
}

/// Built-in lines per part of the day (also what the phone shows offline).
pub fn fallback(part: &str, name: Option<&str>) -> Value {
    let who = name.map(|n| format!(" {n}")).unwrap_or_default();
    let (text, doodle) = match part {
        "khuya" => (format!("Khuya rồi{who}, học nhẹ thôi rồi ngủ nhé"), "cat_sleep"),
        "sang" => (format!("Chào buổi sáng{who}, hôm nay học gì nào?"), "bird_hop"),
        "trua" => (format!("Trưa rồi{who}, nghỉ chút rồi học tiếp nha"), "snail"),
        "chieu" => (format!("Chiều nay{who} muốn ôn phần nào?"), "paper_plane"),
        _ => (format!("Tối vui{who}, làm vài bài cùng tớ không?"), "whale_swim"),
    };
    json!({"t": "greeting", "text": text, "doodle": doodle})
}

/// "Có 2 mục Toán tới hạn ôn" (deterministic, from memory).
fn suggestion(deps: &Deps) -> Option<String> {
    let due = Memory::new(&deps.cfg.tutor_dir).due_items(memory::today());
    let mut by_subject: Vec<(String, usize)> = Vec::new();
    for d in &due {
        match by_subject.iter_mut().find(|(s, _)| *s == d.subject) {
            Some(x) => x.1 += 1,
            None => by_subject.push((d.subject.clone(), 1)),
        }
    }
    let mut folder_due = 0;
    for f in deps.folders.list() {
        folder_due += Memory::new(&deps.cfg.tutor_dir).folder_weak(&f.id, &f.subject).iter().filter(|w| w.due <= memory::today()).count();
    }
    by_subject.sort_by(|a, b| b.1.cmp(&a.1));
    match by_subject.first() {
        Some((s, n)) => Some(format!("Có {n} mục {} tới hạn ôn", memory::subject_name(s))),
        None if folder_due > 0 => Some(format!("Có {folder_due} mục tới hạn ôn")),
        None => None,
    }
}

const SYSTEM: &str = "Bạn viết MỘT câu chào ngắn (tối đa 60 ký tự) cho màn hình chính của app gia sư, xưng tớ gọi cậu, \
dễ thương, hợp với giờ trong ngày và tình hình học. Chọn một hình vẽ: cat_walk, cat_sleep, whale_swim, bird_hop, snail, \
paper_plane. Trả về đúng JSON một dòng: {\"text\":\"...\",\"doodle\":\"...\"}";

pub async fn greeting(deps: &Deps, cache: &GreetingCache, local_time: &str, name: Option<&str>) -> Value {
    let part = part_of_day(local_time);
    let key = format!("{part}|{}", name.unwrap_or(""));
    if let Some((at, k, v)) = cache.slot.lock().unwrap_or_else(|e| e.into_inner()).clone() {
        if k == key && at.elapsed() < CACHE_FOR {
            return v;
        }
    }
    let sub = suggestion(deps);
    let recent: Vec<String> = deps.store.list_sessions().into_iter().take(3).map(|s| s.title).collect();
    let prompt = format!(
        "Giờ địa phương: {local_time}\nTên (có thể trống): {}\nViệc nên làm: {}\nPhiên gần đây: {}",
        name.unwrap_or(""),
        sub.as_deref().unwrap_or("không có gì tới hạn"),
        if recent.is_empty() { "chưa có".into() } else { recent.join("; ") }
    );
    let mut v = match util_llm::ask(&deps.util, &deps.usage, UtilReq::new("greeting", SYSTEM, prompt, 80), Duration::from_secs(6)).await {
        Ok(t) => match util_llm::extract_json(&t) {
            Some(j) if j["text"].as_str().map(|s| !s.trim().is_empty() && s.chars().count() <= 80).unwrap_or(false) => {
                let doodle = j["doodle"].as_str().filter(|d| DOODLES.contains(d)).unwrap_or("cat_walk");
                json!({"t": "greeting", "text": j["text"].as_str().unwrap().trim(), "doodle": doodle})
            }
            _ => fallback(part, name),
        },
        Err(e) => {
            tracing::info!("greeting fallback: {e}");
            fallback(part, name)
        }
    };
    if let Some(s) = sub {
        v["sub"] = json!(s);
    }
    *cache.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some((Instant::now(), key, v.clone()));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_and_fallbacks() {
        assert_eq!(part_of_day("2026-11-15T01:45"), "khuya");
        assert_eq!(part_of_day("2026-11-15T07:00"), "sang");
        assert_eq!(part_of_day("2026-11-15T12:10"), "trua");
        assert_eq!(part_of_day("2026-11-15T15:00"), "chieu");
        assert_eq!(part_of_day("2026-11-15T21:00"), "toi");
        assert_eq!(part_of_day("garbage"), "trua");
        for p in ["khuya", "sang", "trua", "chieu", "toi"] {
            let v = fallback(p, Some("Minh"));
            assert!(v["text"].as_str().unwrap().chars().count() <= 60);
            assert!(DOODLES.contains(&v["doodle"].as_str().unwrap()));
        }
    }
}
