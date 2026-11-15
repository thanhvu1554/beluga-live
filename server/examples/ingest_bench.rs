//! Exam-ingest benchmark on real pages with a ground truth. Spends real money (OpenRouter, DeepSeek),
//! so it is an example you run by hand:
//!
//! ```text
//! cargo run --release --example ingest_bench -- --env ../server/.env --config hybrid --runs 3 \
//!     [--set all|fixtures|pdf] [--out dir] [--reuse]
//! ```
//!
//! Pages and `ground_truth.json` live in `tests/fixtures/study/` (`hard_pages.py` makes them). Each run
//! ingests the set into a fresh folder (`--reuse`: the same folder, to see the page cache) through the
//! public `StudyService` API only, then scores `ingest/records.json` against the ground truth:
//! questions found / matched / extra, kind accuracy, formula fidelity (LaTeX snippets compared after
//! normalisation) and Vietnamese fidelity (phrases with their diacritics; a phrase that only matches
//! without diacritics counts as a diacritics error). Keys are read from `--env` and never printed.
//!
//! `--config` names a preset (see `CONFIGS`) or `custom` with `--set-env K=V` pairs.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use beluga_server::study::{FileKind, IngestFile, IngestRequest, StudyConfig, StudyService};
use serde_json::{json, Value};

/// Presets: env overrides on top of the keys. Unknown keys are ignored by older pipelines.
const CONFIGS: &[(&str, &[(&str, &str)])] = &[
    // The pipeline before the harness: Haiku with high reasoning reads (3 photos per call) and merges.
    ("old-haiku-high", &[("BELUGA_INGEST_MODEL", "anthropic/claude-haiku-5.5"), ("BELUGA_INGEST_EFFORT", "high")]),
    ("flash", &[("BELUGA_INGEST_MODEL", "deepseek-flash"), ("BELUGA_ESCALATE_MODEL", "none"), ("BELUGA_MERGE_MODEL", "deepseek-flash")]),
    ("flash-think", &[("BELUGA_INGEST_MODEL", "deepseek-flash"), ("BELUGA_INGEST_EFFORT", "high"), ("BELUGA_ESCALATE_MODEL", "none"),
        ("BELUGA_MERGE_MODEL", "deepseek-flash")]),
    ("haiku", &[("BELUGA_INGEST_MODEL", "anthropic/claude-haiku-5.5"), ("BELUGA_INGEST_EFFORT", "none"), ("BELUGA_ESCALATE_MODEL", "none"),
        ("BELUGA_MERGE_MODEL", "anthropic/claude-haiku-5.5")]),
    ("haiku-high", &[("BELUGA_INGEST_MODEL", "anthropic/claude-haiku-5.5"), ("BELUGA_INGEST_EFFORT", "high"), ("BELUGA_ESCALATE_MODEL", "none"),
        ("BELUGA_MERGE_MODEL", "anthropic/claude-haiku-5.5")]),
    ("hybrid", &[("BELUGA_INGEST_MODEL", "deepseek-flash"), ("BELUGA_ESCALATE_MODEL", "anthropic/claude-haiku-5.5"),
        ("BELUGA_MERGE_MODEL", "anthropic/claude-haiku-5.5")]),
    ("hybrid-flashmerge", &[("BELUGA_INGEST_MODEL", "deepseek-flash"), ("BELUGA_ESCALATE_MODEL", "anthropic/claude-haiku-5.5"),
        ("BELUGA_MERGE_MODEL", "deepseek-flash")]),
    ("default", &[]),
];

fn args() -> Vec<String> {
    std::env::args().collect()
}

fn arg(name: &str, default: &str) -> String {
    let a = args();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned()).unwrap_or_else(|| default.to_string())
}

fn flag(name: &str) -> bool {
    args().iter().any(|x| x == name)
}

