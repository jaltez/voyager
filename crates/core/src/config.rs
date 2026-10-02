//! Configuration loading and precedence (user -> project -> env overrides).
//!
//! API keys are never written by vygr; they are read from the environment
//! only (each provider documents its variable, see `vygr providers`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Effective configuration after merging all layers.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Config {
    pub default_provider: Option<String>,
    pub providers: BTreeMap<String, ProviderConf>,
    pub llm: LlmConf,
    pub research: ResearchConf,
    pub politeness: PolitenessConf,
    pub cache: CacheConf,
    /// Files that contributed to this configuration, in precedence order.
    #[serde(skip)]
    pub sources: Vec<PathBuf>,
}

/// Per-provider toggles and endpoint overrides (M1.4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConf {
    pub enabled: Option<bool>,
    /// Endpoint override; currently only searxng needs it (instance URL).
    pub base_url: Option<String>,
}

/// LLM backend selection (ADR-0004).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConf {
    /// `"pi"`, `"claude"`, `"codex"`, `"ollama"`, `"openai-compat"`,
    /// or any models.dev provider id (e.g. `"openrouter"`, `"groq"`).
    pub backend: Option<String>,
    pub model: Option<String>,
    /// Override the base URL (used with `openai-compat` and `ollama`).
    pub base_url: Option<String>,
    /// Environment variable holding the API key (defaults per provider).
    pub api_key_env: Option<String>,
}

/// Research loop defaults (ADR-0005, ADR-0007).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ResearchConf {
    pub depth_min: u32,
    pub depth_max: u32,
    pub breadth: u32,
    pub budget_usd: Option<f64>,
    pub max_results_per_query: usize,
    pub fetch_top: usize,
    pub context_max_chars: usize,
    /// Base directory for run artifacts; defaults to `./agents/voyager`.
    pub run_dir: Option<String>,
    /// Freshness filter for research searches ("day|week|month|year").
    pub time_range: Option<String>,
    /// Domain allowlist/blocklist applied to every research search.
    pub include_domains: Vec<String>,
    pub exclude_domains: Vec<String>,
}

impl Default for ResearchConf {
    fn default() -> Self {
        Self {
            depth_min: 2,
            depth_max: 4,
            breadth: 3,
            budget_usd: None,
            max_results_per_query: 5,
            fetch_top: 4,
            context_max_chars: 24_000,
            run_dir: None,
            time_range: None,
            include_domains: Vec::new(),
            exclude_domains: Vec::new(),
        }
    }
}

/// Request spacing and retry policy (ADR-0010, milestone M1.1).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PolitenessConf {
    /// Minimum interval between requests to the same provider.
    pub min_interval_ms_default: u64,
    /// Retries with exponential backoff for retriable failures (429/5xx).
    pub max_retries: u32,
    /// Per-provider interval overrides in milliseconds.
    pub overrides: BTreeMap<String, u64>,
}

impl Default for PolitenessConf {
    fn default() -> Self {
        Self {
            min_interval_ms_default: 250,
            max_retries: 2,
            // DuckDuckGo's HTML endpoint serves anti-bot 202 pages when
            // queried faster than roughly one request per second.
            overrides: BTreeMap::from([("ddgs".to_string(), 1_200)]),
        }
    }
}

/// Disk cache with query-class TTLs (ADR-0010, milestone M1.2).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CacheConf {
    pub enabled: bool,
    pub ttl_news_secs: u64,
    pub ttl_standard_secs: u64,
    pub ttl_reference_secs: u64,
}

impl Default for CacheConf {
    fn default() -> Self {
        Self {
            enabled: true,
            ttl_news_secs: 300,
            ttl_standard_secs: 3_600,
            ttl_reference_secs: 86_400,
        }
    }
}

/// Everything the provider stack needs besides the query itself: a
/// convenience view over the config sections, passed to `build_chain`.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SearchStackConf {
    pub politeness: PolitenessConf,
    pub cache: CacheConf,
    pub providers: BTreeMap<String, ProviderConf>,
}

impl SearchStackConf {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            politeness: cfg.politeness.clone(),
            cache: cfg.cache.clone(),
            providers: cfg.providers.clone(),
        }
    }
}

/// Raw file representation; sections are optional so we can overlay files
/// without resetting untouched sections to defaults.
#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    default_provider: Option<String>,
    providers: Option<BTreeMap<String, ProviderConf>>,
    llm: Option<LlmConf>,
    research: Option<ResearchConf>,
    politeness: Option<PolitenessConf>,
    cache: Option<CacheConf>,
}

