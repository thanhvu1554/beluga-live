//! Study engine (exam matrix) with a fake LLM: page units, cache, retry and escalation, cancel and
//! resume, re-ingest merge with stable ids, provider failover, schema, exams.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use beluga_server::study::exam::{self, render_html_with_base};
use beluga_server::study::ingest::{validate_knowledge, HAIKU};
use beluga_server::study::llm::{FakeLlm, Llm, LlmError, LlmRequest, Part, Providers};
use beluga_server::study::pages::plan_pages;
use beluga_server::study::{FileKind, IngestFile, IngestRequest, StudyConfig, StudyService};
use serde_json::{json, Value};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("beluga-study-{name}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// JPEG-ish bytes that carry the photo name (so every photo is a different page for the cache).
fn images(dir: &Path, prefix: &str, n: usize) -> Vec<IngestFile> {
    (1..=n)
        .map(|i| {
            let p = dir.join(format!("{prefix}{i}.jpg"));
            let mut b = vec![0xFF, 0xD8, 0xFF];
            b.extend(format!("{prefix}{i}").bytes());
            std::fs::write(&p, b).unwrap();
            IngestFile { file_id: format!("{prefix}{i}"), path: p, kind: FileKind::Image, name: format!("{prefix}{i}.jpg") }
        })
        .collect()
}

fn text_of(req: &LlmRequest) -> String {
    req.parts.iter().filter_map(|p| if let Part::Text(t) = p { Some(t.as_str()) } else { None }).collect::<Vec<_>>().join("\n")
}

/// The photo name inside the fake image bytes.
fn photo(req: &LlmRequest) -> String {
    req.parts
        .iter()
        .find_map(|p| if let Part::Image { data, .. } = p { Some(String::from_utf8_lossy(&data[3..]).to_string()) } else { None })
        .unwrap_or_default()
}

fn page_json(title: &str, continues: bool, qs: Vec<Value>) -> String {
    json!({"doc": {"title": title, "kind": "exam", "school": "THCS Nguyễn Du", "date": "", "grade": "9", "minutes": 90, "points": 0,
        "fullExam": true, "continues": continues}, "empty": false,
        "sections": [{"label": "I", "title": "Trắc nghiệm", "kind": "mcq", "note": "Chọn đáp án đúng."},
                     {"label": "II", "title": "Tự luận", "kind": "essay", "note": ""}],
        "questions": qs})
    .to_string()
}

fn item(sec: &str, n: &str, kind: &str, level: &str, topic: &str, form: &str, text: &str) -> Value {
    json!({"sec": sec, "n": n, "kind": kind, "level": level, "pts": if kind == "essay" { 1.5 } else { 0.25 },
        "chapter": "Căn bậc hai, căn bậc ba", "topic": topic, "form": form, "text": text, "fig": false, "ans": ""})
}

/// Page reply: one full exam per photo, 2 MCQ + 1 essay, topics depend on `variant`.
fn extract_reply(req: &LlmRequest, variant: &str) -> String {
    let p = photo(req);
    let qs: Vec<Value> = (0..3)
        .map(|j| {
            let essay = j == 2;
            item(
                if essay { "II" } else { "I" },
                &(if essay { 1 } else { j + 1 }).to_string(),
                if essay { "essay" } else { "mcq" },
                ["nb", "th", "vd"][j],
                &if j == 0 { "Căn bậc hai".to_string() } else { format!("Biến đổi căn thức {variant}") },
                &if j == 0 { "Tìm điều kiện xác định".to_string() } else { format!("Rút gọn biểu thức {variant}{j}") },
                &format!("Câu {p}-{j}: rút gọn $\\sqrt{{{}}}$", 10 + j),
            )
        })
        .collect();
    page_json(&format!("Đề ôn {p}"), false, qs)
}

/// Merge reply: one chapter, a topic per distinct raw topic, one form per raw.
fn merge_reply(req: &LlmRequest) -> String {
    let t = text_of(req);
    let raw_part = t.split("RAW (").nth(1).unwrap_or("");
    let mut topics: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for line in raw_part.lines().skip(1) {
        let cols: Vec<&str> = line.split(" | ").collect();
        if cols.len() < 2 {
            continue;
        }
        let path: Vec<&str> = cols[1].split(" > ").collect();
        let (topic, form) = (path[1].to_string(), path[2].to_string());
        match topics.iter_mut().find(|x| x.0 == topic) {
            Some(x) => x.1.push((form, cols[0].to_string())),
            None => topics.push((topic, vec![(form, cols[0].to_string())])),
        }
    }
    json!({"chapters": [{"id": "", "title": "Căn bậc hai, căn bậc ba", "topics": topics.iter().map(|(t, fs)| json!({
        "id": "", "title": t, "summary": "Tóm tắt", "forms": fs.iter().map(|(f, r)| json!({"id": "", "title": f, "method": "1. Bước một", "raw": [r]})).collect::<Vec<_>>()
    })).collect::<Vec<_>>()}]})
    .to_string()
}

/// Exam reply from the BLUEPRINT lines; `skip` drops one slot (to trigger a repair).
fn exam_reply(req: &LlmRequest, skip: Option<usize>) -> String {
    let t = text_of(req);
    let bp = t.split("BLUEPRINT (").nth(1).unwrap_or("");
    let mut items = Vec::new();
    for line in bp.lines().skip(1) {
        let cols: Vec<&str> = line.split(" | ").collect();
        let Ok(slot) = cols[0].trim().parse::<usize>() else { continue };
        if Some(slot) == skip {
            continue;
        }
        let kind = cols[2];
        items.push(json!({
            "slot": slot, "text": format!("Câu mới số {slot}: tính $\\sqrt{{{}}}$", slot * slot),
            "choices": if kind == "mcq" { json!([format!("${}$", slot + 10), "$1$", "$2$", "$3$"]) } else { json!([]) },
            "statements": [], "answer": if kind == "mcq" { "A" } else { "Kết quả" },
            "solution": format!("Lời giải chi tiết cho câu {slot}: $\\sqrt{{{0}}}={1}$", slot * slot, slot)
        }));
    }
    json!({"title": "Đề luyện giữa kỳ I – Toán 9", "questions": items}).to_string()
}

fn cfg() -> StudyConfig {
    let mut c = StudyConfig::default();
    c.ingest.retry_base_ms = 1;
    c.ingest.concurrency = 2;
    c
}

fn standard_fake(variant: &'static str) -> FakeLlm {
    FakeLlm::new(move |req, _n| match req.tag {
        "extract" => Ok(extract_reply(req, variant)),
        "merge" => Ok(merge_reply(req)),
        _ => Ok(exam_reply(req, None)),
    })
}

async fn drain(mut h: beluga_server::study::IngestHandle) -> (Vec<Value>, Result<Value, beluga_server::study::IngestError>) {
    let mut evs = Vec::new();
    while let Some(e) = h.events.recv().await {
        evs.push(e);
    }
    (evs, h.task.await.unwrap())
}

fn req(folder: &str, files: Vec<IngestFile>) -> IngestRequest {
    IngestRequest { folder_id: folder.into(), subject: "toan".into(), goal: Some("ôn giữa kỳ 1".into()), files }
}

fn notes(evs: &[Value]) -> Vec<String> {
    evs.iter().map(|e| e["note"].as_str().unwrap_or("").to_string()).collect()
}

#[test]
fn every_image_and_every_pdf_page_is_its_own_unit() {
    let d = tmp("plan");
    let mut files = images(&d, "img", 5);
    let pdf = d.join("long.pdf");
    std::fs::write(&pdf, make_pdf(9)).unwrap();
    files.push(IngestFile { file_id: "pdf1".into(), path: pdf, kind: FileKind::Pdf, name: "long.pdf".into() });
    files.extend(images(&d, "late", 1));
    let u = plan_pages(&files).unwrap();
    assert_eq!(u.len(), 15, "5 photos + 9 PDF pages + 1 photo");
    assert_eq!(u[5].first_page, 6);
    assert_eq!(u[13].pages[0].page_in_file, 9);
    for x in &u[5..14] {
        let Part::Pdf { data, .. } = &x.part else { panic!("pdf part") };
        assert_eq!(beluga_server::study::ingest::pdf_page_count(data), 1);
    }
    let shas: std::collections::HashSet<&String> = u.iter().map(|x| &x.sha).collect();
    assert_eq!(shas.len(), 15, "content-addressed: every page has its own key");
}

fn make_pdf(pages: usize) -> Vec<u8> {
    use lopdf::{dictionary, Document, Object, Stream};
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut kids = Vec::new();
    for i in 0..pages {
        let content = Stream::new(dictionary! {}, format!("BT /F1 12 Tf 50 700 Td (Page {i}) Tj ET").into_bytes());
        let cid = doc.add_object(content);
        let page = doc.add_object(dictionary! {"Type" => "Page", "Parent" => pages_id, "Contents" => cid,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()]});
        kids.push(Object::Reference(page));
    }
    doc.objects.insert(pages_id, Object::Dictionary(dictionary! {"Type" => "Pages", "Kids" => kids, "Count" => pages as i64}));
    let catalog = doc.add_object(dictionary! {"Type" => "Catalog", "Pages" => pages_id});
    doc.trailer.set("Root", catalog);
    let mut buf = Vec::new();
    doc.save_to(&mut buf).unwrap();
    buf
}