fn env_file(path: &Path) -> HashMap<String, String> {
    let mut m = HashMap::new();
    for line in std::fs::read_to_string(path).unwrap_or_default().lines() {
        let line = line.trim().trim_start_matches('\u{feff}');
        if line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            m.insert(k.trim().to_string(), v.trim().trim_matches('"').trim_matches('\'').to_string());
        }
    }
    m
}

// ---------------------------------------------------------------- normalisation

fn replace_all(mut s: String, pairs: &[(&str, &str)]) -> String {
    for (a, b) in pairs {
        s = s.replace(a, b);
    }
    s
}

/// Read one TeX argument at `i`: `{...}` (balanced), `\cmd`, or one char. Returns (inner, next index).
fn tex_arg(c: &[char], mut i: usize) -> Option<(String, usize)> {
    while i < c.len() && c[i] == ' ' {
        i += 1;
    }
    if i >= c.len() {
        return None;
    }
    if c[i] == '{' {
        let mut depth = 0;
        for j in i..c.len() {
            match c[j] {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((c[i + 1..j].iter().collect(), j + 1));
                    }
                }
                _ => {}
            }
        }
        return None;
    }
    if c[i] == '\\' {
        let mut j = i + 1;
        while j < c.len() && c[j].is_ascii_alphabetic() {
            j += 1;
        }
        return Some((c[i..j.max(i + 2).min(c.len())].iter().collect(), j.max(i + 2).min(c.len())));
    }
    Some((c[i].to_string(), i + 1))
}

/// Canonical braces: `\frac ab` → `\frac{a}{b}`, `\sqrt5` → `\sqrt{5}`, `x^{2}` → `x^2`.
fn canon_braces(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        let rest: String = c[i..(i + 6).min(c.len())].iter().collect();
        if rest.starts_with("\\frac") && !c.get(i + 5).is_some_and(|x| x.is_ascii_alphabetic()) {
            if let Some((a, j)) = tex_arg(&c, i + 5) {
                if let Some((b, k)) = tex_arg(&c, j) {
                    out.push_str(&format!("\\frac{{{}}}{{{}}}", canon_braces(&a), canon_braces(&b)));
                    i = k;
                    continue;
                }
            }
        }
        if rest.starts_with("\\sqrt") && !c.get(i + 5).is_some_and(|x| x.is_ascii_alphabetic()) {
            let mut j = i + 5;
            let mut idx = String::new();
            if c.get(j) == Some(&'[') {
                if let Some(e) = c[j..].iter().position(|x| *x == ']') {
                    idx = c[j..j + e + 1].iter().collect();
                    j += e + 1;
                }
            }
            if let Some((a, k)) = tex_arg(&c, j) {
                out.push_str(&format!("\\sqrt{idx}{{{}}}", canon_braces(&a)));
                i = k;
                continue;
            }
        }
        if rest.starts_with("\\vec") || rest.starts_with("\\hat") {
            if !c.get(i + 4).is_some_and(|x| x.is_ascii_alphabetic()) {
                if let Some((a, k)) = tex_arg(&c, i + 4) {
                    out.push_str(&format!("{}{{{}}}", &rest[..4], canon_braces(&a)));
                    i = k;
                    continue;
                }
            }
        }
        if (c[i] == '^' || c[i] == '_') && c.get(i + 1) == Some(&'{') {
            if let Some((a, k)) = tex_arg(&c, i + 1) {
                let inner = canon_braces(&a);
                if inner.chars().count() == 1 || (inner.starts_with('\\') && inner[1..].chars().all(|x| x.is_ascii_alphabetic())) {
                    out.push(c[i]);
                    out.push_str(&inner);
                } else {
                    out.push(c[i]);
                    out.push('{');
                    out.push_str(&inner);
                    out.push('}');
                }
                i = k;
                continue;
            }
        }
        out.push(c[i]);
        i += 1;
    }
    out
}

