//! models.dev catalog access (ADR-0004): provider discovery, API-key env
//! var names, OpenAI-compatible base URLs and per-model pricing used for
//! cost estimation. The catalog is cached on disk for 24 hours.

use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde_json::Value;
use vygr_core::VygrError;

const CATALOG_URL: &str = "https://models.dev/api.json";
const TTL: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone, Copy)]
pub struct CostPerMtok {
    /// USD per 1M input tokens.
    pub input: f64,
    /// USD per 1M output tokens.
    pub output: f64,
}

#[derive(Debug, Clone)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    pub env: Vec<String>,
    pub api: Option<String>,
    pub model_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Catalog {
    json: Value,
}

impl Catalog {
    pub fn provider(&self, id: &str) -> Option<ProviderInfo> {
        let node = self.json.get(id)?;
        Some(ProviderInfo {
            id: id.to_string(),
            name: node
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(id)
                .to_string(),
            env: node
                .get("env")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            api: node.get("api").and_then(Value::as_str).map(str::to_string),
            model_ids: node
                .get("models")
                .and_then(Value::as_object)
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default(),
        })
    }

    /// First model of a provider that supports tool calling (used as a
    /// sane default when none is given).
    pub fn tool_call_default_model(&self, id: &str) -> Option<String> {
        let models = self.json.get(id)?.get("models")?.as_object()?;
        models
            .iter()
            .find(|(_, m)| m.get("tool_call").and_then(Value::as_bool).unwrap_or(false))
            .map(|(k, _)| k.clone())
    }

    /// Per-million-token pricing for a provider/model pair.
    pub fn cost(&self, id: &str, model: &str) -> Option<CostPerMtok> {
        let cost = self.json.get(id)?.get("models")?.get(model)?.get("cost")?;
        Some(CostPerMtok {
            input: cost.get("input").and_then(Value::as_f64).unwrap_or(0.0),
            output: cost.get("output").and_then(Value::as_f64).unwrap_or(0.0),
        })
    }

    /// Summary rows for `vygr models`: `(id, name, env, openai_compatible)`.
    pub fn provider_rows(&self) -> Vec<(String, String, Option<String>, bool)> {
        let mut rows: Vec<_> = self
            .json
            .as_object()
            .map(|m| m.iter())
            .into_iter()
            .flatten()
            .filter_map(|(id, node)| {
                if id.starts_with("$") {
                    return None; // metadata keys like "$schema"
                }
                Some((
                    id.clone(),
                    node.get("name")
                        .and_then(Value::as_str)
                        .unwrap_or(id)
                        .to_string(),
                    node.get("env")
                        .and_then(Value::as_array)
                        .and_then(|a| a.first())
                        .and_then(Value::as_str)
                        .map(str::to_string),
                    node.get("api").and_then(Value::as_str).is_some(),
                ))
            })
            .collect();
        rows.sort();
        rows
    }
}

/// Load the catalog from cache or network (24h TTL, best-effort caching).
pub async fn load(http: &reqwest::Client) -> Result<Catalog, VygrError> {
    let cache: Option<PathBuf> =
        vygr_core::config::Config::cache_dir().map(|d| d.join("models-dev.json"));

    if let Some(path) = &cache {
        if let Ok(meta) = std::fs::metadata(path) {
            let fresh = meta
                .modified()
                .ok()
                .and_then(|m| SystemTime::now().duration_since(m).ok())
                .is_some_and(|age| age < TTL);
            if fresh {
                if let Ok(text) = std::fs::read_to_string(path) {
                    return Ok(Catalog {
                        json: serde_json::from_str(&text)
                            .map_err(|e| VygrError::Parse(format!("cached models.dev: {e}")))?,
                    });
                }
            }
        }
    }

    let resp = http
        .get(CATALOG_URL)
        .timeout(std::time::Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| VygrError::Network(format!("fetching models.dev: {e}")))?;
    if !resp.status().is_success() {
        return Err(VygrError::Provider {
            provider: "models.dev".into(),
            message: format!("HTTP {}", resp.status()),
        });
    }
    let json: Value = resp
        .json()
        .await
        .map_err(|e| VygrError::Parse(format!("models.dev response: {e}")))?;

    if let Some(path) = &cache {
        if let Some(dir) = path.parent() {
            if std::fs::create_dir_all(dir).is_ok() {
                if let Ok(text) = serde_json::to_string(&json) {
                    let _ = std::fs::write(path, text);
                }
            }
        }
    }

    Ok(Catalog { json })
}
