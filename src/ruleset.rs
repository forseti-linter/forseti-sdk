use crate::core::{Annotation, AnnotationParser, Diagnostic, PreprocessingContext, RuleInfo, RulesetInfo, RulesetCapabilities, Envelope};
use crate::core::{RulesetCfg, SharedConfig};
use serde_json::{Value, json};
use std::collections::HashMap;
use anyhow::Result;

pub struct RuleContext<'a> {
    pub uri: &'a str,
    pub text: &'a str,
    pub options: &'a Value,
    pub diagnostics: Vec<Diagnostic>,
    pub annotations: &'a [Annotation],
    pub annotation_parser: Option<&'a AnnotationParser>,
}
impl<'a> RuleContext<'a> {
    pub fn report(&mut self, d: Diagnostic) {
        // Check if this diagnostic should be ignored based on annotations
        if let Some(parser) = self.annotation_parser {
            let line = d.range.start.line;
            if parser.should_ignore_rule(self.annotations, &d.rule_id, line) {
                return; // Skip this diagnostic
            }
        }
        self.diagnostics.push(d);
    }

    /// Check if a specific rule should be ignored for a given line
    pub fn should_ignore_rule(&self, rule_id: &str, line: u32) -> bool {
        if let Some(parser) = self.annotation_parser {
            parser.should_ignore_rule(self.annotations, rule_id, line)
        } else {
            false
        }
    }
}

pub trait Rule: Send + Sync {
    fn id(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn check(&self, ctx: &mut RuleContext);

    /// Default configuration for this rule (severity and options)
    fn default_config(&self) -> serde_json::Value {
        serde_json::Value::String("warn".to_string())
    }
}

/// Trait for ruleset-level capabilities and configuration
pub trait RulesetOptions: Send + Sync {
    /// Get ruleset capabilities (file patterns, version, etc.)
    fn get_capabilities(&self) -> RulesetCapabilities;

    /// Preprocess files and return context for rules
    fn preprocess_files(&self, file_uris: &[String]) -> Result<PreprocessingContext>;

    /// Create the ruleset with all its rules
    fn create_ruleset(&self) -> Ruleset;

    /// Get default configuration for this ruleset (auto-generated from rules and config_settings)
    fn get_default_config(&self) -> HashMap<String, Value> {
        let mut config = HashMap::new();

        // Get rule defaults
        let ruleset = self.create_ruleset();
        for rule in &ruleset.rules {
            config.insert(rule.id().to_string(), rule.default_config());
        }

        config
    }
}

pub struct Ruleset {
    pub id: String,
    pub rules: Vec<Box<dyn Rule>>,
}
impl Ruleset {
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            rules: vec![],
        }
    }
    pub fn with_rule(mut self, rule: Box<dyn Rule>) -> Self {
        self.rules.push(rule);
        self
    }

    /// Generate information about this ruleset and its rules
    pub fn info(&self) -> RulesetInfo {
        RulesetInfo {
            id: self.id.clone(),
            rules: self.rules.iter().map(|rule| RuleInfo {
                id: rule.id().to_string(),
                description: rule.description().to_string(),
            }).collect(),
        }
    }
}

pub fn run_ruleset(
    uri: &str,
    text: &str,
    rs: &Ruleset,
    options: &std::collections::HashMap<String, Value>,
) -> Vec<Diagnostic> {
    run_ruleset_with_annotations(uri, text, rs, options, &[], None)
}

/// Run ruleset with annotation support
pub fn run_ruleset_with_annotations(
    uri: &str,
    text: &str,
    rs: &Ruleset,
    options: &std::collections::HashMap<String, Value>,
    annotations: &[Annotation],
    annotation_parser: Option<&AnnotationParser>,
) -> Vec<Diagnostic> {
    let mut all = Vec::new();
    for r in &rs.rules {
        if let Some(opts) = options.get(r.id()) {
            let mut ctx = RuleContext {
                uri,
                text,
                options: opts,
                diagnostics: vec![],
                annotations,
                annotation_parser,
            };
            r.check(&mut ctx);
            all.extend(ctx.diagnostics);
        }
    }
    all
}

/// Run a ruleset with preprocessing context (new flow)
pub fn run_ruleset_with_context(
    rs: &Ruleset,
    preprocessing_context: &PreprocessingContext,
    options: &std::collections::HashMap<String, Value>,
) -> Vec<Diagnostic> {
    run_ruleset_with_context_and_annotations(rs, preprocessing_context, options, None)
}

/// Run a ruleset with preprocessing context and annotation support
pub fn run_ruleset_with_context_and_annotations(
    rs: &Ruleset,
    preprocessing_context: &PreprocessingContext,
    options: &std::collections::HashMap<String, Value>,
    annotation_parser: Option<&AnnotationParser>,
) -> Vec<Diagnostic> {
    let mut all = Vec::new();

    for file_context in &preprocessing_context.files {
        // Load file content on-demand only when needed
        let content = if file_context.content.is_empty() {
            load_file_content(&file_context.uri).unwrap_or_default()
        } else {
            file_context.content.clone()
        };

        // Parse annotations if parser is provided
        let annotations = if let Some(parser) = annotation_parser {
            parser.parse_annotations(&content)
        } else {
            Vec::new()
        };

        for rule in &rs.rules {
            if let Some(opts) = options.get(rule.id()) {
                let mut ctx = RuleContext {
                    uri: &file_context.uri,
                    text: &content,
                    options: opts,
                    diagnostics: vec![],
                    annotations: &annotations,
                    annotation_parser,
                };
                rule.check(&mut ctx);
                all.extend(ctx.diagnostics);
            }
        }
    }

    all
}