fn strip_cmd_keep_arg(s: &str, cmd: &str) -> String {
    // `\text{abc}` → `abc`
    let mut out = String::new();
    let mut rest = s;
    while let Some(p) = rest.find(cmd) {
        out.push_str(&rest[..p]);
        let after = &rest[p + cmd.len()..];
        if let Some(stripped) = after.strip_prefix('{') {
            let mut depth = 1;
            let mut end = None;
            for (j, ch) in stripped.char_indices() {
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(j);
                            break;
                        }
                    }
                    _ => {}
                }
            }
            match end {
                Some(e) => {
                    out.push_str(&stripped[..e]);
                    rest = &stripped[e + 1..];
                }
                None => {
                    out.push_str(stripped);
                    rest = "";
                }
            }
        } else {
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

pub fn norm_math(s: &str) -> String {
    let sub: String = s
        .chars()
        .map(|ch| match ch {
            '₀'..='₉' => char::from_digit(ch as u32 - '₀' as u32, 10).unwrap(),
            '⁰' => '0',
            '¹' => '1',
            '²' => '2',
            '³' => '3',
            '⁴'..='⁹' => char::from_digit(ch as u32 - '⁴' as u32 + 4, 10).unwrap(),
            _ => ch,
        })
        .collect();
    // `√12` → `\sqrt{12}` (a digit run under a unicode root sign)
    let sub = {
        let cs: Vec<char> = sub.chars().collect();
        let mut o = String::new();
        let mut i = 0;
        while i < cs.len() {
            if cs[i] == '√' && cs.get(i + 1).is_some_and(|c| c.is_ascii_digit()) {
                let mut j = i + 1;
                while j < cs.len() && cs[j].is_ascii_digit() {
                    j += 1;
                }
                o.push_str(&format!("\\sqrt{{{}}}", cs[i + 1..j].iter().collect::<String>()));
                i = j;
            } else {
                o.push(cs[i]);
                i += 1;
            }
        }
        o
    };
    let mut t = replace_all(
        sub,
        &[
            ("\\longrightarrow", "->"), ("\\rightarrow", "->"), ("\\xrightarrow", "->"), ("\\rightleftharpoons", "<=>"),
            ("\\leftrightarrows", "<=>"), ("\\Leftrightarrow", "<=>"),
            ("−", "-"), ("–", "-"), ("≥", "\\ge "), ("≤", "\\le "), ("≠", "\\ne "), ("±", "\\pm "), ("·", "\\cdot "),
            ("×", "\\times "), ("√", "\\sqrt"), ("°", "^\\circ"), ("π", "\\pi "), ("α", "\\alpha "), ("∞", "\\infty "),
            ("∪", "\\cup "), ("ℝ", "\\mathbb{R}"), ("⟶", "->"), ("→", "->"), ("⇌", "<=>"), ("∫", "\\int"), ("∑", "\\sum"),
            ("Δ", "\\Delta "), ("\\(", ""), ("\\)", ""), ("\\[", ""), ("\\]", ""), ("$", ""),
            ("\\left", ""), ("\\right", ""), ("\\displaystyle", ""), ("\\limits", ""), ("\\,", ""), ("\\;", ""), ("\\!", ""),
            ("\\quad", ""), ("\\ ", ""), ("~", ""), ("\\dfrac", "\\frac"), ("\\tfrac", "\\frac"), ("\\geqslant", "\\ge "),
            ("\\leqslant", "\\le "), ("\\geq", "\\ge "), ("\\leq", "\\le "), ("\\neq", "\\ne "), ("\\overrightarrow", "\\vec"),
            ("\\widehat", "\\hat"),
            ("\\mathbb R", "\\mathbb{R}"), ("{,}", ","), ("^{\\circ}", "^\\circ"), ("\\longrightarrow", "->"),
            ("\\rightarrow", "->"), ("\\xrightarrow", "->"), ("\\rightleftharpoons", "<=>"), ("\\leftrightarrows", "<=>"),
            ("\\to", "->"), ("\\varnothing", "\\emptyset"),
        ],
    );
    for cmd in ["\\text", "\\mathrm", "\\textrm", "\\operatorname", "\\mbox"] {
        t = strip_cmd_keep_arg(&t, cmd);
    }
    let t: String = t.chars().filter(|c| !c.is_whitespace()).collect();
    canon_braces(&t).replace("\\to", "->")
}

fn norm_chem(s: &str) -> String {
    let t = strip_cmd_keep_arg(&norm_math(s), "\\ce");
    t.chars().filter(|c| !matches!(c, '{' | '}' | '_' | '^')).collect::<String>().replace("\\mathrm", "")
}

const VI_FROM: &str = "àáảãạăằắẳẵặâầấẩẫậèéẻẽẹêềếểễệìíỉĩịòóỏõọôồốổỗộơờớởỡợùúủũụưừứửữựỳýỷỹỵđ";
const VI_TO: &str = "aaaaaaaaaaaaaaaaaeeeeeeeeeeeiiiiiooooooooooooooooouuuuuuuuuuuyyyyyd";

fn strip_vi(s: &str) -> String {
    let from: Vec<char> = VI_FROM.chars().collect();
    let to: Vec<char> = VI_TO.chars().collect();
    s.chars().map(|c| from.iter().position(|x| *x == c).map(|i| to[i]).unwrap_or(c)).collect()
}

fn norm_vi(s: &str) -> String {
    let t = replace_all(
        s.to_lowercase(),
        &[("oà", "òa"), ("oá", "óa"), ("oả", "ỏa"), ("oã", "õa"), ("oạ", "ọa"), ("oè", "òe"), ("oé", "óe"), ("oẻ", "ỏe"),
          ("oẽ", "õe"), ("oẹ", "ọe"), ("uỳ", "ùy"), ("uý", "úy"), ("uỷ", "ủy"), ("uỹ", "ũy"), ("uỵ", "ụy")],
    );
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn norm_n(n: &str) -> String {
    let l = n.to_lowercase();
    let l = replace_all(l, &[("câu", ""), ("bài", ""), ("cau", ""), ("bai", ""), ("ý", "")]);
    l.chars().filter(|c| c.is_alphanumeric()).collect()
}

fn norm_sec(s: &str) -> String {
    let u = s.to_uppercase().replace("PHẦN", " ").replace("PHAN", " ");
    for tok in u.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()) {
        if ["I", "II", "III", "IV", "V", "VI", "A", "B", "C", "D"].contains(&tok) || tok.chars().all(|c| c.is_ascii_digit()) {
            return tok.to_string();
        }
        break;
    }
    String::new()
}

// ---------------------------------------------------------------- scoring

#[derive(Default, Debug, Clone)]
struct Score {
    gt: usize,
    found: usize,
    matched: usize,
    extra: usize,
    kind_ok: usize,
    math_total: usize,
    math_ok: usize,
    vi_total: usize,
    vi_ok: usize,
    vi_diacritics: usize,
    misses: Vec<String>,
}

fn score(gt: &Value, records: &Value) -> Score {
    let mut sc = Score::default();
    let sources = records["sources"].as_array().cloned().unwrap_or_default();
    let qs = records["questions"].as_array().cloned().unwrap_or_default();
    sc.found = qs.len();
    let mut used: HashSet<usize> = HashSet::new();
    for doc in gt["docs"].as_array().into_iter().flatten() {
        let files: HashSet<String> =
            doc["files"].as_array().into_iter().flatten().filter_map(|f| f.as_str()).map(|f| f.trim_end_matches(".jpg").to_string()).collect();
        let srcs: HashSet<String> = sources
            .iter()
            .filter(|s| s["fileIds"].as_array().into_iter().flatten().any(|f| files.contains(f.as_str().unwrap_or(""))))
            .map(|s| s["id"].as_str().unwrap_or("").to_string())
            .collect();
        let cand: Vec<usize> = (0..qs.len()).filter(|i| srcs.contains(qs[*i]["src"].as_str().unwrap_or(""))).collect();
        let gq = doc["questions"].as_array().cloned().unwrap_or_default();
        sc.gt += gq.len();
        let mut m: Vec<Option<usize>> = vec![None; gq.len()];
        let key = |q: &Value| (norm_sec(q["section"].as_str().unwrap_or("")), norm_n(q["n"].as_str().unwrap_or("")));
        // pass 1: section + number
        for (gi, g) in gq.iter().enumerate() {
            let (gs, gn) = (g["section"].as_str().unwrap_or("").to_string(), norm_n(g["n"].as_str().unwrap_or("")));
            if let Some(&i) = cand.iter().find(|i| !used.contains(*i) && key(&qs[**i]) == (gs.clone(), gn.clone())) {
                used.insert(i);
                m[gi] = Some(i);
            }
        }
        // pass 2: number only, in reading order
        for (gi, g) in gq.iter().enumerate() {
            if m[gi].is_some() {
                continue;
            }
            let gn = norm_n(g["n"].as_str().unwrap_or(""));
            if let Some(&i) = cand.iter().find(|i| !used.contains(*i) && key(&qs[**i]).1 == gn) {
                used.insert(i);
                m[gi] = Some(i);
            }
        }
        // pass 3: "1" vs "1a" (cluster split or not split)
        for (gi, g) in gq.iter().enumerate() {
            if m[gi].is_some() {
                continue;
            }
            let gn = norm_n(g["n"].as_str().unwrap_or(""));
            let base = gn.trim_end_matches(|c: char| c.is_alphabetic()).to_string();
            if let Some(&i) = cand.iter().find(|i| {
                let n = key(&qs[**i]).1;
                !used.contains(*i) && (n.trim_end_matches(|c: char| c.is_alphabetic()) == base) && (n == base || gn == base)
            }) {
                used.insert(i);
                m[gi] = Some(i);
            }
        }
        for (gi, g) in gq.iter().enumerate() {
            let gk = format!("{}:{}", doc["id"].as_str().unwrap_or(""), g["key"].as_str().unwrap_or(""));
            let Some(i) = m[gi] else {
                sc.misses.push(format!("{gk} not found"));
                let nm = g["math"].as_array().map(|a| a.len()).unwrap_or(0);
                let nv = g["vi"].as_array().map(|a| a.len()).unwrap_or(0);
                sc.math_total += nm;
                sc.vi_total += nv;
                continue;
            };
            sc.matched += 1;
            if qs[i]["kind"] == g["kind"] {
                sc.kind_ok += 1;
            } else {
                sc.misses.push(format!("{gk} kind {} != {}", qs[i]["kind"], g["kind"]));
            }
            // search space: this item + its siblings (same base number, e.g. 1a/1b share the stem)
            let base = norm_n(qs[i]["n"].as_str().unwrap_or("")).trim_end_matches(|c: char| c.is_alphabetic()).to_string();
            let sec = key(&qs[i]).0;
            let mut text = String::new();
            for &j in &cand {
                let (s2, n2) = key(&qs[j]);
                if j == i || (s2 == sec && n2.trim_end_matches(|c: char| c.is_alphabetic()) == base) {
                    text.push_str(qs[j]["text"].as_str().unwrap_or(""));
                    text.push('\n');
                }
            }
            let chem = g["chem"].as_bool().unwrap_or(false);
            let nt = if chem { norm_chem(&text) } else { norm_math(&text) };
            for a in g["math"].as_array().into_iter().flatten().filter_map(|a| a.as_str()) {
                sc.math_total += 1;
                let na = if chem { norm_chem(a) } else { norm_math(a) };
                if nt.contains(&na) {
                    sc.math_ok += 1;
                } else {
                    sc.misses.push(format!("{gk} math `{a}` (norm `{na}`) not in `{}`", nt.chars().take(260).collect::<String>()));
                }
            }
            let vt = norm_vi(&text);
            for p in g["vi"].as_array().into_iter().flatten().filter_map(|a| a.as_str()) {
                sc.vi_total += 1;
                let np = norm_vi(p);
                if vt.contains(&np) {
                    sc.vi_ok += 1;
                } else if strip_vi(&vt).contains(&strip_vi(&np)) {
                    sc.vi_diacritics += 1;
                    sc.misses.push(format!("{gk} diacritics `{p}`"));
                } else {
                    sc.misses.push(format!("{gk} vi `{p}` missing"));
                }
            }
        }
    }
    sc.extra = qs.len() - used.len();
    for (i, q) in qs.iter().enumerate() {
        if !used.contains(&i) {
            sc.misses.push(format!("extra {} {} n={} {}", q["src"], q["section"], q["n"], q["text"].as_str().unwrap_or("").chars().take(60).collect::<String>()));
        }
    }
    sc
}

fn pct(a: usize, b: usize) -> f64 {
    if b == 0 {
        100.0
    } else {
        (a as f64 * 1000.0 / b as f64).round() / 10.0
    }
}

// ---------------------------------------------------------------- run

#[tokio::main]
async fn main() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(std::env::var("RUST_LOG").unwrap_or_else(|_| "beluga_server=warn".into()))
        .with_writer(std::io::stderr)
        .try_init();
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let fx = here.join("tests/fixtures/study");
    let gt: Value = serde_json::from_str(&std::fs::read_to_string(fx.join("ground_truth.json")).expect("ground_truth.json")).unwrap();
    let rescore = arg("--rescore", "");
    if !rescore.is_empty() {
        // Score saved runs again (after a scorer fix) without calling any model.
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&rescore).unwrap().flatten().map(|e| e.path()).filter(|p| p.join("row.json").exists()).collect();
        dirs.sort();
        for d in dirs {
            let rec: Value = serde_json::from_slice(&std::fs::read(d.join("data/folders/bench/ingest/records.json")).unwrap()).unwrap();
            let have: HashSet<String> = rec["sources"].as_array().into_iter().flatten()
                .flat_map(|s| s["fileIds"].as_array().cloned().unwrap_or_default()).filter_map(|f| f.as_str().map(String::from)).collect();
            let docs: Vec<Value> = gt["docs"].as_array().unwrap().iter()
                .filter(|d| d["files"].as_array().unwrap().iter().any(|f| have.contains(f.as_str().unwrap().trim_end_matches(".jpg")))).cloned().collect();
            let sc = score(&json!({"docs": docs}), &rec);
            let mut row: Value = serde_json::from_slice(&std::fs::read(d.join("row.json")).unwrap()).unwrap();
            for (k, v) in [("matched", json!(sc.matched)), ("gt", json!(sc.gt)), ("extra", json!(sc.extra)), ("questions", json!(sc.found)),
                ("kindPct", json!(pct(sc.kind_ok, sc.matched))), ("mathPct", json!(pct(sc.math_ok, sc.math_total))), ("mathOk", json!(sc.math_ok)),
                ("mathTotal", json!(sc.math_total)), ("viOk", json!(sc.vi_ok)), ("viTotal", json!(sc.vi_total)), ("viDiacritics", json!(sc.vi_diacritics))] {
                row[k] = v;
            }
            std::fs::write(d.join("score.txt"), sc.misses.join("\n")).unwrap();
            std::fs::write(d.join("row.json"), serde_json::to_vec_pretty(&row).unwrap()).unwrap();
            println!("{}", serde_json::to_string(&row).unwrap());
        }
        return;
    }
    let env_path = PathBuf::from(arg("--env", "../../../../server/.env"));
    let keys = env_file(&env_path);
    let cfg_name = arg("--config", "default");
    let runs: usize = arg("--runs", "1").parse().unwrap_or(1);
    let set = arg("--set", "all");
    let out = PathBuf::from(arg("--out", &std::env::temp_dir().join("beluga-bench").to_string_lossy()));
    let reuse = flag("--reuse");
    let mut map: HashMap<String, String> = keys
        .into_iter()
        .filter(|(k, _)| k == "OPENROUTER_API_KEY" || k == "DEEPSEEK_API_KEY" || k.starts_with("OPENROUTER_") || k.starts_with("DEEPSEEK_"))
        .collect();
    if let Some((_, kv)) = CONFIGS.iter().find(|(n, _)| *n == cfg_name) {
        for (k, v) in kv.iter() {
            map.insert(k.to_string(), v.to_string());
        }
    } else if cfg_name != "custom" {
        panic!("unknown --config {cfg_name}");
    }
    let a = args();
    for (i, x) in a.iter().enumerate() {
        if x == "--set-env" {
            if let Some((k, v)) = a.get(i + 1).and_then(|s| s.split_once('=')) {
                map.insert(k.to_string(), v.to_string());
            }
        }
    }
    let shown: Vec<String> = map.iter().filter(|(k, _)| k.starts_with("BELUGA_")).map(|(k, v)| format!("{k}={v}")).collect();
    println!("config {cfg_name}: {} | keys: openrouter={} deepseek={}", shown.join(" "), map.contains_key("OPENROUTER_API_KEY"),
        map.contains_key("DEEPSEEK_API_KEY"));
    let names: Vec<&str> = match set.as_str() {
        "fixtures" => vec!["de1_p1.jpg", "de1_p2.jpg", "de1_p3.jpg", "phieu.jpg"],
        "pdf" => vec!["de2.pdf"],
        _ => vec!["de1_p1.jpg", "de1_p2.jpg", "de1_p3.jpg", "phieu.jpg", "hard_math.jpg", "hard_2col.jpg", "hard_table.jpg", "hard_chem.jpg"],
    };
    let gt_set = {
        let stems: HashSet<String> = names.iter().map(|n| n.to_string()).collect();
        let docs: Vec<Value> = gt["docs"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|d| d["files"].as_array().unwrap().iter().any(|f| stems.contains(f.as_str().unwrap())))
            .cloned()
            .collect();
        json!({"docs": docs})
    };
    let mut rows = Vec::new();
    let base = out.join(&cfg_name);
    for run in 1..=runs {
        let data = if reuse { base.join("data") } else { base.join(format!("run{run}-{}", uuid::Uuid::new_v4().simple())).join("data") };
        let svc = StudyService::new(data.clone(), StudyConfig::from_map(&map));
        let files: Vec<IngestFile> = names
            .iter()
            .map(|n| IngestFile {
                file_id: n.rsplit_once('.').unwrap().0.to_string(),
                path: fx.join("pages").join(n),
                kind: if n.ends_with(".pdf") { FileKind::Pdf } else { FileKind::Image },
                name: n.to_string(),
            })
            .collect();
        let t0 = Instant::now();
        let mut h = svc
            .start_ingest(IngestRequest { folder_id: "bench".into(), subject: "toan".into(), goal: Some("ôn cuối kỳ".into()), files })
            .expect("start");
        let mut merge_at = None;
        let mut last = Value::Null;
        let mut log = Vec::new();
        while let Some(ev) = h.events.recv().await {
            let ms = t0.elapsed().as_millis();
            if ev["state"] == "merging" && merge_at.is_none() {
                merge_at = Some(t0.elapsed().as_secs_f64());
            }
            if flag("--verbose") {
                println!("  {ms:>6} {} {}/{} {}", ev["state"], ev["done"], ev["total"], ev["note"].as_str().unwrap_or(""));
            }
            log.push(json!({"ms": ms, "ev": ev}));
            last = ev;
        }
        let r = h.task.await.unwrap();
        let total = t0.elapsed().as_secs_f64();
        let read = merge_at.unwrap_or(total);
        let k = match r {
            Ok(k) => k,
            Err(e) => {
                println!("run {run}: FAILED {e}");
                rows.push(json!({"run": run, "error": e.to_string()}));
                continue;
            }
        };
        let rec: Value = serde_json::from_slice(&std::fs::read(data.join("folders/bench/ingest/records.json")).unwrap()).unwrap();
        let sc = score(&gt_set, &rec);
        let errs = beluga_server::study::ingest::validate_knowledge(&k);
        let topics: usize = k["curriculum"].as_array().into_iter().flatten().map(|c| c["topics"].as_array().map(|t| t.len()).unwrap_or(0)).sum();
        let cost = last["costUsd"].as_f64().unwrap_or(0.0);
        let row = json!({
            "config": cfg_name, "run": run, "totalS": (total * 10.0).round() / 10.0, "readS": (read * 10.0).round() / 10.0,
            "mergeS": ((total - read) * 10.0).round() / 10.0, "costUsd": cost,
            "questions": sc.found, "gt": sc.gt, "matched": sc.matched, "extra": sc.extra,
            "kindPct": pct(sc.kind_ok, sc.matched), "mathPct": pct(sc.math_ok, sc.math_total), "viPct": pct(sc.vi_ok, sc.vi_total),
            "mathOk": sc.math_ok, "mathTotal": sc.math_total, "viOk": sc.vi_ok, "viTotal": sc.vi_total, "viDiacritics": sc.vi_diacritics,
            "topics": topics, "structures": k["structures"].as_array().map(|a| a.len()).unwrap_or(0), "knowledgeErrors": errs.len(),
        });
        println!(
            "{cfg_name} run {run}: total {:.1}s (read {:.1}s, merge {:.1}s) ${cost:.4} | q {} matched {}/{} extra {} | kind {}% math {}/{} vi {}/{} (diacritics {}) | topics {} structures {} kerr {}",
            total, read, total - read, sc.found, sc.matched, sc.gt, sc.extra, pct(sc.kind_ok, sc.matched), sc.math_ok, sc.math_total,
            sc.vi_ok, sc.vi_total, sc.vi_diacritics, topics, row["structures"], errs.len()
        );
        let dir = data.parent().unwrap().to_path_buf();
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("score.txt"), sc.misses.join("\n")).unwrap();
        std::fs::write(dir.join("events.json"), serde_json::to_vec_pretty(&log).unwrap()).unwrap();
        std::fs::write(dir.join("row.json"), serde_json::to_vec_pretty(&row).unwrap()).unwrap();
        rows.push(row);
    }
    let _ = std::fs::create_dir_all(&base);
    let path = base.join(format!("rows-{}.json", chrono::Utc::now().format("%H%M%S")));
    std::fs::write(&path, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
    println!("rows -> {}", path.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_normalisation() {
        assert_eq!(norm_math(r"$\dfrac{2}{\sqrt5 - 1}$"), norm_math(r"\frac{2}{\sqrt{5}-1}"));
        assert_eq!(norm_math(r"x \geq 3"), norm_math(r"x\ge3"));
        assert_eq!(norm_math("x ≥ −3"), norm_math(r"x\ge -3"));
        assert_eq!(norm_math(r"60^{\circ}"), norm_math("60°"));
        assert_eq!(norm_math(r"\frac35"), norm_math(r"\dfrac{3}{5}"));
        assert_eq!(norm_math(r"x^{2}"), "x^2");
        assert_ne!(norm_math(r"\sqrt{2x-6}"), norm_math(r"\sqrt{2}x-6"));
        assert_eq!(norm_chem(r"\ce{Fe + 2HCl -> FeCl2 + H2}"), norm_chem("Fe + 2HCl → FeCl₂ + H₂"));
        assert_eq!(norm_chem(r"1s^{2}2s^{2}"), "1s22s2");
        assert_eq!(norm_vi("Hoà tan"), norm_vi("Hòa  tan"));
        assert_eq!(strip_vi("chất khử"), "chat khu");
        assert_eq!(norm_n("Câu 1."), "1");
        assert_eq!(norm_n("Bài 2 a)"), "2a");
        assert_eq!(norm_sec("PHẦN II. Đúng sai"), "II");
        assert_eq!(norm_sec("I. TRẮC NGHIỆM"), "I");
        assert_eq!(norm_sec("Tự luận"), "");
    }
}
