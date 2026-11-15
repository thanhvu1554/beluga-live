//! Model catalogue (PROTOCOL section 10): which models the phone may pick and how the `claude`
//! process is launched for each one.
//!
//! Claude Code is only the harness: any endpoint that speaks the Anthropic Messages API works by
//! giving the child process `ANTHROPIC_BASE_URL` + `ANTHROPIC_AUTH_TOKEN` and `--model`.
//! Verified with Claude Code 2.1.325 (15/11/2026): DeepSeek `https://api.deepseek.com/anthropic`
//! (`deepseek-flash`) and OpenRouter `https://openrouter.ai/api` (`anthropic/claude-haiku-5.5`).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

/// USD per million tokens (for providers whose turns are not on the Claude plan).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelEntry {
    pub id: String,
    pub label: String,
    /// `claude` (the CLI's own login) or `anthropic-compatible`.
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub key_env: Option<String>,
    pub model: String,
    #[serde(default)]
    pub efforts: Vec<String>,
    #[serde(default)]
    pub default_effort: Option<String>,
    #[serde(default)]
    pub vision: bool,
    #[serde(default)]
    pub price: Option<Price>,
    /// Resolved from `key_env`; never serialized or logged.
    #[serde(skip)]
    key: Option<String>,
}

fn default_provider() -> String {
    "claude".into()
}

pub const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

impl ModelEntry {
    pub fn tracked(&self) -> bool {
        self.provider == "claude"
    }

    /// Entry for `ready.models` (no URL, no key).
    pub fn public(&self) -> Value {
        let mut v = json!({"id": self.id, "label": self.label, "provider": self.provider, "efforts": self.efforts,
                           "vision": self.vision, "tracked": self.tracked()});
        if let Some(e) = &self.default_effort {
            v["defaultEffort"] = json!(e);
        }
        v
    }