#[tokio::test]
async fn ingest_end_to_end_with_fake_writes_valid_knowledge() {
    let d = tmp("e2e");
    let fake = Arc::new(standard_fake("A"));
    let svc = StudyService::with_llm(d.join("data"), cfg(), fake.clone());
    let h = svc.start_ingest(req("f1", images(&d, "a", 5))).unwrap();
    let (evs, r) = drain(h).await;
    let k = r.unwrap();
    let states: Vec<&str> = evs.iter().map(|e| e["state"].as_str().unwrap()).collect();
    assert_eq!(states.first(), Some(&"queued"));
    assert!(states.contains(&"reading") && states.contains(&"merging"));
    assert_eq!(states.last(), Some(&"done"));
    for e in &evs {
        assert_eq!(e["t"], "ingest");
        for f in ["folderId", "jobId", "state", "done", "total", "note"] {
            assert!(e.get(f).is_some(), "{f} in {e}");
        }
    }
    let last = evs.last().unwrap();
    assert_eq!((last["done"].as_u64(), last["total"].as_u64()), (Some(5), Some(5)));
    assert_eq!(last["found"]["questions"], 15);
    assert!(last["costUsd"].as_f64().unwrap() > 0.0);
    assert!(notes(&evs).iter().any(|n| n.starts_with("Đã đọc trang ") && n.contains("a1.jpg") && n.contains("3 câu")), "{evs:?}");
    assert_eq!(fake.calls_with_tag("extract"), 5, "one call per page, no escalation for clean pages");
    assert_eq!(fake.calls_with_model("extract", HAIKU), 0);
    assert_eq!(fake.calls_with_tag("merge"), 1);
    let merge_prompt = text_of(&fake.calls.lock().unwrap().iter().find(|r| r.tag == "merge").unwrap().clone());
    assert!(merge_prompt.contains("Place ALL 3 RAW ids"), "{merge_prompt}");

    assert!(validate_knowledge(&k).is_empty(), "{:?}", validate_knowledge(&k));
    assert_eq!(svc.knowledge("f1").unwrap(), k);
    assert_eq!(k["stats"]["questions"], 15);
    assert_eq!(k["grade"], "9");
    assert_eq!(k["stats"]["byLevel"], json!({"nb": 5, "th": 5, "vd": 5, "vdc": 0}));
    assert_eq!(k["sources"].as_array().unwrap().len(), 5, "every photo has its own header here");
    // Progress is computed: every topic seen is covered; current = furthest topic of the newest document.
    assert_eq!(k["schoolProgress"]["coveredTopicIds"], json!(["c1.t1", "c1.t2"]));
    assert_eq!(k["schoolProgress"]["currentTopicId"], "c1.t2");
    let s = &k["structures"][0];
    assert_eq!(k["structures"].as_array().unwrap().len(), 1, "same format → one structure");
    assert_eq!(s["id"], "s1");
    assert_eq!(s["title"], "Đề kiểm tra (trắc nghiệm + tự luận, 90 phút)");
    assert_eq!(s["sections"][0]["kind"], "mcq");
    assert_eq!(s["sections"][0]["count"], 2);
    assert_eq!(s["basedOn"].as_array().unwrap().len(), 5);
    let shares: f64 = k["curriculum"][0]["topics"].as_array().unwrap().iter().map(|t| t["share"].as_f64().unwrap()).sum();
    assert!((shares - 1.0).abs() < 0.01);
    let f = &k["curriculum"][0]["topics"][0]["forms"][0];
    assert!(f["example"]["md"].as_str().unwrap().contains("\\sqrt"));
    assert_eq!(f["refs"][0]["fileId"], "a1");
    assert_eq!(svc.ingest_status("f1").unwrap()["state"], "done");
    assert!(svc.usage_summary()["ingest"]["total"]["costUsd"].as_f64().unwrap() > 0.0);
    for f in ["ingest/records.json", "ingest/assign.json"] {
        assert!(d.join("data/folders/f1").join(f).exists(), "{f}");
    }
    assert_eq!(std::fs::read_dir(d.join("data/folders/f1/ingest/pages")).unwrap().count(), 5, "one cache entry per page");
}

