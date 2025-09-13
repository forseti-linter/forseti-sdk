use std::collections::HashMap;
use std::io::{self};

use serde::{Deserialize, Serialize}; // <— add this
use serde_json::{Value, json};

use crate::core::{AnnotationParser, Diagnostic, Envelope, Ndjson, read_line_value};
use crate::core::{EngineCapabilities, EngineCfg, PreprocessingContext, SharedConfig};
use crate::ruleset::{Ruleset, run_ruleset_with_annotations};

#[derive(Debug, Clone, Default, Serialize, Deserialize)] // <— add Serialize, Deserialize
pub struct EngineConfig {
    pub enabled: Option<bool>,
    pub rulesets: Option<HashMap<String, Value>>, // ruleset id -> per-rule config map
}

pub trait EngineOptions: Send + Sync {
    fn get_default_config(&self) -> EngineConfig;
    fn load_ruleset(&self, id: &str) -> anyhow::Result<Ruleset>;

    /// Get engine capabilities (file patterns, version, etc.)
    fn get_capabilities(&self) -> EngineCapabilities;

    /// Preprocess files and return context for rulesets
    fn preprocess_files(&self, file_uris: &[String]) -> anyhow::Result<PreprocessingContext>;

    /// List all available ruleset IDs that this engine can load
    fn list_rulesets(&self) -> Vec<String>;
}

pub struct EngineServer {
    initialized: bool,
    cfg: EngineConfig,
    loaded: HashMap<String, Loaded>,
    opts: Box<dyn EngineOptions>,
    out: Ndjson<io::BufWriter<io::Stdout>>,
}

struct Loaded {
    ruleset: Ruleset,
    config: HashMap<String, Value>, // ruleId -> options (disabled rules omitted)
}

impl EngineServer {
    pub fn new(opts: Box<dyn EngineOptions>) -> Self {
        Self {
            initialized: false,
            cfg: EngineConfig::default(),
            loaded: HashMap::new(),
            opts,
            out: Ndjson::new(io::BufWriter::new(io::stdout())),
        }
    }

    pub fn run_stdio(&mut self) -> anyhow::Result<()> {
        while let Ok(v) = read_line_value() {
            let msg = v;
            let typ = msg.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let id = msg
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            match typ {
                "initialize" => {
                    self.on_initialize(&id, msg.get("payload").cloned().unwrap_or(json!({})))?
                }
                "shutdown" => self.on_shutdown(&id)?,
                "getDefaultConfig" => self.on_get_default_config(&id)?,
                "getCapabilities" => self.on_get_capabilities(&id)?,
                "preprocessFiles" => {
                    self.on_preprocess_files(&id, msg.get("payload").cloned().unwrap_or(json!({})))?
                }
                "analyzeFile" => {
                    self.on_analyze_file(&id, msg.get("payload").cloned().unwrap_or(json!({})))?
                }
                _ => {
                    self.log("warn", &format!("Unhandled message type: {typ}"));
                }
            }
        }
        Ok(())
    }

    fn send<T: serde::Serialize>(&mut self, obj: &T) {
        let _ = self.out.send(obj);
    }
    fn log(&mut self, level: &str, message: &str) {
        self.send(&Envelope::event(
            "log",
            json!({ "level": level, "message": message }),
        ));
    }

    fn on_initialize(&mut self, id: &str, payload: Value) -> anyhow::Result<()> {
        let defaults = self.opts.get_default_config();
        let user_cfg: EngineConfig =
            serde_json::from_value(payload.get("engineConfig").cloned().unwrap_or(json!({})))
                .unwrap_or_default();
        self.cfg = merge_engine_config(&defaults, &user_cfg);
        self.loaded.clear();
        if let Some(rs_map) = &self.cfg.rulesets {
            for (rs_id, cfg_entry) in rs_map {
                let ruleset = self.opts.load_ruleset(rs_id)?;
                let mut config: HashMap<String, Value> = HashMap::new();
                if let Some(obj) = cfg_entry.as_object() {
                    for (rule_id, setting) in obj {
                        // Interpret string "off" to disable; array/object => enabled with options
                        if let Some(s) = setting.as_str() {
                            if s == "off" {
                                continue;
                            } else {
                                config.insert(rule_id.clone(), json!({}));
                            }
                        } else if setting.is_array() {
                            let opts = setting.get(1).cloned().unwrap_or(json!({}));
                            // If level is off, skip
                            if setting.get(0).and_then(|x| x.as_str()) == Some("off") {
                                continue;
                            }
                            config.insert(rule_id.clone(), opts);
                        } else if setting.is_object() {
                            config.insert(rule_id.clone(), setting.clone());
                        }
                    }
                }
                self.loaded
                    .insert(rs_id.clone(), Loaded { ruleset, config });
            }
        }
        self.send(&Envelope::res(
            "initialize",
            id.to_string(),
            json!({"ok": true}),
        ));
        self.initialized = true;
        Ok(())
    }

