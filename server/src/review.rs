//! Folder review (PROTOCOL section 12): the `review` frame, the weekly summary and the text of
//! the review-quiz kickoff turn.

use crate::folders::curriculum_topics;
use crate::memory::{self, topic_key, Entry, Memory, Tag};
use crate::session::Deps;
use crate::util_llm::{self, UtilReq};
use chrono::{Duration, NaiveDate};
use serde_json::{json, Value};

fn fmt(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// State of one topic from the folder memory entries that match it.
fn topic_state(entries: &[Entry]) -> (String, u32, u32, Option<NaiveDate>) {
    let last = entries.iter().map(|e| e.last).max();
    if let Some(w) = entries.iter().find(|e| e.tag == Tag::Weak) {
        return ("weak".into(), w.count, w.streak, last);
    }
    if entries.iter().any(|e| matches!(e.tag, Tag::Mastered | Tag::Understood)) {
        return ("solid".into(), 0, 1, last);
    }
    if entries.iter().any(|e| e.tag == Tag::Learned) {
        return ("learning".into(), 0, 0, last);
    }
    ("new".into(), 0, 0, None)
}

/// The `review` frame of a folder.
pub fn review_frame(deps: &Deps, folder_id: &str) -> Value {
    let Some(folder) = deps.folders.get(folder_id) else {
        return json!({"t": "error", "code": "no_folder", "text": "Không tìm thấy folder.", "fatal": false});
    };
    let today = memory::today();
    let file = Memory::new(&deps.cfg.tutor_dir).folder_file(folder_id);
    let entries: Vec<Entry> = file.entries().into_iter().map(|(_, e)| e).filter(|e| e.tag != Tag::Style).collect();
    let mut topics = Vec::new();
    let knowledge = deps.folders.knowledge(folder_id);
    let curriculum = knowledge.as_ref().map(curriculum_topics).unwrap_or_default();
    if !curriculum.is_empty() {
        for (id, title, chapter) in &curriculum {
            let m: Vec<Entry> = entries.iter().filter(|e| e.topic_id.as_deref() == Some(id.as_str()) || topic_key(&e.topic) == topic_key(title)).cloned().collect();
            let (state, wrong, right, last) = topic_state(&m);
            topics.push(json!({"topicId": id, "title": title, "chapter": chapter, "state": state, "wrong": wrong, "right": right, "lastSeen": last.map(fmt)}));
        }
    } else {
        let mut keys: Vec<String> = Vec::new();
        for e in &entries {
            let k = topic_key(&e.topic);
            if !keys.contains(&k) {
                keys.push(k);
            }
        }
        for k in keys {
            let m: Vec<Entry> = entries.iter().filter(|e| topic_key(&e.topic) == k).cloned().collect();
            let (state, wrong, right, last) = topic_state(&m);
            let id = m.iter().find_map(|e| e.topic_id.clone()).unwrap_or_else(|| m[0].topic.clone());
            topics.push(json!({"topicId": id, "title": m[0].topic, "chapter": "", "state": state, "wrong": wrong, "right": right, "lastSeen": last.map(fmt)}));
        }
    }
    let mut weak_items = Memory::new(&deps.cfg.tutor_dir).folder_weak(folder_id, &folder.kind);
    weak_items.sort_by(|a, b| a.due.cmp(&b.due).then(b.count.cmp(&a.count)));
    let weak: Vec<Value> = weak_items
        .iter()
        .map(|w| json!({"topic": w.topic, "note": w.note, "count": w.count, "lastSeen": fmt(w.last), "due": fmt(w.due)}))
        .collect();
    let due: Vec<Value> = weak_items.iter().filter(|w| w.due <= today).map(|w| json!({"topic": w.topic, "note": w.note, "due": fmt(w.due)})).collect();
    let sessions: Vec<Value> = deps
        .store
        .list_sessions()
        .into_iter()
        .filter(|s| s.folder_id.as_deref() == Some(folder_id))
        .map(|s| {
            let steps = deps.store.load_events(&s.session_id).iter().filter(|e| e["t"] == "step").count();
            json!({"sessionId": s.session_id, "title": s.title, "updatedAt": s.updated_at, "mode": s.mode, "steps": steps})
        })
        .collect();
    let weekly = deps.folders.weekly(folder_id).map(|w| json!({"from": w["from"], "to": w["to"], "md": w["md"]}));
    json!({"t": "review", "folderId": folder_id, "topics": topics, "weak": weak, "due": due,
           "weekly": weekly, "sessions": sessions, "updatedAt": crate::store::now_rfc3339()})
}

const WEEKLY_SYSTEM: &str = "Bạn viết tóm tắt tuần học cho một học sinh, đọc trên điện thoại. Tiếng Việt, markdown, tối đa \
12 dòng, ba phần có tiêu đề nhỏ: **Tuần này học gì**, **Còn sai ở đâu**, **Nên ôn gì tiếp**. Xưng tớ gọi cậu, ngắn, cụ thể, \
không khen sáo rỗng, không bịa ngoài dữ liệu.";

/// Write (and save) the folder's weekly summary with the utility model, then return the new
/// `review` frame. Without a utility model a plain summary is built from the same data.
pub async fn weekly(deps: &Deps, folder_id: &str) -> Value {
    let Some(folder) = deps.folders.get(folder_id) else {
        return json!({"t": "error", "code": "no_folder", "text": "Không tìm thấy folder.", "fatal": false});
    };
    let today = memory::today();
    let from = today - Duration::days(6);
    let file = Memory::new(&deps.cfg.tutor_dir).folder_file(folder_id);
    let recent: Vec<Entry> = file.entries().into_iter().map(|(_, e)| e).filter(|e| e.last >= from && e.tag != Tag::Style).collect();
    let line = |e: &Entry| {
        let what = match e.tag {
            Tag::Weak => format!("SAI {} lần", e.count),
            Tag::Mastered => "đã vững".into(),
            Tag::Learned => "đã học".into(),
            Tag::Understood => "hiểu sau khi giảng lại".into(),
            Tag::Style => String::new(),
        };
        if e.note.is_empty() {
            format!("- {} ({what})", e.topic)
        } else {
            format!("- {} ({what}): {}", e.topic, e.note)
        }
    };
    let titles: Vec<String> = deps
        .store
        .list_sessions()
        .into_iter()
        .filter(|s| s.folder_id.as_deref() == Some(folder_id))
        .filter(|s| chrono::DateTime::parse_from_rfc3339(&s.updated_at).map(|d| d.date_naive() >= from).unwrap_or(false))
        .map(|s| s.title)
        .take(20)
        .collect();
    let due: Vec<String> = Memory::new(&deps.cfg.tutor_dir).folder_weak(folder_id, &folder.kind).into_iter().filter(|w| w.due <= today + Duration::days(2)).map(|w| w.topic).collect();
    let data = format!(
        "Folder: {} (môn {})\nTừ {} tới {}\nPhiên học trong tuần: {}\nGhi nhớ trong tuần:\n{}\nSắp tới hạn ôn: {}",
        folder.name,
        memory::subject_name(&folder.kind),
        fmt(from),
        fmt(today),
        if titles.is_empty() { "không có".to_string() } else { titles.join("; ") },
        if recent.is_empty() { "- (chưa có)".to_string() } else { recent.iter().map(line).collect::<Vec<_>>().join("\n") },
        if due.is_empty() { "không có".to_string() } else { due.join("; ") },
    );
    let md = match util_llm::ask(&deps.util, &deps.usage, UtilReq::new("weekly", WEEKLY_SYSTEM, data.clone(), 700), std::time::Duration::from_secs(30)).await {
        Ok(md) => md.trim().to_string(),
        Err(e) => {
            tracing::info!("weekly summary without utility model: {e}");
            let weak: Vec<String> = recent.iter().filter(|e| e.tag == Tag::Weak).map(line).collect();
            let learned: Vec<String> = recent.iter().filter(|e| e.tag != Tag::Weak).map(line).collect();
            format!(
                "**Tuần này học gì**\n{}\n\n**Còn sai ở đâu**\n{}\n\n**Nên ôn gì tiếp**\n{}",
                if learned.is_empty() && titles.is_empty() { "- Chưa có buổi nào.".to_string() } else if learned.is_empty() { titles.iter().map(|t| format!("- {t}")).collect::<Vec<_>>().join("\n") } else { learned.join("\n") },
                if weak.is_empty() { "- Chưa ghi nhận lỗi nào.".to_string() } else { weak.join("\n") },
                if due.is_empty() { "- Chưa có mục tới hạn.".to_string() } else { due.iter().map(|t| format!("- {t}")).collect::<Vec<_>>().join("\n") },
            )
        }
    };
    deps.folders.save_weekly(folder_id, &json!({"from": fmt(from), "to": fmt(today), "md": md, "createdAt": crate::store::now_rfc3339()}));
    review_frame(deps, folder_id)
}

/// First turn of `new_session{kickoff:"review_quiz"}`: what is due and weak in the folder, and
/// nothing from anywhere else (PROTOCOL section 18; the quiz always has a folder).
pub fn quiz_kickoff(deps: &Deps, folder: Option<&str>) -> String {
    let today = memory::today();
    let mem = Memory::new(&deps.cfg.tutor_dir);
    let mut items = match folder.and_then(|f| deps.folders.get(f)) {
        Some(f) => mem.folder_weak(&f.id, &f.kind),
        None => Vec::new(),
    };
    items.sort_by(|a, b| a.due.cmp(&b.due).then(b.count.cmp(&a.count)));
    let parts: Vec<String> = items
        .iter()
        .take(6)
        .map(|w| {
            let due = if w.due <= today { ", tới hạn" } else { "" };
            let note = if w.note.is_empty() { String::new() } else { format!(": {}", w.note) };
            format!("{} · {} (sai {}{due}){note}", memory::subject_name(&w.subject), w.topic, w.count)
        })
        .collect();
    if parts.is_empty() {
        "[ôn nhanh: chưa có mục nào đang yếu; hỏi bạn ấy muốn ôn phần nào, hoặc ra một câu về chủ đề đang học trên trường]".to_string()
    } else {
        format!("[ôn nhanh: {}]", parts.join("; "))
    }
}