#[tokio::test]
async fn broken_reply_is_retried_once_then_escalated_and_a_dead_page_is_skipped() {
    let d = tmp("retry");
    let fake = Arc::new(FakeLlm::new(|req, n| match req.tag {
        "extract" => {
            if photo(req) == "b3" {
                return Err(LlmError { status: 500, message: "always down".into(), retryable: true });
            }
            match n {
                0 => Err(LlmError { status: 429, message: "slow down".into(), retryable: true }),
                1 => Ok("not json at all".into()),
                _ => Ok(extract_reply(req, "A")),
            }
        }
        "merge" => Ok(merge_reply(req)),
        _ => unreachable!(),
    }));
    let mut c = cfg();
    c.ingest.concurrency = 1;
    let svc = StudyService::with_llm(d.join("data"), c, fake.clone());
    let (evs, r) = drain(svc.start_ingest(req("f2", images(&d, "b", 3))).unwrap()).await;
    let k = r.unwrap();
    // b1: Flash 429, Flash bad JSON (= one retry) → Haiku ok. b2: Flash ok. b3: 2 × Flash + 2 × Haiku fail → skipped.
    assert_eq!(k["stats"]["questions"], 6);
    assert_eq!(fake.calls_with_tag("extract"), 8);
    assert_eq!(fake.calls_with_model("extract", HAIKU), 3);
    let ns = notes(&evs);
    assert!(ns.iter().any(|n| n.to_lowercase().contains("trang 1/3") && n.contains("đọc lại bằng Haiku")), "{ns:?}");
    assert!(ns.iter().any(|n| n.contains("Không đọc được trang 3/3")), "{ns:?}");
    let first = k["sources"].as_array().unwrap().iter().find(|s| s["fileId"] == "b1").unwrap().clone();
    assert_eq!(first["questions"], 3);
}

