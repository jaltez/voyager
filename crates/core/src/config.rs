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
    /// Result-language hint (ISO code) applied to every research search.
    pub language: Option<String>,
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
            language: None,
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
            // arXiv's API asks for ~3s spacing as a courtesy.
            overrides: BTreeMap::from([("ddgs".to_string(), 1_200), ("arxiv".to_string(), 3_000)]),
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
/// without resetting untouched sections to defaults. Sections carry only
/// the fields actually present (`*Overlay`) so that a project file setting
/// `breadth` no longer resets user-file `depth_max` back to defaults.
#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    default_provider: Option<String>,
    providers: Option<BTreeMap<String, ProviderConf>>,
    llm: Option<LlmOverlay>,
    research: Option<ResearchOverlay>,
    politeness: Option<PolitenessOverlay>,
    cache: Option<CacheOverlay>,
}

/// Present-fields-only view of `[llm]`.
#[derive(Debug, Default, Deserialize)]
struct LlmOverlay {
    backend: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    api_key_env: Option<String>,
}

impl LlmOverlay {
    fn apply(self, to: &mut LlmConf) {
        if self.backend.is_some() {
            to.backend = self.backend;
        }
        if self.model.is_some() {
            to.model = self.model;
        }
        if self.base_url.is_some() {
            to.base_url = self.base_url;
        }
        if self.api_key_env.is_some() {
            to.api_key_env = self.api_key_env;
        }
    }
}

/// Present-fields-only view of `[research]`.
#[derive(Debug, Default, Deserialize)]
struct ResearchOverlay {
    depth_min: Option<u32>,
    depth_max: Option<u32>,
    breadth: Option<u32>,
    budget_usd: Option<f64>,
    max_results_per_query: Option<usize>,
    fetch_top: Option<usize>,
    context_max_chars: Option<usize>,
    run_dir: Option<String>,
    time_range: Option<String>,
    include_domains: Option<Vec<String>>,
    exclude_domains: Option<Vec<String>>,
    language: Option<String>,
}

impl ResearchOverlay {
    fn apply(self, to: &mut ResearchConf) {
        if let Some(v) = self.depth_min {
            to.depth_min = v;
        }
        if let Some(v) = self.depth_max {
            to.depth_max = v;
        }
        if let Some(v) = self.breadth {
            to.breadth = v;
        }
        if let Some(v) = self.budget_usd {
            to.budget_usd = Some(v);
        }
        if let Some(v) = self.max_results_per_query {
            to.max_results_per_query = v;
        }
        if let Some(v) = self.fetch_top {
            to.fetch_top = v;
        }
        if let Some(v) = self.context_max_chars {
            to.context_max_chars = v;
        }
        if let Some(v) = self.run_dir {
            to.run_dir = Some(v);
        }
        if let Some(v) = self.time_range {
            to.time_range = Some(v);
        }
        if let Some(v) = self.include_domains {
            to.include_domains = v;
        }
        if let Some(v) = self.exclude_domains {
            to.exclude_domains = v;
        }
        if let Some(v) = self.language {
            to.language = Some(v);
        }
    }
}

/// Present-fields-only view of `[politeness]`.
#[derive(Debug, Default, Deserialize)]
struct PolitenessOverlay {
    min_interval_ms_default: Option<u64>,
    max_retries: Option<u32>,
    overrides: Option<BTreeMap<String, u64>>,
}

impl PolitenessOverlay {
    fn apply(self, to: &mut PolitenessConf) {
        if let Some(v) = self.min_interval_ms_default {
            to.min_interval_ms_default = v;
        }
        if let Some(v) = self.max_retries {
            to.max_retries = v;
        }
        if let Some(overrides) = self.overrides {
            // Extend, so user overrides add to (or replace) the ddgs
            // default instead of dropping it.
            to.overrides.extend(overrides);
        }
    }
}

/// Present-fields-only view of `[cache]`.
#[derive(Debug, Default, Deserialize)]
struct CacheOverlay {
    enabled: Option<bool>,
    ttl_news_secs: Option<u64>,
    ttl_standard_secs: Option<u64>,
    ttl_reference_secs: Option<u64>,
}

impl CacheOverlay {
    fn apply(self, to: &mut CacheConf) {
        if let Some(v) = self.enabled {
            to.enabled = v;
        }
        if let Some(v) = self.ttl_news_secs {
            to.ttl_news_secs = v;
        }
        if let Some(v) = self.ttl_standard_secs {
            to.ttl_standard_secs = v;
        }
        if let Some(v) = self.ttl_reference_secs {
            to.ttl_reference_secs = v;
        }
    }
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
                    llm.apply(&mut self.llm);
                }
                if let Some(research) = file.research {
                    research.apply(&mut self.research);
                }
                if let Some(politeness) = file.politeness {
                    politeness.apply(&mut self.politeness);
                }
                if let Some(cache) = file.cache {
                    cache.apply(&mut self.cache);
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
        assert_eq!(file.research.unwrap().breadth, Some(5));
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
        // A file section only overrides the fields it actually sets; the
        // ddgs spacing default survives partial politeness overrides.
        let file: FileConfig = toml::from_str("[politeness]\nmax_retries = 0\n").unwrap();
        let overlay = file.politeness.unwrap();
        assert_eq!(overlay.max_retries, Some(0));
        assert_eq!(overlay.min_interval_ms_default, None);
        assert!(overlay.overrides.is_none());
    }

    #[test]
    fn merge_preserves_fields_from_other_layers() {
        // User layer sets depth and breadth; project layer only touches
        // breadth. The depth setting must survive the overlay.
        let mut cfg = Config::default();
        cfg.research.depth_max = 6;
        cfg.research.breadth = 5;
        let file: FileConfig = toml::from_str("[research]\nbreadth = 2\n").unwrap();
        file.research.unwrap().apply(&mut cfg.research);
        assert_eq!(cfg.research.depth_max, 6);
        assert_eq!(cfg.research.breadth, 2);

        // Politeness overrides extend instead of clobbering the ddgs
        // spacing default.
        let mut cfg2 = Config::default();
        let file: FileConfig =
            toml::from_str("[politeness]\nmax_retries = 0\n[politeness.overrides]\nbrave = 400\n")
                .unwrap();
        file.politeness.unwrap().apply(&mut cfg2.politeness);
        assert_eq!(cfg2.politeness.max_retries, 0);
        assert_eq!(cfg2.politeness.overrides.get("ddgs"), Some(&1_200));
        assert_eq!(cfg2.politeness.overrides.get("brave"), Some(&400));
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
        assert_eq!(file.cache.unwrap().enabled, Some(false));
    }
}
