//! The subject skills under `tutor/.claude/skills/`: right shape, and every `@directive` example in
//! them is accepted by the real lesson parser (so the tutor is never taught a line the server drops).

use beluga_server::config::find_repo_root;
use beluga_server::memory::SUBJECTS;
use beluga_server::parser::{Event, LessonParser};
use std::path::PathBuf;

fn skills_dir() -> PathBuf {
    find_repo_root().join("tutor").join(".claude").join("skills")
}

const BLOCK_TYPES: &[&str] = &["math", "chem", "text", "table", "code", "timeline", "vocab", "svg", "plot", "answer"];

fn read(id: &str) -> String {
    std::fs::read_to_string(skills_dir().join(id).join("SKILL.md")).unwrap_or_else(|e| panic!("{id}: {e}"))
}

/// `@command {json}` examples written inside backticks.
fn examples(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let mut rest = line;
        while let Some(i) = rest.find("`@") {
            let after = &rest[i + 1..];
            let Some(end) = after.find('`') else { break };
            let span = &after[..end];
            if span.contains(" {") {
                out.push(span.to_string()); // `@step` alone is a mention, not an example
            }
            rest = &after[end + 1..];
        }
    }
    out
}

#[test]
fn there_is_one_skill_per_subject_and_nothing_else() {
    let mut on_disk: Vec<String> = std::fs::read_dir(skills_dir())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    on_disk.sort();
    let mut want: Vec<String> = SUBJECTS.iter().map(|(id, _)| id.to_string()).collect();
    want.sort();
    assert_eq!(on_disk, want);
}

#[test]
fn frontmatter_length_and_memory_conventions() {
    for (id, _) in SUBJECTS {
        let t = read(id);
        let lines: Vec<&str> = t.lines().collect();
        assert!((40..=80).contains(&lines.len()), "{id}: {} lines", lines.len());
        assert_eq!(lines[0], "---", "{id}");
        let end = lines.iter().skip(1).position(|l| *l == "---").expect("closing ---") + 1;
        let front = &lines[1..end];
        assert_eq!(front.len(), 2, "{id}: only name and description: {front:?}");
        assert_eq!(front[0], format!("name: {id}"));
        let desc = front[1].strip_prefix("description: ").expect("description");
        assert!(desc.chars().count() > 60, "{id}: description says when to use it");
        assert!(!desc.contains(": "), "{id}: a colon followed by a space would break the YAML");
        assert!(t.contains("memory_note") && t.contains(&format!("`{id}`")), "{id}: names the memory_note subject");
        assert!(t.contains("@ask"), "{id}: says where to ask");
    }
}

#[test]
fn every_directive_example_parses_and_uses_known_blocks() {
    let mut total = 0;
    for (id, _) in SUBJECTS {
        let ex = examples(&read(id));
        assert!(ex.len() >= 3, "{id}: only {} examples", ex.len());
        for e in ex {
            total += 1;
            let mut p = LessonParser::new(1, 1, 0);
            let mut ev = p.push_str(&e);
            ev.extend(p.push_str("\n"));
            ev.extend(p.finish());
            let dropped = p.take_dropped();
            assert!(dropped.is_empty(), "{id}: parser drops `{e}`: {dropped:?}");
            for x in ev {
                if let Event::Show { block, .. } = x {
                    let ty = block["type"].as_str().unwrap();
                    assert!(BLOCK_TYPES.contains(&ty), "{id}: unknown block type {ty}");
                    match ty {
                        "math" | "chem" => assert!(block["tex"].is_string(), "{id}: {e}"),
                        "text" | "answer" => assert!(block["md"].is_string(), "{id}: {e}"),
                        "table" => {
                            let head = block["head"].as_array().expect("head");
                            for r in block["rows"].as_array().expect("rows") {
                                assert_eq!(r.as_array().unwrap().len(), head.len(), "{id}: row width in {e}");
                            }
                        }
                        "code" => assert!(block["lang"].is_string() && block["code"].is_string(), "{id}: {e}"),
                        "timeline" => assert!(block["items"][0]["when"].is_string() && block["items"][0]["what"].is_string()),
                        "vocab" => assert!(block["word"].is_string() && block["meaning"].is_string()),
                        "svg" => assert!(block["svg"].as_str().unwrap().starts_with("<svg")),
                        "plot" => assert!(block["fn"].is_array() && block["x"].is_array()),
                        _ => {}
                    }
                }
            }
        }
    }
    assert!(total >= 25, "{total} examples checked");
}

#[test]
fn skills_are_the_only_skill_rules_the_tutor_may_use() {
    let rules = beluga_server::brain::claude::skill_rules();
    assert_eq!(rules.len(), SUBJECTS.len());
    for (id, _) in SUBJECTS {
        assert!(rules.contains(&format!("Skill({id})")));
        assert!(skills_dir().join(id).join("SKILL.md").exists());
    }
}