/// Load file content on-demand
fn load_file_content(uri: &str) -> Result<String, std::io::Error> {
    let path = if uri.starts_with("file://") {
        uri.strip_prefix("file://").unwrap_or(uri)
    } else {
        uri
    };
    std::fs::read_to_string(path)
}

/// Ruleset server that handles NDJSON protocol communication
pub struct RulesetServer {
    initialized: bool,
    config: HashMap<String, Value>,
    ruleset: Option<Ruleset>,
    opts: Box<dyn RulesetOptions>,
    out: crate::core::Ndjson<std::io::BufWriter<std::io::Stdout>>,
}

impl RulesetServer {
    pub fn new(opts: Box<dyn RulesetOptions>) -> Self {
        Self {
            initialized: false,
            config: HashMap::new(),
            ruleset: None,
            opts,
            out: crate::core::Ndjson::new(std::io::BufWriter::new(std::io::stdout())),
        }
    }

    pub fn run_stdio(&mut self) -> Result<()> {
        use crate::core::read_line_value;

        loop {
            let msg: serde_json::Value = match read_line_value() {
                Ok(v) => v,
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
                Err(e) => return Err(anyhow::anyhow!("Failed to read input: {}", e)),
            };

            let envelope: Envelope<serde_json::Value> = serde_json::from_value(msg)?;
            let msg_type = envelope.typ.as_str();
            let id = envelope.id.unwrap_or_default();

            match msg_type {
                "initialize" => {
                    self.on_initialize(&id, envelope.payload.unwrap_or(json!({})))?
                }
                "shutdown" => self.on_shutdown(&id)?,
                "getDefaultConfig" => self.on_get_default_config(&id)?,
                "getCapabilities" => self.on_get_capabilities(&id)?,
                "preprocessFiles" => {
                    self.on_preprocess_files(&id, envelope.payload.unwrap_or(json!({})))?
                }
                "analyzeFile" => {
                    self.on_analyze_file(&id, envelope.payload.unwrap_or(json!({})))?
                }
                _ => {
                    return Err(anyhow::anyhow!("Unknown message type: {}", msg_type));
                }
            }
        }

        Ok(())
    }

    fn send(&mut self, envelope: &Envelope<serde_json::Value>) {
        let _ = self.out.send(envelope);
    }

    fn on_initialize(&mut self, id: &str, payload: serde_json::Value) -> Result<()> {
        // Extract config from payload
        if let Some(config) = payload.get("rulesetConfig").and_then(|v| v.as_object()) {
            self.config = config.iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
        } else {
            self.config = self.opts.get_default_config();
        }

        // Create the ruleset
        self.ruleset = Some(self.opts.create_ruleset());
        self.initialized = true;

        self.send(&Envelope::res(
            "initialize",
            id.to_string(),
            json!({"ok": true}),
        ));
        Ok(())
    }

    fn on_get_default_config(&mut self, id: &str) -> Result<()> {
        let defaults = self.opts.get_default_config();
        self.send(&Envelope::res(
            "getDefaultConfig",
            id.to_string(),
            serde_json::to_value(defaults)?,
        ));
        Ok(())
    }

    fn on_get_capabilities(&mut self, id: &str) -> Result<()> {
        let mut capabilities = self.opts.get_capabilities();

        // Populate rules from the created ruleset
        let ruleset = self.opts.create_ruleset();
        capabilities.rules = ruleset.rules.iter().map(|rule| RuleInfo {
            id: rule.id().to_string(),
            description: rule.description().to_string(),
        }).collect();

        // Auto-inject rule enable/disable settings
        for rule in &ruleset.rules {
            capabilities.config_settings.push(crate::core::ConfigSetting {
                name: rule.id().to_string(),
                description: format!("Enable or disable the {} rule", rule.id()),
                setting_type: crate::core::ConfigType::Enum,
                default: rule.default_config(),
                required: false,
                allowed_values: Some(vec![
                    serde_json::Value::String("off".to_string()),
                    serde_json::Value::String("warn".to_string()),
                    serde_json::Value::String("error".to_string()),
                ]),
                min: None,
                max: None,
            });
        }

        self.send(&Envelope::res(
            "getCapabilities",
            id.to_string(),
            serde_json::to_value(capabilities)?,
        ));
        Ok(())
    }

    fn on_preprocess_files(&mut self, id: &str, payload: serde_json::Value) -> Result<()> {
        let file_uris: Vec<String> = payload
            .get("fileUris")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        let context = self.opts.preprocess_files(&file_uris)?;

        self.send(&Envelope::res(
            "preprocessFiles",
            id.to_string(),
            serde_json::to_value(context)?,
        ));
        Ok(())
    }

    fn on_analyze_file(&mut self, id: &str, payload: serde_json::Value) -> Result<()> {
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

        if let Some(ruleset) = &self.ruleset {
            let diagnostics = run_ruleset(&uri, &content, ruleset, &self.config);

            // Emit diagnostics event
            self.send(&Envelope::event(
                "diagnostics",
                json!({
                    "uri": uri,
                    "diagnostics": diagnostics
                }),
            ));
        }

        // Send completion response
        self.send(&Envelope::res(
            "analyzeFile",
            id.to_string(),
            json!({"ok": true}),
        ));
        Ok(())
    }

    fn on_shutdown(&mut self, id: &str) -> Result<()> {
        self.send(&Envelope::res(
            "shutdown",
            id.to_string(),
            json!({"ok": true}),
        ));
        Ok(())
    }
}

pub fn enabled_rulesets(cfg: &SharedConfig) -> impl Iterator<Item = (&String, &RulesetCfg)> {
    cfg.get().ruleset.iter().filter(|(_, r)| r.enabled)
}