#[tokio::test]
async fn suspicious_page_goes_to_the_strong_model_and_numbering_gaps_between_pages_too() {
    let d = tmp("suspicious");
    let fake = Arc::new(FakeLlm::new(|req, _| match req.tag {
        "extract" => {
            let p = photo(req);
            let strong = req.model == HAIKU;
            let qs = |ns: &[&str]| ns.iter().map(|n| item("I", n, "mcq", "th", "Căn bậc hai", "Tính", &format!("Câu {n} của {p}: tính $\\sqrt{{4}}$"))).collect::<Vec<_>>();
            Ok(match (p.as_str(), strong) {
                // s1: Flash skips a number inside the page.
                ("s1", false) => page_json("Đề 1", false, qs(&["1", "2", "4"])),
                ("s1", true) => page_json("Đề 1", false, qs(&["1", "2", "3", "4"])),
                // s2 continues s1 but Flash misses its first question (5): a gap between pages.
                ("s2", false) => page_json("", true, qs(&["6", "7"])),
                ("s2", true) => page_json("", true, qs(&["5", "6", "7"])),
                _ => unreachable!(),
            })
        }
        "merge" => Ok(merge_reply(req)),
        _ => unreachable!(),
    }));
    let svc = StudyService::with_llm(d.join("data"), cfg(), fake.clone());
    let (evs, r) = drain(svc.start_ingest(req("f9", images(&d, "s", 2))).unwrap()).await;
    let k = r.unwrap();
    assert_eq!(k["stats"]["questions"], 7, "1-4 from page 1, 5-7 from page 2, one document");
    assert_eq!(k["sources"].as_array().unwrap().len(), 1);
    assert_eq!(fake.calls_with_model("extract", "deepseek-flash"), 2, "no blind retry of a suspicious reading");
    assert_eq!(fake.calls_with_model("extract", HAIKU), 2);
    let ns = notes(&evs);
    assert!(ns.iter().any(|n| n.contains("đánh số nhảy 2 → 4")), "{ns:?}");
    assert!(ns.iter().any(|n| n.contains("chưa khớp nhau") && n.contains("thiếu câu")), "{ns:?}");
}

#[tokio::test]
async fn all_pages_failing_is_an_error_event() {
    let d = tmp("fail");
    let fake = Arc::new(FakeLlm::new(|_, _| Err(LlmError { status: 401, message: "bad key".into(), retryable: false })));
    let svc = StudyService::with_llm(d.join("data"), cfg(), fake.clone());
    let (evs, r) = drain(svc.start_ingest(req("f3", images(&d, "c", 2))).unwrap()).await;
    assert!(r.is_err());
    assert_eq!(evs.last().unwrap()["state"], "error");
    assert_eq!(fake.calls_with_tag("extract"), 4, "non-retryable: no retry, one escalation per page");
    assert!(svc.knowledge("f3").is_none());
}

