use crate::core::{Annotation, AnnotationParser, Diagnostic, PreprocessingContext};
use crate::core::{RulesetCfg, SharedConfig};
use serde_json::Value;

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
    fn check(&self, ctx: &mut RuleContext);
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

pub fn enabled_rulesets(cfg: &SharedConfig) -> impl Iterator<Item = (&String, &RulesetCfg)> {
    cfg.get().ruleset.iter().filter(|(_, r)| r.enabled)
}
