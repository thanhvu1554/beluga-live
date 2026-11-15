//! Study engine (exam matrix) with a fake LLM: batching, retry, cancel, re-ingest merge, schema, exams.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use beluga_server::study::exam::{self, render_html_with_base};
use beluga_server::study::ingest::{plan_batches, validate_knowledge, IngestOptions};
use beluga_server::study::llm::{FakeLlm, LlmError, LlmRequest, Part};
use beluga_server::study::{FileKind, IngestFile, IngestRequest, StudyConfig, StudyService};
use serde_json::{json, Value};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("beluga-study-{name}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn images(dir: &Path, prefix: &str, n: usize) -> Vec<IngestFile> {
    (1..=n)
        .map(|i| {
            let p = dir.join(format!("{prefix}{i}.jpg"));
            std::fs::write(&p, [0xFF, 0xD8, 0xFF, i as u8]).unwrap();
            IngestFile { file_id: format!("{prefix}{i}"), path: p, kind: FileKind::Image, name: format!("{prefix}{i}.jpg") }
        })
        .collect()
}

fn text_of(req: &LlmRequest) -> String {
    req.parts.iter().filter_map(|p| if let Part::Text(t) = p { Some(t.as_str()) } else { None }).collect::<Vec<_>>().join("\n")
}

/// Extraction reply: one full exam per batch, 2 MCQ + 1 essay per page, topics depend on `variant`.
fn extract_reply(req: &LlmRequest, variant: &str) -> String {
    let pages = req.parts.iter().filter(|p| matches!(p, Part::Image { .. } | Part::Pdf { .. })).count();
    let first_photo = text_of(req).split('"').nth(1).unwrap_or("x").to_string();
    let mut qs = Vec::new();
    for p in 1..=pages {
        for j in 0..3 {
            let essay = j == 2;
            qs.push(json!({
                "src": "A", "page": p, "section": if essay { "II" } else { "I" }, "n": format!("{}{}", p, j),
                "kind": if essay { "essay" } else { "mcq" }, "level": (["nb", "th", "vd"][j]),
                "points": if essay { 1.5 } else { 0.25 },
                "chapter": "Căn bậc hai, căn bậc ba",
                "topic": if j == 0 { "Căn bậc hai".to_string() } else { format!("Biến đổi căn thức {variant}") },
                "form": if j == 0 { "Tìm điều kiện xác định".to_string() } else { format!("Rút gọn biểu thức {variant}{j}") },
                "text": format!("Câu {first_photo} {p}-{j}: rút gọn $\\sqrt{{{}}}$", p * 10 + j),
                "hasFigure": false, "answer": ""
            }));
        }
    }
    json!({"sources": [{"key": "A", "pages": (1..=pages).collect::<Vec<_>>(), "title": format!("Đề ôn {first_photo}"),
        "kind": "exam", "school": "THCS Nguyễn Du", "date": "", "grade": "9", "durationMin": 90, "totalPoints": 0,
        "fullExam": true, "continuesPrevious": false,
        "sections": [{"label": "I", "title": "Trắc nghiệm", "kind": "mcq", "instructions": "Chọn đáp án đúng."},
                     {"label": "II", "title": "Tự luận", "kind": "essay", "instructions": ""}]}],
        "questions": qs})
    .to_string()
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
    let sources: Vec<String> = t
        .lines()
        .filter(|l| l.starts_with('S') && l.contains(" | exam | yes"))
        .map(|l| l.split(" | ").next().unwrap().to_string())
        .collect();
    json!({
        "grade": "9",
        "curriculum": [{"title": "Căn bậc hai, căn bậc ba", "topics": topics.iter().map(|(t, fs)| json!({
            "title": t, "summary": "Tóm tắt", "forms": fs.iter().map(|(f, r)| json!({"title": f, "method": "1. Bước một", "raw": [r]})).collect::<Vec<_>>()
        })).collect::<Vec<_>>()}],
        "structures": [{"title": "Đề giữa kỳ I (12 TN + tự luận)", "sources": sources}],
        "progress": {"covered": ["1.1"], "current": "1.2", "confidence": 0.6, "note": "Suy từ đề mới nhất"}
    })
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
    c.ingest.batch_images = 2;
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

#[test]
fn batching_groups_images_and_splits_long_pdfs() {
    let d = tmp("batch");
    let mut files = images(&d, "img", 5);
    // 9-page PDF built with lopdf.
    let pdf = d.join("long.pdf");
    std::fs::write(&pdf, make_pdf(9)).unwrap();
    files.push(IngestFile { file_id: "pdf1".into(), path: pdf, kind: FileKind::Pdf, name: "long.pdf".into() });
    files.extend(images(&d, "late", 1));
    let opts = IngestOptions { batch_images: 2, pdf_whole_max_pages: 6, pdf_pages_per_batch: 4, ..Default::default() };
    let b = plan_batches(&files, &opts).unwrap();
    let sizes: Vec<usize> = b.iter().map(|b| b.pages.len()).collect();
    assert_eq!(sizes, vec![2, 2, 1, 4, 4, 1, 1], "images in pairs, pdf 9 pages = 4+4+1, then the last image");
    assert_eq!(b[3].first_page, 6);
    assert_eq!(b[5].pages[0].page_in_file, 9);
    for x in &b[3..6] {
        let Part::Pdf { data, .. } = &x.parts[1] else { panic!("pdf part") };
        assert_eq!(beluga_server::study::ingest::pdf_page_count(data), x.pages.len());
    }
    // Short PDFs stay whole.
    let opts = IngestOptions { pdf_whole_max_pages: 10, ..opts };
    assert_eq!(plan_batches(&files[5..6], &opts).unwrap().len(), 1);
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
    assert_eq!(last["done"], 5);
    assert_eq!(last["total"], 5);
    assert_eq!(last["found"]["questions"], 15);
    assert!(last["costUsd"].as_f64().unwrap() > 0.0);
    assert!(evs.iter().any(|e| e["note"].as_str().unwrap().starts_with("Đang đọc trang 1–2/5")), "{evs:?}");
    assert_eq!(fake.calls_with_tag("extract"), 3);
    assert_eq!(fake.calls_with_tag("merge"), 1);

    assert!(validate_knowledge(&k).is_empty(), "{:?}", validate_knowledge(&k));
    assert_eq!(svc.knowledge("f1").unwrap(), k);
    assert_eq!(k["stats"]["questions"], 15);
    assert_eq!(k["stats"]["byLevel"], json!({"nb": 5, "th": 5, "vd": 5, "vdc": 0}));
    assert_eq!(k["sources"].as_array().unwrap().len(), 3);
    assert_eq!(k["schoolProgress"]["coveredTopicIds"], json!(["c1.t1"]));
    assert_eq!(k["schoolProgress"]["currentTopicId"], "c1.t2");
    let s = &k["structures"][0];
    assert_eq!(s["id"], "s1");
    assert_eq!(s["sections"][0]["kind"], "mcq");
    assert_eq!(s["sections"][0]["count"], 4, "representative source = 2 pages × 2 mcq");
    assert_eq!(s["basedOn"].as_array().unwrap().len(), 3);
    let shares: f64 = k["curriculum"][0]["topics"].as_array().unwrap().iter().map(|t| t["share"].as_f64().unwrap()).sum();
    assert!((shares - 1.0).abs() < 0.01);
    let f = &k["curriculum"][0]["topics"][0]["forms"][0];
    assert!(f["example"]["md"].as_str().unwrap().contains("\\sqrt"));
    assert_eq!(f["refs"][0]["fileId"], "a1");
    assert_eq!(svc.ingest_status("f1").unwrap()["state"], "done");
    assert!(svc.usage_summary()["ingest"]["total"]["costUsd"].as_f64().unwrap() > 0.0);
    assert!(d.join("data/folders/f1/ingest/records.json").exists());
}

#[tokio::test]
async fn extraction_retries_then_succeeds_and_skips_a_dead_batch() {
    let d = tmp("retry");
    let fake = Arc::new(FakeLlm::new(|req, n| match req.tag {
        "extract" => {
            let t = text_of(req);
            if t.contains("b3.jpg") {
                return Err(LlmError { status: 500, message: "always down".into(), retryable: true });
            }
            if n == 0 {
                return Err(LlmError { status: 429, message: "slow down".into(), retryable: true });
            }
            if n == 1 {
                return Ok("not json at all".into());
            }
            Ok(extract_reply(req, "A"))
        }
        "merge" => Ok(merge_reply(req)),
        _ => unreachable!(),
    }));
    let mut c = cfg();
    c.ingest.concurrency = 1;
    let svc = StudyService::with_llm(d.join("data"), c, fake.clone());
    let (evs, r) = drain(svc.start_ingest(req("f2", images(&d, "b", 3))).unwrap()).await;
    let k = r.unwrap();
    // Batch 1 (b1,b2): 429, bad JSON, ok. Batch 2 (b3): 3 failures → skipped.
    assert_eq!(k["stats"]["questions"], 6);
    assert!(evs.iter().any(|e| e["note"].as_str().unwrap().contains("Không đọc được trang 3/3")));
    assert_eq!(fake.calls_with_tag("extract"), 6);
}

#[tokio::test]
async fn all_batches_failing_is_an_error_event() {
    let d = tmp("fail");
    let fake = Arc::new(FakeLlm::new(|_, _| Err(LlmError { status: 401, message: "bad key".into(), retryable: false })));
    let svc = StudyService::with_llm(d.join("data"), cfg(), fake.clone());
    let (evs, r) = drain(svc.start_ingest(req("f3", images(&d, "c", 2))).unwrap()).await;
    assert!(r.is_err());
    assert_eq!(evs.last().unwrap()["state"], "error");
    assert_eq!(fake.calls_with_tag("extract"), 1, "non-retryable: no retry");
    assert!(svc.knowledge("f3").is_none());
}

#[tokio::test]
async fn cancel_stops_the_job() {
    let d = tmp("cancel");
    let mut fake = standard_fake("A");
    fake.delay = Duration::from_millis(400);
    let svc = StudyService::with_llm(d.join("data"), cfg(), Arc::new(fake));
    let mut h = svc.start_ingest(req("f4", images(&d, "d", 6))).unwrap();
    assert!(svc.start_ingest(req("f4", images(&d, "e", 1))).is_err(), "one job per folder");
    let first = h.events.recv().await.unwrap();
    assert_eq!(first["state"], "queued");
    assert!(svc.cancel("f4"));
    let (evs, r) = drain(h).await;
    assert_eq!(r.unwrap_err(), beluga_server::study::IngestError::Cancelled);
    assert_eq!(evs.last().unwrap()["state"], "cancelled");
    assert!(svc.knowledge("f4").is_none());
    assert!(!svc.cancel("f4"), "job is gone");
    // A new job can start after the cancel.
    let (_, r) = drain(svc.start_ingest(req("f4", images(&d, "g", 1))).unwrap()).await;
    assert!(r.is_ok());
}

#[tokio::test]
async fn second_ingest_merges_with_existing_knowledge_and_keeps_ids() {
    let d = tmp("merge");
    let data = d.join("data");
    let svc = StudyService::with_llm(data.clone(), cfg(), Arc::new(standard_fake("A")));
    let (_, r) = drain(svc.start_ingest(req("f5", images(&d, "m", 2))).unwrap()).await;
    let k1 = r.unwrap();
    let svc = StudyService::with_llm(data.clone(), cfg(), Arc::new(standard_fake("B")));
    let (_, r) = drain(svc.start_ingest(req("f5", images(&d, "n", 2))).unwrap()).await;
    let k2 = r.unwrap();
    assert!(validate_knowledge(&k2).is_empty(), "{:?}", validate_knowledge(&k2));
    assert_eq!(k2["stats"]["questions"], 12, "old + new records");
    assert_eq!(k2["sources"].as_array().unwrap().len(), 2);
    let id_of = |k: &Value, title: &str| -> Option<String> {
        k["curriculum"][0]["topics"].as_array().unwrap().iter().find(|t| t["title"] == title).map(|t| t["id"].as_str().unwrap().to_string())
    };
    assert_eq!(id_of(&k1, "Biến đổi căn thức A"), id_of(&k2, "Biến đổi căn thức A"));
    assert_eq!(id_of(&k2, "Biến đổi căn thức B").as_deref(), Some("c1.t3"), "new topic gets the next id");
    assert_eq!(k2["goal"], "ôn giữa kỳ 1");

    // Uploading the same file again replaces its earlier reading instead of doubling it.
    let (_, r) = drain(svc.start_ingest(req("f5", images(&d, "n", 2))).unwrap()).await;
    assert_eq!(r.unwrap()["stats"]["questions"], 12);
}

#[tokio::test]
async fn broken_merge_falls_back_to_raw_curriculum() {
    let d = tmp("fallback");
    let fake = Arc::new(FakeLlm::new(|req, _| match req.tag {
        "extract" => Ok(extract_reply(req, "A")),
        _ => Ok(json!({"curriculum": []}).to_string()),
    }));
    let svc = StudyService::with_llm(d.join("data"), cfg(), fake.clone());
    let (evs, r) = drain(svc.start_ingest(req("f6", images(&d, "h", 2))).unwrap()).await;
    let k = r.unwrap();
    assert!(validate_knowledge(&k).is_empty(), "{:?}", validate_knowledge(&k));
    assert_eq!(fake.calls_with_tag("merge"), 3, "1 + 2 retries");
    assert!(evs.iter().any(|e| e["note"].as_str().unwrap().contains("dựng giáo trình trực tiếp")));
    assert_eq!(k["curriculum"][0]["title"], "Căn bậc hai, căn bậc ba");
    assert_eq!(k["structures"].as_array().unwrap().len(), 1, "full exam still becomes a structure");
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
    let exam_id = item["examId"].as_str().unwrap();
    let doc = svc.exam(exam_id).unwrap();
    let st = &k["structures"][0];
    assert!(exam::check_exam(&doc, st).is_empty(), "{:?}", exam::check_exam(&doc, st));
    assert_eq!(doc["questions"], st["sections"].as_array().unwrap().iter().map(|s| s["count"].as_u64().unwrap()).sum::<u64>());
    assert!(text_of(&fake.calls.lock().unwrap().last().unwrap().clone()).contains("khó hơn một chút"));
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
async fn gen_exam_repairs_once_then_gives_up() {
    // First exam call misses slot 2, the repair is complete.
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

/// Real OpenRouter run on the sample material (photos + PDF rendered by tests/fixtures/study/render.py).
/// `BELUGA_STUDY_MATERIAL=<dir> cargo test --test study real_ -- --ignored --nocapture`
#[tokio::test]
#[ignore]
async fn real_ingest_and_exam() {
    let dir = PathBuf::from(std::env::var("BELUGA_STUDY_MATERIAL").expect("BELUGA_STUDY_MATERIAL"));
    let out = std::env::var("BELUGA_STUDY_OUT").map(PathBuf::from).unwrap_or_else(|_| tmp("real"));
    let server_dir = PathBuf::from(std::env::var("BELUGA_SERVER_DIR").unwrap_or_else(|_| env!("CARGO_MANIFEST_DIR").into()));
    let cfg = StudyConfig::from_env(&server_dir);
    assert!(cfg.api_key.is_some(), "OPENROUTER_API_KEY");
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