    fn on_shutdown(&mut self, id: &str) -> anyhow::Result<()> {
        self.initialized = false;
        self.loaded.clear();
        self.send(&Envelope::res(
            "shutdown",
            id.to_string(),
            json!({"ok": true}),
        ));
        Ok(())
    }

    fn on_get_default_config(&mut self, id: &str) -> anyhow::Result<()> {
        let defaults = self.opts.get_default_config();
        self.send(&Envelope::res(
            "getDefaultConfig",
            id.to_string(),
            serde_json::to_value(defaults)?,
        ));
        Ok(())
    }

    fn on_get_capabilities(&mut self, id: &str) -> anyhow::Result<()> {
        let mut capabilities = self.opts.get_capabilities();

        // Collect rule information from all available rulesets
        let ruleset_ids = self.opts.list_rulesets();
        capabilities.rulesets = ruleset_ids.into_iter()
            .filter_map(|rs_id| {
                self.opts.load_ruleset(&rs_id).ok().map(|rs| rs.info())
            })
            .collect();

        self.send(&Envelope::res(
            "getCapabilities",
            id.to_string(),
            serde_json::to_value(capabilities)?,
        ));
        Ok(())
    }

    fn on_preprocess_files(&mut self, id: &str, payload: Value) -> anyhow::Result<()> {
        let file_uris: Vec<String> = payload
            .get("fileUris")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        match self.opts.preprocess_files(&file_uris) {
            Ok(context) => {
                self.send(&Envelope::res(
                    "preprocessFiles",
                    id.to_string(),
                    serde_json::to_value(context)?,
                ));
            }
            Err(e) => {
                self.send(&Envelope::res(
                    "preprocessFiles",
                    id.to_string(),
                    json!({"ok": false, "error": e.to_string()}),
                ));
            }
        }
        Ok(())
    }

    fn on_analyze_file(&mut self, id: &str, payload: Value) -> anyhow::Result<()> {
        if !self.initialized {
            self.send(&Envelope::res(
                "analyzeFile",
                id.to_string(),
                json!({"ok": false, "error": "not_initialized"}),
            ));
            return Ok(());
        }
        let uri = payload
            .get("uri")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let content = payload
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        // Get annotation prefixes from engine capabilities
        let capabilities = self.opts.get_capabilities();
        let annotation_parser = if !capabilities.annotation_prefixes.is_empty() {
            Some(AnnotationParser::new(capabilities.annotation_prefixes))
        } else {
            None
        };

        // Parse annotations from content
        let annotations = if let Some(ref parser) = annotation_parser {
            parser.parse_annotations(&content)
        } else {
            Vec::new()
        };

        let mut diagnostics: Vec<Diagnostic> = Vec::new();
        for loaded in self.loaded.values() {
            let diags = run_ruleset_with_annotations(
                &uri,
                &content,
                &loaded.ruleset,
                &loaded.config,
                &annotations,
                annotation_parser.as_ref(),
            );
            diagnostics.extend(diags);
        }
        self.send(&Envelope::event(
            "diagnostics",
            json!({ "uri": uri, "diagnostics": diagnostics }),
        ));
        self.send(&Envelope::res(
            "analyzeFile",
            id.to_string(),
            json!({"ok": true}),
        ));
        Ok(())
    }
}

pub fn merge_engine_config(defaults: &EngineConfig, user: &EngineConfig) -> EngineConfig {
    let enabled = user.enabled.or(defaults.enabled).or(Some(true));
    let mut rulesets = defaults.rulesets.clone().unwrap_or_default();
    if let Some(u) = &user.rulesets {
        for (k, v) in u {
            rulesets.insert(k.clone(), v.clone());
        }
    }
    EngineConfig {
        enabled,
        rulesets: Some(rulesets),
    }
}

pub fn enabled_engines(cfg: &SharedConfig) -> impl Iterator<Item = (&String, &EngineCfg)> {
    cfg.get().engine.iter().filter(|(_, e)| e.enabled)
}