#[tokio::test]
async fn cancel_stops_the_job_and_the_next_job_resumes_from_the_page_cache() {
    let d = tmp("cancel");
    let mut fake = standard_fake("A");
    fake.delay = Duration::from_millis(300);
    let fake = Arc::new(fake);
    let mut c = cfg();
    c.ingest.concurrency = 1;
    let svc = StudyService::with_llm(d.join("data"), c, fake.clone());
    let files = images(&d, "d", 4);
    let mut h = svc.start_ingest(req("f4", files.clone())).unwrap();
    assert!(svc.start_ingest(req("f4", images(&d, "e", 1))).is_err(), "one job per folder");
    // Cancel right after the first page is read.
    loop {
        let e = h.events.recv().await.unwrap();
        if e["note"].as_str().unwrap_or("").starts_with("Đã đọc trang 1/4") {
            break;
        }
    }
    let t = std::time::Instant::now();
    assert!(svc.cancel("f4"));
    let (evs, r) = drain(h).await;
    assert!(t.elapsed() < Duration::from_millis(250), "cancel does not wait for the call in flight");
    assert_eq!(r.unwrap_err(), beluga_server::study::IngestError::Cancelled);
    assert_eq!(evs.last().unwrap()["state"], "cancelled");
    assert!(svc.knowledge("f4").is_none());
    assert!(!svc.cancel("f4"), "job is gone");
    let before = fake.calls_with_tag("extract");
    assert_eq!(before, 2, "page 1 done, page 2 in flight when cancelled");
    // Resume: the same files again. Page 1 comes from the cache.
    let (evs, r) = drain(svc.start_ingest(req("f4", files.clone())).unwrap()).await;
    assert_eq!(r.unwrap()["stats"]["questions"], 12);
    assert_eq!(fake.calls_with_tag("extract") - before, 3);
    assert!(notes(&evs).iter().any(|n| n.contains("trang 1/4") && n.contains("đã đọc ở lần trước")));
    // Everything cached now: re-ingesting costs no page call at all, and the merge is all known.
    let calls = fake.calls.lock().unwrap().len();
    let (evs, r) = drain(svc.start_ingest(req("f4", files)).unwrap()).await;
    assert_eq!(r.unwrap()["stats"]["questions"], 12, "re-upload replaces, never doubles");
    assert_eq!(fake.calls.lock().unwrap().len(), calls, "no model call at all");
    assert!(notes(&evs).iter().any(|n| n.contains("không cần gọi model")));
}

#[tokio::test]
async fn second_ingest_merges_incrementally_and_keeps_ids() {
    let d = tmp("merge");
    let data = d.join("data");
    let svc = StudyService::with_llm(data.clone(), cfg(), Arc::new(standard_fake("A")));
    let (_, r) = drain(svc.start_ingest(req("f5", images(&d, "m", 2))).unwrap()).await;
    let k1 = r.unwrap();
    let fake_b = Arc::new(standard_fake("B"));
    let svc = StudyService::with_llm(data.clone(), cfg(), fake_b.clone());
    let (_, r) = drain(svc.start_ingest(req("f5", images(&d, "n", 2))).unwrap()).await;
    let k2 = r.unwrap();
    assert!(validate_knowledge(&k2).is_empty(), "{:?}", validate_knowledge(&k2));
    assert_eq!(k2["stats"]["questions"], 12, "old + new records");
    assert_eq!(k2["sources"].as_array().unwrap().len(), 4);
    // Only the new groups went to the model, with an outline of the existing tree.
    let p = text_of(&fake_b.calls.lock().unwrap().iter().find(|r| r.tag == "merge").unwrap().clone());
    assert!(p.contains("EXISTING:\nc1 Căn bậc hai, căn bậc ba\n  c1.t1 Căn bậc hai"), "{p}");
    let raw_lines = p.split("RAW (").nth(1).unwrap().lines().skip(1).filter(|l| l.contains(" | ")).count();
    assert_eq!(raw_lines, 2, "the known group (Tìm điều kiện xác định) was placed without the model");
    let id_of = |k: &Value, title: &str| -> Option<String> {
        k["curriculum"][0]["topics"].as_array().unwrap().iter().find(|t| t["title"] == title).map(|t| t["id"].as_str().unwrap().to_string())
    };
    for t in ["Căn bậc hai", "Biến đổi căn thức A"] {
        assert_eq!(id_of(&k1, t), id_of(&k2, t), "{t} keeps its id");
    }
    assert_eq!(id_of(&k2, "Biến đổi căn thức B").as_deref(), Some("c1.t3"), "new topic gets the next id");
    assert_eq!(k2["curriculum"][0]["topics"][0]["count"], 4, "the old topic now counts both ingests");
    assert_eq!(k2["goal"], "ôn giữa kỳ 1");
    assert_eq!(k1["structures"][0]["id"], k2["structures"][0]["id"]);
}

