//! Home-screen greeting (PROTOCOL sections 15, 18): one short line from the utility model, cached
//! for 3 hours (and per part of the day and per folder), with a built-in fallback list. `sub` is
//! computed fresh every time and always names the folder it talks about.

use crate::memory::{self, Memory};
use crate::session::Deps;
use crate::util_llm::{self, UtilReq};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const DOODLES: &[&str] = &["cat_walk", "cat_sleep", "whale_swim", "bird_hop", "snail", "paper_plane"];
const CACHE_FOR: Duration = Duration::from_secs(3 * 3600);

#[derive(Default)]
pub struct GreetingCache {
    /// key (part of day | name | folder) -> (made at, greeting without `sub`).
    slots: Mutex<HashMap<String, (Instant, Value)>>,
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

/// "Có 2 mục tới hạn ôn trong Tiếng Anh trên lớp" (deterministic, from the folder memory files).
/// With `folder` only that folder is looked at; without, the folder with the most due items.
/// Review items outside folders are never suggested (no self-initiated review there).
fn suggestion(deps: &Deps, folder: Option<&str>) -> Option<String> {
    let mem = Memory::new(&deps.cfg.tutor_dir);
    let today = memory::today();
    let mut best: Option<(usize, String)> = None;
    for f in deps.folders.list().into_iter().filter(|f| folder.map(|id| id == f.id).unwrap_or(true)) {
        let n = mem.folder_weak(&f.id, &f.kind).iter().filter(|w| w.due <= today).count();
        if n > 0 && best.as_ref().map(|(b, _)| n > *b).unwrap_or(true) {
            best = Some((n, f.name));
        }
    }
    best.map(|(n, name)| format!("Có {n} mục tới hạn ôn trong {name}"))
}

const SYSTEM: &str = "Bạn viết MỘT câu chào ngắn (tối đa 60 ký tự) cho màn hình chính của app gia sư, xưng tớ gọi cậu, \
dễ thương, hợp với giờ trong ngày và tình hình học. Chọn một hình vẽ: cat_walk, cat_sleep, whale_swim, bird_hop, snail, \
paper_plane. Trả về đúng JSON một dòng: {\"text\":\"...\",\"doodle\":\"...\"}";

/// `folder`: the greeting of a chat opened inside that folder: `sub` and everything the utility
/// model sees come from that folder only. An unknown folder id gets a greeting with nothing in it.
pub async fn greeting(deps: &Deps, cache: &GreetingCache, local_time: &str, name: Option<&str>, folder: Option<&str>) -> Value {
    let part = part_of_day(local_time);
    let folder = folder.filter(|f| !f.is_empty());
    let known = folder.map(|f| deps.folders.get(f).is_some()).unwrap_or(true);
    let sub = if known { suggestion(deps, folder) } else { None };
    let with_sub = |mut v: Value| {
        if let Some(s) = &sub {
            v["sub"] = json!(s);
        }
        v
    };
    let key = format!("{part}|{}|{}", name.unwrap_or(""), folder.unwrap_or(""));
    if let Some((at, v)) = cache.slots.lock().unwrap_or_else(|e| e.into_inner()).get(&key).cloned() {
        if at.elapsed() < CACHE_FOR {
            return with_sub(v);
        }
    }
    let recent: Vec<String> = if known {
        deps.store.list_sessions().into_iter().filter(|s| folder.map(|f| s.folder_id.as_deref() == Some(f)).unwrap_or(true)).take(3).map(|s| s.title).collect()
    } else {
        Vec::new()
    };
    let prompt = format!(
        "Giờ địa phương: {local_time}\nTên (có thể trống): {}\nViệc nên làm: {}\nPhiên gần đây: {}",
        name.unwrap_or(""),
        sub.as_deref().unwrap_or("không có gì tới hạn"),
        if recent.is_empty() { "chưa có".into() } else { recent.join("; ") }
    );
    let v = match util_llm::ask(&deps.util, &deps.usage, UtilReq::new("greeting", SYSTEM, prompt, 80), Duration::from_secs(6)).await {
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
    {
        let mut slots = cache.slots.lock().unwrap_or_else(|e| e.into_inner());
        slots.retain(|_, (at, _)| at.elapsed() < CACHE_FOR);
        slots.insert(key, (Instant::now(), v.clone()));
    }
    with_sub(v)
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
