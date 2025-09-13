use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::string::String;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("validation error: {0}")]
    Validation(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub linter: LinterCfg,
    #[serde(default)]
    pub ruleset: HashMap<String, RulesetCfg>,
}

impl Config {
    /// Build a default config (no rulesets, default linter).
    pub fn from_default() -> Self {
        Self {
            linter: LinterCfg::default(),
            ruleset: HashMap::new(),
        }
    }

    pub fn load_from_path<P: AsRef<std::path::Path>>(path: P) -> Result<Self, ConfigError> {
        let raw = std::fs::read_to_string(path)?;
        Self::load_from_str(&raw)
    }

    pub fn load_from_str(raw: &str) -> Result<Self, ConfigError> {
        let mut cfg: Config = toml::from_str(raw)?;
        cfg.apply_defaults();
        cfg.validate()?;
        Ok(cfg)
    }

    fn apply_defaults(&mut self) {
        // Nothing needed here because serde defaults cover everything,
        // but this hook is nice if you add computed defaults later.
    }

    fn validate(&self) -> Result<(), ConfigError> {
        // Keys are unique by virtue of HashMap. Add rules here if needed.
        // Example: ensure at least one enabled engine/ruleset (optional):
        // if !self.engine.values().any(|e| e.enabled) { ... }
        Ok(())
    }

    /// Merge overrides from OS environment (std::env::var).
    pub fn merge_env_overrides_from_os(&mut self) {
        self.merge_env_overrides(|k| std::env::var(k).ok());
    }

    /// Merge overrides from a custom getter (useful for tests).
    pub fn merge_env_overrides<F: Fn(&str) -> Option<String>>(&mut self, get: F) {
        // ---- LINTER ----
        if let Some(v) = get("FORSETI_LINTER_LOG_LEVEL")
            && let Ok(parsed) = parse_log_level(&v)
        {
            self.linter.log_level = parsed;
        }
        if let Some(v) = get("FORSETI_LINTER_OUTPUT_FORMAT")
            && let Ok(parsed) = parse_output_format(&v)
        {
            self.linter.output_format = parsed;
        }
        if let Some(v) = get("FORSETI_LINTER_PARALLELISM")
            && let Ok(n) = v.parse::<u16>()
        {
            self.linter.parallelism = n;
        }
        if let Some(v) = get("FORSETI_LINTER_FAIL_ON_ERROR")
            && let Ok(b) = parse_bool(&v)
        {
            self.linter.fail_on_error = b;
        }


        // ---- RULESETS ----
        if let Some(ids) = get("FORSETI_RULESET_IDS") {
            for id in parse_csv_ids(&ids) {
                self.ruleset.entry(id).or_default();
            }
        }

        let ruleset_keys: Vec<String> = self.ruleset.keys().cloned().collect();
        for id in ruleset_keys {
            let k_enabled = format!("FORSETI_RULESET_{}_ENABLED", upper(&id));
            if let Some(v) = get(&k_enabled)
                && let Ok(b) = parse_bool(&v)
                && let Some(cfg) = self.ruleset.get_mut(&id)
            {
                cfg.enabled = b;
            }

            let k_cfg = format!("FORSETI_RULESET_{}_CONFIG_JSON", upper(&id));
            if let Some(v) = get(&k_cfg)
                && let Ok(json) = serde_json::from_str::<serde_json::Value>(&v)
                && let Some(obj) = json.as_object()
                && let Some(rs) = self.ruleset.get_mut(&id)
            {
                merge_json_object_into_toml_table(obj, &mut rs.config);
            }
        }
    }
}

// ⬇️ Helpers (private to this module)
fn parse_csv_ids(s: &str) -> Vec<String> {
    s.split(',')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| p.to_string())
        .collect()
}

fn parse_bool(s: &str) -> Result<bool, ()> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(()),
    }
}

fn parse_log_level(s: &str) -> Result<LogLevel, ()> {
    match s.trim().to_ascii_lowercase().as_str() {
        "trace" => Ok(LogLevel::Trace),
        "debug" => Ok(LogLevel::Debug),
        "info" => Ok(LogLevel::Info),
        "warn" => Ok(LogLevel::Warn),
        "error" => Ok(LogLevel::Error),
        _ => Err(()),
    }
}

fn parse_output_format(s: &str) -> Result<OutputFormat, ()> {
    match s.trim().to_ascii_lowercase().as_str() {
        "json" => Ok(OutputFormat::Json),
        "ndjson" => Ok(OutputFormat::Ndjson),
        "text" => Ok(OutputFormat::Text),
        "sarif" => Ok(OutputFormat::Sarif),
        _ => Err(()),
    }
}

fn upper(id: &str) -> String {
    id.replace(|c: char| !c.is_ascii_alphanumeric(), "_")
        .to_ascii_uppercase()
}

/// Merge a JSON object shallowly into a TOML table.
fn merge_json_object_into_toml_table(
    json_obj: &serde_json::Map<String, serde_json::Value>,
    toml_tbl: &mut toml::value::Table,
) {
    for (k, v) in json_obj {
        if let Some(tv) = json_to_toml_value(v) {
            toml_tbl.insert(k.clone(), tv);
        }
    }
}

fn json_to_toml_value(v: &serde_json::Value) -> Option<toml::Value> {
    use serde_json::Value::*;
    Some(match v {
        Null => toml::Value::String(std::string::String::new()), // or return None to skip nulls
        Bool(b) => toml::Value::Boolean(*b),
        Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                toml::Value::Float(f)
            } else {
                return None;
            }
        }
        String(s) => toml::Value::String(s.clone()),
        Array(a) => {
            let mut out = Vec::with_capacity(a.len());
            for item in a {
                if let Some(tv) = json_to_toml_value(item) {
                    out.push(tv)
                }
            }
            toml::Value::Array(out)
        }
        Object(o) => {
            let mut tbl = toml::map::Map::new();
            for (kk, vv) in o {
                if let Some(tv) = json_to_toml_value(vv) {
                    tbl.insert(kk.clone(), tv);
                }
            }
            toml::Value::Table(tbl)
        }
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct LinterCfg {
    #[serde(default)]
    pub log_level: LogLevel,
    #[serde(default)]
    pub output_format: OutputFormat,
    /// 0 => auto
    #[serde(default)]
    pub parallelism: u16,
    #[serde(default = "default_fail_on_error")]
    pub fail_on_error: bool,
}
fn default_fail_on_error() -> bool {
    true
}
impl Default for LinterCfg {
    fn default() -> Self {
        Self {
            log_level: LogLevel::Info,
            output_format: OutputFormat::Json,
            parallelism: 0,
            fail_on_error: true,
        }
    }
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct RulesetCfg {
    /// Defaults to true when omitted
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Opaque, free-form table; defaults to {}
    #[serde(default)]
    pub config: toml::value::Table,
    /// Optional git repository URL to clone and build from source
    #[serde(default)]
    pub git: Option<String>,
    /// Optional local path to binary executable
    #[serde(default)]
    pub path: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OutputFormat {
    #[default]
    Json,
    Ndjson,
    Text,
    Sarif,
}