    /// Effective effort: the phone's choice when this model supports it, else the default.
    pub fn effort(&self, wanted: Option<&str>) -> Option<String> {
        match wanted {
            Some(w) if self.efforts.iter().any(|e| e == w) => Some(w.to_string()),
            _ => self.default_effort.clone(),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn entry(id: &str, label: &str, provider: &str, base: Option<&str>, key_env: Option<&str>, model: &str, efforts: &[&str], default_effort: Option<&str>, vision: bool, price: Option<Price>) -> ModelEntry {
    ModelEntry {
        id: id.into(),
        label: label.into(),
        provider: provider.into(),
        base_url: base.map(String::from),
        key_env: key_env.map(String::from),
        model: model.into(),
        efforts: efforts.iter().map(|s| s.to_string()).collect(),
        default_effort: default_effort.map(String::from),
        vision,
        price,
        key: None,
    }
}

/// Used when `server/models.json` does not exist. Same content as `models.example.json`.
pub fn builtin() -> Vec<ModelEntry> {
    let ac = "anthropic-compatible";
    vec![
        entry("sonnet", "Claude Sonnet", "claude", None, None, "sonnet", &["low", "medium", "high"], Some("medium"), true, None),
        entry("opus", "Claude Opus", "claude", None, None, "opus", &["low", "medium", "high", "xhigh", "max"], Some("low"), true, None),
        entry("haiku", "Claude Haiku", "claude", None, None, "haiku", &[], None, true, None),
        entry("deepseek", "DeepSeek Flash", ac, Some("https://api.deepseek.com/anthropic"), Some("DEEPSEEK_API_KEY"), "deepseek-flash", &[], None, true,
            Some(Price { input: 0.05, output: 0.2, cache_read: 0.01, cache_write: 0.05 })),
        entry("openrouter-haiku", "Haiku · OpenRouter", ac, Some("https://openrouter.ai/api"), Some("OPENROUTER_API_KEY"), "anthropic/claude-haiku-5.5", &[], None, true,
            Some(Price { input: 0.1, output: 0.5, cache_read: 0.01, cache_write: 0.125 })),
    ]
}

/// How to start `claude` for one model. Two equal launches can share a process.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Launch {
    pub model: String,
    pub effort: Option<String>,
    /// Extra environment for the child process (may hold a key: never log).
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
}

impl Launch {
    /// Identity without secrets, for logs.
    pub fn describe(&self) -> String {
        let base = self.env.iter().find(|(k, _)| k == "ANTHROPIC_BASE_URL").map(|(_, v)| v.as_str()).unwrap_or("claude login");
        format!("model={} effort={} via {}", self.model, self.effort.as_deref().unwrap_or("-"), base)
    }
}

pub struct Catalog {
    entries: Vec<ModelEntry>,
    /// `BELUGA_MODEL_MAP` (v1): alias -> real model for `provider:"claude"` entries.
    model_map: Vec<(String, String)>,
    /// `BELUGA_EFFORT`: forces the effort for every model.
    effort_override: Option<String>,
}

impl Catalog {
    /// `models.json` when present (invalid file: log and use the built-ins), keys from `key`.
    pub fn load(path: &Path, key: impl Fn(&str) -> Option<String>, model_map: Vec<(String, String)>, effort_override: Option<String>) -> Catalog {
        let raw = match std::fs::read_to_string(path) {
            Ok(t) => match serde_json::from_str::<Vec<ModelEntry>>(&t) {
                Ok(v) if !v.is_empty() => v,
                Ok(_) => builtin(),
                Err(e) => {
                    tracing::error!("{} is not valid ({e}); using built-in models", path.display());
                    builtin()
                }
            },
            Err(_) => builtin(),
        };
        Catalog::from_entries(raw, key, model_map, effort_override)
    }

    pub fn from_entries(raw: Vec<ModelEntry>, key: impl Fn(&str) -> Option<String>, model_map: Vec<(String, String)>, effort_override: Option<String>) -> Catalog {
        let mut entries = Vec::new();
        for mut e in raw {
            if entries.iter().any(|x: &ModelEntry| x.id == e.id) {
                continue;
            }
            if let Some(k) = &e.key_env {
                match key(k) {
                    Some(v) => e.key = Some(v),
                    None => continue, // no key: the model is not offered
                }
            }
            if e.provider != "claude" && e.base_url.is_none() {
                tracing::warn!("model {} has no baseUrl; skipped", e.id);
                continue;
            }
            e.efforts.retain(|x| EFFORTS.contains(&x.as_str()));
            entries.push(e);
        }
        Catalog { entries, model_map, effort_override }
    }

    pub fn entries(&self) -> &[ModelEntry] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&ModelEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    pub fn default_id(&self) -> String {
        if self.get("sonnet").is_some() {
            return "sonnet".into();
        }
        self.entries.first().map(|e| e.id.clone()).unwrap_or_else(|| "sonnet".into())
    }

    /// The entry for `id`, or the default one.
    pub fn resolve(&self, id: &str) -> ModelEntry {
        self.get(id)
            .or_else(|| self.get(&self.default_id()))
            .cloned()
            .unwrap_or_else(|| builtin().remove(0))
    }

    pub fn public(&self) -> Value {
        Value::Array(self.entries.iter().map(|e| e.public()).collect())
    }

    /// Effort shown in `ready.effort` and passed to `--effort`.
    pub fn effort_for(&self, e: &ModelEntry, wanted: Option<&str>) -> Option<String> {
        if let Some(o) = &self.effort_override {
            return Some(o.clone());
        }
        e.effort(wanted)
    }

    pub fn launch(&self, e: &ModelEntry, wanted_effort: Option<&str>) -> Launch {
        let effort = self.effort_for(e, wanted_effort);
        if e.provider == "claude" {
            let model = self.model_map.iter().find(|(f, _)| *f == e.model).map(|(_, t)| t.clone()).unwrap_or_else(|| e.model.clone());
            // Built-in aliases use the CLI's own login (Pro plan), never a stray API key. A model
            // redirected through `BELUGA_MODEL_MAP` goes through the user's own gateway, whose
            // `ANTHROPIC_BASE_URL`/`ANTHROPIC_AUTH_TOKEN` must survive.
            let gateway = !matches!(model.as_str(), "sonnet" | "opus" | "haiku");
            let env_remove = if gateway { vec![] } else { vec!["ANTHROPIC_API_KEY".into(), "ANTHROPIC_AUTH_TOKEN".into()] };
            return Launch { model, effort, env: vec![], env_remove };
        }
        let mut env = vec![
            ("ANTHROPIC_BASE_URL".to_string(), e.base_url.clone().unwrap_or_default()),
            ("ANTHROPIC_AUTH_TOKEN".to_string(), e.key.clone().unwrap_or_default()),
            // Background calls (titles, summaries inside Claude Code) must not ask for a Claude
            // model name the provider does not know.
            ("ANTHROPIC_DEFAULT_HAIKU_MODEL".to_string(), e.model.clone()),
            ("ANTHROPIC_DEFAULT_SONNET_MODEL".to_string(), e.model.clone()),
            ("ANTHROPIC_DEFAULT_OPUS_MODEL".to_string(), e.model.clone()),
            ("CLAUDE_CODE_SUBAGENT_MODEL".to_string(), e.model.clone()),
            ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".to_string(), "1".to_string()),
        ];
        env.sort();
        Launch { model: e.model.clone(), effort, env, env_remove: vec!["ANTHROPIC_API_KEY".into()] }
    }

    /// Cost of one turn: list price from the catalogue when it has one, else the CLI's estimate.
    pub fn turn_cost(e: &ModelEntry, input: u64, output: u64, cache_read: u64, cache_write: u64, cli_estimate: f64) -> f64 {
        match (&e.price, e.tracked()) {
            (Some(p), false) => (input as f64 * p.input + output as f64 * p.output + cache_read as f64 * p.cache_read + cache_write as f64 * p.cache_write) / 1e6,
            _ => cli_estimate,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(k: &str) -> Option<String> {
        (k == "DEEPSEEK_API_KEY").then(|| "sk-test".to_string())
    }

    #[test]
    fn entries_without_keys_are_hidden_and_public_view_has_no_secrets() {
        let c = Catalog::from_entries(builtin(), keys, vec![], None);
        let ids: Vec<&str> = c.entries().iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, ["sonnet", "opus", "haiku", "deepseek"]);
        let p = c.public().to_string();
        assert!(!p.contains("sk-test") && !p.contains("api.deepseek.com"), "{p}");
        assert_eq!(c.public()[0]["tracked"], true);
        assert_eq!(c.public()[3]["tracked"], false);
        assert_eq!(c.public()[0]["defaultEffort"], "medium");
        assert_eq!(c.default_id(), "sonnet");
        assert_eq!(c.resolve("nope").id, "sonnet");
    }

    #[test]
    fn launch_per_provider() {
        let c = Catalog::from_entries(builtin(), keys, vec![], None);
        let l = c.launch(c.get("sonnet").unwrap(), None);
        assert_eq!((l.model.as_str(), l.effort.as_deref()), ("sonnet", Some("medium")));
        assert!(l.env.is_empty() && l.env_remove.contains(&"ANTHROPIC_AUTH_TOKEN".to_string()));
        assert_eq!(c.launch(c.get("sonnet").unwrap(), Some("high")).effort.as_deref(), Some("high"));
        assert_eq!(c.launch(c.get("sonnet").unwrap(), Some("max")).effort.as_deref(), Some("medium"), "unsupported -> default");
        assert_eq!(c.launch(c.get("opus").unwrap(), None).effort.as_deref(), Some("low"));
        assert_eq!(c.launch(c.get("haiku").unwrap(), Some("high")).effort, None);
        let d = c.launch(c.get("deepseek").unwrap(), None);
        let env = |k: &str| d.env.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone());
        assert_eq!(env("ANTHROPIC_BASE_URL").as_deref(), Some("https://api.deepseek.com/anthropic"));
        assert_eq!(env("ANTHROPIC_AUTH_TOKEN").as_deref(), Some("sk-test"));
        assert_eq!(env("ANTHROPIC_DEFAULT_HAIKU_MODEL").as_deref(), Some("deepseek-flash"));
        assert_eq!(d.model, "deepseek-flash");
        assert!(!d.describe().contains("sk-test"));
    }

    #[test]
    fn model_map_and_effort_override_still_apply() {
        let c = Catalog::from_entries(builtin(), keys, vec![("opus".into(), "ds/deepseek-flash".into())], Some("high".into()));
        let l = c.launch(c.get("opus").unwrap(), None);
        assert_eq!(l.model, "ds/deepseek-flash");
        assert!(l.env_remove.is_empty(), "gateway auth survives");
        assert_eq!(l.effort.as_deref(), Some("high"));
        assert_eq!(c.launch(c.get("haiku").unwrap(), None).effort.as_deref(), Some("high"));
    }

    #[test]
    fn example_file_matches_builtins_and_bad_files_fall_back() {
        let example = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("models.example.json")).unwrap();
        let parsed: Vec<ModelEntry> = serde_json::from_str(&example).unwrap();
        assert_eq!(parsed, builtin());
        let d = std::env::temp_dir().join(format!("beluga-models-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&d, "not json").unwrap();
        assert_eq!(Catalog::load(&d, |_| None, vec![], None).entries().len(), 3);
        std::fs::write(&d, r#"[{"id":"x","label":"X","model":"haiku"}]"#).unwrap();
        let c = Catalog::load(&d, |_| None, vec![], None);
        assert_eq!((c.default_id().as_str(), c.get("x").unwrap().provider.as_str()), ("x", "claude"));
    }

    #[test]
    fn cost_uses_price_for_other_providers_only() {
        let c = Catalog::from_entries(builtin(), keys, vec![], None);
        let d = c.get("deepseek").unwrap();
        assert!((Catalog::turn_cost(d, 1_000_000, 0, 0, 0, 9.0) - 0.05).abs() < 1e-9);
        assert_eq!(Catalog::turn_cost(c.get("sonnet").unwrap(), 1, 1, 0, 0, 0.3), 0.3);
    }
}