#[tokio::test]
async fn broken_merge_falls_back_to_the_names_read() {
    let d = tmp("fallback");
    let fake = Arc::new(FakeLlm::new(|req, _| match req.tag {
        "extract" => Ok(extract_reply(req, "A")),
        _ => Ok(json!({"curriculum": []}).to_string()),
    }));
    let svc = StudyService::with_llm(d.join("data"), cfg(), fake.clone());
    let (evs, r) = drain(svc.start_ingest(req("f6", images(&d, "h", 2))).unwrap()).await;
    let k = r.unwrap();
    assert!(validate_knowledge(&k).is_empty(), "{:?}", validate_knowledge(&k));
    assert_eq!(fake.calls_with_tag("merge"), 2, "1 + 1 retry");
    assert!(notes(&evs).iter().any(|n| n.contains("theo tên chương")));
    assert_eq!(k["curriculum"][0]["title"], "Căn bậc hai, căn bậc ba");
    assert_eq!(k["structures"].as_array().unwrap().len(), 1, "full exams still become a structure");
}

#[tokio::test]
async fn a_flaky_provider_fails_over_to_the_other_one() {
    let d = tmp("failover");
    let deepseek = Arc::new(FakeLlm::new(|_, _| Err(LlmError { status: 503, message: "overloaded".into(), retryable: true })));
    let openrouter = Arc::new(standard_fake("A"));
    let providers = Providers::new(Some(deepseek.clone() as Arc<dyn Llm>), Some(openrouter.clone() as Arc<dyn Llm>), Duration::from_secs(5));
    let mut c = cfg();
    c.ingest.concurrency = 1;
    let svc = StudyService::with_llm(d.join("data"), c, Arc::new(providers));
    let (_, r) = drain(svc.start_ingest(req("f7", images(&d, "p", 3))).unwrap()).await;
    assert_eq!(r.unwrap()["stats"]["questions"], 9);
    assert_eq!(deepseek.calls.lock().unwrap().len(), 1, "DeepSeek failed once, then rested");
    assert_eq!(openrouter.calls_with_model("extract", "deepseek/deepseek-v4.1-flash"), 3, "same model through OpenRouter");
    assert_eq!(openrouter.calls_with_model("extract", HAIKU), 0, "failover, not escalation");
}

async fn folder_with_knowledge(name: &str, fake: Arc<FakeLlm>) -> (StudyService, PathBuf) {
    let d = tmp(name);
    let svc = StudyService::with_llm(d.join("data"), cfg(), fake);
    let (_, r) = drain(svc.start_ingest(req("fx", images(&d, "x", 4))).unwrap()).await;
    r.unwrap();
    (svc, d)
}