impl Config {
    pub fn user_config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("voyager").join("config.toml"))
    }

    pub fn cache_dir() -> Option<PathBuf> {
        dirs::cache_dir().map(|d| d.join("voyager"))
    }

    /// Load the effective configuration: defaults <- user file <- project
    /// file <- `VGR_*` environment overrides.
    pub fn load() -> Config {
        let mut cfg = Config::default();

        if let Some(path) = Self::user_config_path().filter(|p| p.exists()) {
            cfg.merge_file(&path);
        }
        if let Some(path) = find_project_config() {
            cfg.merge_file(&path);
        }

        if let Ok(v) = std::env::var("VGR_DEFAULT_PROVIDER") {
            cfg.default_provider = Some(v);
        }
        if let Ok(v) = std::env::var("VGR_LLM_BACKEND") {
            cfg.llm.backend = Some(v);
        }
        if let Ok(v) = std::env::var("VGR_LLM_MODEL") {
            cfg.llm.model = Some(v);
        }

        cfg
    }

    fn merge_file(&mut self, path: &Path) {
        match std::fs::read_to_string(path)
            .map_err(|e| e.to_string())
            .and_then(|s| toml::from_str::<FileConfig>(&s).map_err(|e| e.to_string()))
        {
            Ok(file) => {
                if file.default_provider.is_some() {
                    self.default_provider = file.default_provider;
                }
                if let Some(providers) = file.providers {
                    self.providers.extend(providers);
                }
                if let Some(llm) = file.llm {
                    self.llm = llm;
                }
                if let Some(research) = file.research {
                    self.research = research;
                }
                if let Some(politeness) = file.politeness {
                    self.politeness = politeness;
                }
                if let Some(cache) = file.cache {
                    self.cache = cache;
                }
                self.sources.push(path.to_path_buf());
            }
            Err(e) => {
                tracing::warn!("ignoring config file {}: {}", path.display(), e);
            }
        }
    }
}

fn find_project_config() -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let candidate = dir.join(".voyager.toml");
        if candidate.is_file() {
            return Some(candidate);
        }
        if !dir.pop() {
            return None;
        }
    }
}

/// Example configuration rendered in `vygr config` and docs.
pub fn example_toml() -> &'static str {
    r#"# ~/.config/voyager/config.toml
# Search: comma-separated fallback chain (first non-empty wins).
default_provider = "ddgs,brave"

[llm]
# Harness shell-out (reuses the agent's configured LLM), ollama, or any
# OpenAI-compatible provider from the models.dev catalog:
#   backend = "pi"                            # pi | claude | codex
#   backend = "ollama"
#   model   = "qwen3:8b"
backend = "openrouter"
model = "anthropic/claude-sonnet-4.5"

[research]
depth_min = 2
depth_max = 4
breadth = 3
budget_usd = 0.50
max_results_per_query = 5
fetch_top = 4
# run_dir = "./agents/voyager"

[politeness]
min_interval_ms_default = 250
max_retries = 2

[politeness.overrides]
# DuckDuckGo's HTML endpoint needs ~1 req/s to avoid anti-bot 202s.
ddgs = 1200

[cache]
enabled = true
ttl_news_secs = 300
ttl_standard_secs = 3600
ttl_reference_secs = 86400
"#
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sections_independently() {
        let file: FileConfig =
            toml::from_str("default_provider = \"brave\"\n[research]\nbreadth = 5\n").unwrap();
        assert_eq!(file.default_provider.as_deref(), Some("brave"));
        assert!(file.llm.is_none());
        assert_eq!(file.research.unwrap().breadth, 5);
    }

    #[test]
    fn research_defaults_are_sane() {
        let r = ResearchConf::default();
        assert_eq!((r.depth_min, r.depth_max, r.breadth), (2, 4, 3));
        assert_eq!(r.max_results_per_query, 5);
    }

    #[test]
    fn politeness_defaults_space_ddgs() {
        let p = PolitenessConf::default();
        assert_eq!(p.min_interval_ms_default, 250);
        assert_eq!(p.max_retries, 2);
        assert_eq!(p.overrides.get("ddgs"), Some(&1_200));
        // A file section replaces the section wholesale, so overrides
        // supplied by the user drop the ddgs default (documented behavior).
        let file: FileConfig = toml::from_str("[politeness]\nmax_retries = 0\n").unwrap();
        assert_eq!(file.politeness.unwrap().max_retries, 0);
    }

    #[test]
    fn cache_defaults_match_adr() {
        let c = CacheConf::default();
        assert!(c.enabled);
        assert_eq!(
            (c.ttl_news_secs, c.ttl_standard_secs, c.ttl_reference_secs),
            (300, 3_600, 86_400)
        );
        let file: FileConfig = toml::from_str("[cache]\nenabled = false\n").unwrap();
        assert!(!file.cache.unwrap().enabled);
    }
}
