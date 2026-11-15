//! Home-screen suggestions (PROTOCOL section 18): `get_home{folderId?}` -> `home`.
//!
//! With a folder, everything comes from that folder only (never another folder's data). Without
//! one, every item carries its folder's name so the phone always says which folder it is about.

use crate::memory::{self, Memory};
use crate::session::Deps;
use serde_json::{json, Value};

/// Most `due` entries on the home screen outside folders.
const MAX_DUE: usize = 3;
/// Most `quick` entries outside folders.
const MAX_QUICK: usize = 5;

/// The `home` frame. `folder`: the folder the screen belongs to; an unknown id is an error.
pub fn home_frame(deps: &Deps, folder: Option<&str>) -> Value {
    let folder = folder.filter(|f| !f.is_empty());
    let folders = deps.folders.list();
    if let Some(id) = folder {
        if !folders.iter().any(|f| f.id == id) {
            return json!({"t": "error", "code": "no_folder", "text": "Không tìm thấy folder.", "fatal": false});
        }
    }
    let in_scope: Vec<_> = folders.iter().filter(|f| folder.map(|id| f.id == id).unwrap_or(true)).collect();
    let name_of = |id: &str| folders.iter().find(|f| f.id == id).map(|f| f.name.clone());

    let cont = deps
        .store
        .list_sessions()
        .into_iter()
        .find(|s| match folder {
            Some(id) => s.folder_id.as_deref() == Some(id),
            None => true,
        })
        .map(|s| {
            let mut c = json!({"sessionId": s.session_id, "title": s.title, "updatedAt": s.updated_at});
            if let Some(f) = s.folder_id.as_deref() {
                c["folderId"] = json!(f);
                if let Some(n) = name_of(f) {
                    c["folderName"] = json!(n);
                }
            }
            c
        });

    let mem = Memory::new(&deps.cfg.tutor_dir);
    let today = memory::today();
    let mut due: Vec<(usize, Value)> = Vec::new();
    let mut quick: Vec<Value> = Vec::new();
    for f in &in_scope {
        let mut weak = mem.folder_weak(&f.id, &f.kind);
        weak.sort_by(|a, b| a.due.cmp(&b.due).then(b.count.cmp(&a.count)));
        let due_items: Vec<&memory::DueItem> = weak.iter().filter(|w| w.due <= today).collect();
        if !due_items.is_empty() {
            let topics: Vec<&str> = due_items.iter().take(3).map(|w| w.topic.as_str()).collect();
            due.push((due_items.len(), json!({"folderId": f.id, "folderName": f.name, "kind": f.kind, "count": due_items.len(), "topics": topics})));
        }
        // "Ôn nhanh" makes sense once the folder has something to quiz on.
        if !weak.is_empty() || deps.folders.has_knowledge(&f.id) {
            quick.push(json!({"folderId": f.id, "folderName": f.name, "kind": f.kind}));
        }
    }
    due.sort_by_key(|d| std::cmp::Reverse(d.0));
    let mut due: Vec<Value> = due.into_iter().map(|(_, v)| v).collect();
    if folder.is_none() {
        due.truncate(MAX_DUE);
        quick.truncate(MAX_QUICK);
    }
    let mut v = json!({"t": "home", "due": due, "quick": quick});
    if let Some(id) = folder {
        v["folderId"] = json!(id);
    }
    if let Some(c) = cont {
        v["continue"] = c;
    }
    v
}