#[tokio::test]
async fn gen_exam_matches_structure_and_renders() {
    let fake = Arc::new(standard_fake("A"));
    let (svc, d) = folder_with_knowledge("exam", fake.clone()).await;
    let k = svc.knowledge("fx").unwrap();
    let events = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let ev2 = events.clone();
    let item = svc.gen_exam("fx", "s1", Some("khó hơn một chút"), move |e| ev2.lock().unwrap().push(e)).await.unwrap();
    let states: Vec<String> = events.lock().unwrap().iter().map(|e| e["state"].as_str().unwrap().to_string()).collect();
    assert_eq!(states, ["writing", "checking", "done"]);
    assert_eq!(events.lock().unwrap()[2]["title"], "Đề luyện giữa kỳ I – Toán 9");
    assert_eq!(fake.calls_with_tag("exam"), 2, "one call per section, in parallel");
    let exam_id = item["examId"].as_str().unwrap();
    let doc = svc.exam(exam_id).unwrap();
    let st = &k["structures"][0];
    assert!(exam::check_exam(&doc, st).is_empty(), "{:?}", exam::check_exam(&doc, st));
    assert_eq!(doc["questions"], st["sections"].as_array().unwrap().iter().map(|s| s["count"].as_u64().unwrap()).sum::<u64>());
    assert!(fake.calls.lock().unwrap().iter().filter(|r| r.tag == "exam").all(|r| text_of(r).contains("khó hơn một chút")));
    assert_eq!(svc.list_exams("fx").len(), 1);
    assert_eq!(svc.list_exams("fx")[0]["examId"], exam_id);

    let html = svc.exam_html(exam_id).unwrap();
    assert!(html.contains("/board/vendor/katex/katex.min.js"));
    assert!(html.contains("page-break-before: always"));
    assert!(html.contains("exam_ready") && html.contains("dataset.ready"));
    let n = doc["questions"].as_u64().unwrap() as usize;
    for i in 1..=n {
        assert!(html.contains(&format!("Câu mới số {i}:")), "question {i} in html");
    }
    let key_at = html.find("class=\"key\"").unwrap();
    assert!(html[key_at..].contains("Lời giải chi tiết"), "answer key after the exam");
    assert!(html.contains("data-tex=\"\\sqrt{1}\""));
    assert!(svc.delete_exam(exam_id));
    assert!(svc.list_exams("fx").is_empty());
    svc.delete_folder_data("fx").unwrap();
    assert!(svc.knowledge("fx").is_none());
    let _ = std::fs::remove_dir_all(d);
}

#[tokio::test]
async fn gen_exam_repairs_only_the_broken_section_once_then_gives_up() {
    // The section with slot 2 misses it; its repair is complete. The other section is never repaired.
    let fake = Arc::new(FakeLlm::new(|req, n| match req.tag {
        "extract" => Ok(extract_reply(req, "A")),
        "merge" => Ok(merge_reply(req)),
        "exam" => Ok(exam_reply(req, Some(2))),
        "repair" => {
            assert!(text_of(req).contains("missing slots: 2"));
            Ok(if n == 0 { exam_reply(req, None) } else { "{}".into() })
        }
        _ => unreachable!(),
    }));
    let (svc, _d) = folder_with_knowledge("repair", fake.clone()).await;
    let item = svc.gen_exam("fx", "s1", None, |_| {}).await.unwrap();
    assert_eq!(fake.calls_with_tag("repair"), 1);
    assert!(item["questions"].as_u64().unwrap() > 0);

    // Second time the repair is broken as well → error event, nothing stored.
    let events = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
    let ev2 = events.clone();
    let r = svc.gen_exam("fx", "s1", None, move |e| ev2.lock().unwrap().push(e)).await;
    assert!(r.is_err());
    assert_eq!(events.lock().unwrap().last().unwrap()["state"], "error");
    assert_eq!(svc.list_exams("fx").len(), 1);
    // Unknown structure.
    assert!(svc.gen_exam("fx", "s9", None, |_| {}).await.unwrap_err().contains("s9"));
}

#[test]
fn html_lays_out_mcq_choices_and_true_false() {
    let doc = json!({"title": "Đề thử", "subject": "toan", "grade": "9", "durationMin": 45, "sections": [
        {"id": "A", "title": "Trắc nghiệm", "kind": "mcq", "points": 0.5, "instructions": "", "questions": [
            {"no": 1, "label": "Câu 1", "kind": "mcq", "points": 0.25, "text": "Tính $1+1$", "choices": ["$2$", "$3$", "$4$", "$5$"], "answer": "A", "solution": ""},
            {"no": 2, "label": "Câu 2", "kind": "mcq", "points": 0.25, "text": "Chọn câu đúng <b>", "choices": ["Một khẳng định rất dài về tam giác vuông và các cạnh", "b", "c", "d"], "answer": "B", "solution": ""}]},
        {"id": "B", "title": "Đúng sai", "kind": "truefalse", "points": 1, "instructions": "", "questions": [
            {"no": 3, "label": "Câu 3", "kind": "truefalse", "points": 1, "text": "Cho $x=2$.", "statements": ["$x>1$", "$x<0$", "$x^2=4$", "$x=3$"], "answer": "Đ,S,Đ,S", "solution": ""}]}]});
    let h = render_html_with_base(&doc, "../katex/");
    assert!(h.contains("choices ch4") && h.contains("choices ch1"));
    assert!(h.contains("Chọn câu đúng &lt;b&gt;"), "text is escaped");
    assert!(h.contains("a) Đ") && h.contains("d) S"));
    assert!(h.contains("../katex/katex.min.css"));
}

/// Real run on the sample material (photos + PDF rendered by tests/fixtures/study/render.py).
/// `BELUGA_STUDY_MATERIAL=<dir> cargo test --test study real_ -- --ignored --nocapture`
/// (the benchmark with scoring is `examples/ingest_bench.rs`).
#[tokio::test]
#[ignore]
async fn real_ingest_and_exam() {
    let dir = PathBuf::from(std::env::var("BELUGA_STUDY_MATERIAL").expect("BELUGA_STUDY_MATERIAL"));
    let out = std::env::var("BELUGA_STUDY_OUT").map(PathBuf::from).unwrap_or_else(|_| tmp("real"));
    let server_dir = PathBuf::from(std::env::var("BELUGA_SERVER_DIR").unwrap_or_else(|_| env!("CARGO_MANIFEST_DIR").into()));
    let cfg = StudyConfig::from_env(&server_dir);
    assert!(cfg.api_key.is_some() || cfg.deepseek_key.is_some(), "OPENROUTER_API_KEY or DEEPSEEK_API_KEY");
    let svc = StudyService::new(out.join("data"), cfg);
    let mut files = Vec::new();
    for name in ["de1_p1.jpg", "de1_p2.jpg", "de1_p3.jpg", "phieu.jpg", "de2.pdf"] {
        let p = dir.join(name);
        let kind = if name.ends_with(".pdf") { FileKind::Pdf } else { FileKind::Image };
        files.push(IngestFile { file_id: name.replace('.', "_"), path: p, kind, name: name.into() });
    }
    let t0 = std::time::Instant::now();
    let (evs, r) = drain(svc.start_ingest(req("real", files)).unwrap()).await;
    for e in &evs {
        println!("{e}");
    }
    let k = r.expect("ingest");
    println!("ingest: {:.1}s", t0.elapsed().as_secs_f64());
    println!("validate: {:?}", validate_knowledge(&k));
    let sid = k["structures"][0]["id"].as_str().expect("a structure").to_string();
    let t1 = std::time::Instant::now();
    let item = svc.gen_exam("real", &sid, None, |e| println!("{e}")).await.expect("exam");
    println!("exam: {:.1}s {item}", t1.elapsed().as_secs_f64());
    let html = svc.exam_html(item["examId"].as_str().unwrap()).unwrap();
    std::fs::write(out.join("exam.html"), html).unwrap();
    println!("usage: {}", svc.usage_summary());
    println!("out: {}", out.display());
}

/// Exam only, on the knowledge of a previous `real_ingest_and_exam` run (`BELUGA_STUDY_OUT`).
/// `BELUGA_EXAM_MODEL` / `BELUGA_EXAM_EFFORT` pick the generator.
#[tokio::test]
#[ignore]
async fn real_exam_only() {
    let out = PathBuf::from(std::env::var("BELUGA_STUDY_OUT").expect("BELUGA_STUDY_OUT"));
    let server_dir = PathBuf::from(std::env::var("BELUGA_SERVER_DIR").unwrap_or_else(|_| env!("CARGO_MANIFEST_DIR").into()));
    let cfg = StudyConfig::from_env(&server_dir);
    println!("model {} effort {:?}", cfg.exam.model, cfg.exam.effort);
    let svc = StudyService::new(out.join("data"), cfg);
    let k = svc.knowledge("real").expect("knowledge");
    let sid = k["structures"][0]["id"].as_str().unwrap().to_string();
    let t1 = std::time::Instant::now();
    let item = svc.gen_exam("real", &sid, None, |e| println!("{e}")).await.expect("exam");
    println!("exam: {:.1}s {item}", t1.elapsed().as_secs_f64());
    let id = item["examId"].as_str().unwrap();
    std::fs::write(out.join(format!("{id}.html")), svc.exam_html(id).unwrap()).unwrap();
    println!("usage: {}", svc.usage_summary());
}

#[test]
fn sample_fixture_is_valid_knowledge() {
    let k: Value = serde_json::from_str(include_str!("fixtures/knowledge.sample.json")).unwrap();
    assert!(validate_knowledge(&k).is_empty(), "{:?}", validate_knowledge(&k));
    let st = &k["structures"][0];
    let (_, slots) = exam::blueprint(&k, st["id"].as_str().unwrap()).unwrap();
    let want: u64 = st["sections"].as_array().unwrap().iter().map(|s| s["count"].as_u64().unwrap()).sum();
    assert_eq!(slots.len() as u64, want);
}
