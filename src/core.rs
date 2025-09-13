use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{self, BufRead, Write};

pub use crate::config::{
    Config, ConfigError, LinterCfg, LogLevel, OutputFormat, RulesetCfg,
};


pub const PROTOCOL_VERSION: u8 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Req,
    Res,
    Event,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Envelope<T = Value> {
    pub v: u8,
    pub kind: Kind,
    #[serde(rename = "type")]
    pub typ: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<T>,
}

impl<T> Envelope<T> {
    pub fn event(typ: &str, payload: T) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            kind: Kind::Event,
            typ: typ.to_string(),
            id: None,
            payload: Some(payload),
        }
    }
    pub fn res(typ: &str, id: impl Into<String>, payload: T) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            kind: Kind::Res,
            typ: typ.to_string(),
            id: Some(id.into()),
            payload: Some(payload),
        }
    }
    pub fn req(typ: &str, id: impl Into<String>, payload: T) -> Self {
        Self {
            v: PROTOCOL_VERSION,
            kind: Kind::Req,
            typ: typ.to_string(),
            id: Some(id.into()),
            payload: Some(payload),
        }
    }
}

/// Minimal NDJSON writer.
pub struct Ndjson<W: Write> {
    writer: W,
}
impl<W: Write> Ndjson<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }
    pub fn send<S: Serialize>(&mut self, obj: &S) -> io::Result<()> {
        let line = serde_json::to_string(obj)?;
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")?;
        self.writer.flush()
    }
}

/// Read one NDJSON line from stdin as raw JSON.
pub fn read_line_value() -> io::Result<Value> {
    let stdin = io::stdin();
    let mut lock = stdin.lock();
    let mut buf = String::new();
    buf.clear();
    let n = lock.read_line(&mut buf)?;
    if n == 0 {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "stdin closed"));
    }
    let trimmed = buf.trim();
    let value: Value =
        serde_json::from_str(trimmed).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    Ok(value)
}

/// Common position types and diagnostics.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fix {
    pub range: Range,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestFix {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Diagnostic {
    pub rule_id: String,
    pub message: String,
    pub severity: String, // "error" | "warn" | "info"
    pub range: Range,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggest: Option<Vec<SuggestFix>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs_url: Option<String>,
}

/// Utility for line/offset mapping for plain-text rules.
pub struct LineIndex {
    text: String,
    starts: Vec<usize>,
}
impl LineIndex {
    pub fn new(text: &str) -> Self {
        let mut s = vec![0usize];
        for (i, ch) in text.char_indices() {
            if ch == '\n' {
                s.push(i + 1);
            }
        }
        Self {
            text: text.to_string(),
            starts: s,
        }
    }
    pub fn to_pos(&self, mut off: usize) -> Position {
        if off > self.text.len() {
            off = self.text.len();
        }
        // binary search
        let (mut lo, mut hi) = (0usize, self.starts.len().saturating_sub(1));
        while lo <= hi {
            let mid = (lo + hi) / 2;
            let start = self.starts[mid];
            let next = if mid + 1 < self.starts.len() {
                self.starts[mid + 1]
            } else {
                self.text.len() + 1
            };
            if off < start {
                if mid == 0 {
                    break;
                }
                hi = mid - 1;
            } else if off >= next {
                lo = mid + 1;
            } else {
                return Position {
                    line: mid as u32,
                    character: (off - start) as u32,
                };
            }
        }
        Position {
            line: 0,
            character: off as u32,
        }
    }
    pub fn to_range(&self, s: usize, e: usize) -> Range {
        Range {
            start: self.to_pos(s),
            end: self.to_pos(e),
        }
    }
}

/// Information about a single rule
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleInfo {
    pub id: String,
    pub description: String,
}

/// Information about a ruleset and its rules
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesetInfo {
    pub id: String,
    pub rules: Vec<RuleInfo>,
}

/// Configuration setting definition for rulesets
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigSetting {
    /// Setting name/key
    pub name: String,
    /// Human-readable description
    pub description: String,
    /// Data type of the setting
    #[serde(rename = "type")]
    pub setting_type: ConfigType,
    /// Default value
    pub default: Value,
    /// Whether this setting is required
    #[serde(default)]
    pub required: bool,
    /// Allowed values (for enum types)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_values: Option<Vec<Value>>,
    /// Minimum value (for numeric types)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    /// Maximum value (for numeric types)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
}

/// Data types for configuration settings
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigType {
    String,
    Number,
    Integer,
    Boolean,
    Array,
    Object,
    /// One of a set of predefined values
    Enum,
}

/// Ruleset capabilities and metadata (replaces EngineCapabilities)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesetCapabilities {
    pub ruleset_id: String,
    pub version: String,
    pub file_patterns: Vec<String>,
    pub max_file_size: Option<u64>,
    /// Comment prefixes used for annotations (e.g., ["//", "#", "/*"])
    pub annotation_prefixes: Vec<String>,
    /// Rules available in this ruleset
    pub rules: Vec<RuleInfo>,
    /// Default configuration for rules
    pub default_config: HashMap<String, Value>,
    /// Configuration settings that can be customized
    #[serde(default)]
    pub config_settings: Vec<ConfigSetting>,
}


/// File preprocessing context from ruleset
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreprocessingContext {
    pub ruleset_id: String,
    pub files: Vec<FileContext>,
    pub global_context: HashMap<String, Value>, // Cross-file context
}

/// Context for a single file after preprocessing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileContext {
    pub uri: String,
    pub content: String,
    pub language: Option<String>,
    pub context: HashMap<String, Value>, // AST, symbols, etc.
}

/// Ruleset execution result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RulesetResult {
    pub ruleset_id: String,
    pub diagnostics: Vec<Diagnostic>,
    pub execution_time_ms: u64,
    pub files_processed: usize,
}

/// Aggregated linting results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintResults {
    pub results: Vec<RulesetResult>,
    pub total_files: usize,
    pub total_diagnostics: usize,
    pub execution_time_ms: u64,
    pub summary: ResultSummary,
}

/// Summary of linting results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultSummary {
    pub errors: usize,
    pub warnings: usize,
    pub info: usize,
    pub rulesets_used: Vec<String>,
}

/// Annotation scope for ignore directives
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnotationScope {
    /// Ignore the next line only
    NextLine,
    /// Ignore the entire file
    File,
}

/// Parsed annotation directive
#[derive(Debug, Clone)]
pub struct Annotation {
    pub scope: AnnotationScope,
    pub rule_ids: Vec<String>, // Empty means all rules
    pub line: u32,             // Line where annotation appears (0-based)
}

/// Utility for parsing annotations from text
pub struct AnnotationParser {
    prefixes: Vec<String>,
}

impl AnnotationParser {
    pub fn new(prefixes: Vec<String>) -> Self {
        Self { prefixes }
    }

    /// Parse all annotations from text content
    pub fn parse_annotations(&self, text: &str) -> Vec<Annotation> {
        let mut annotations = Vec::new();

        for (line_num, line) in text.lines().enumerate() {
            if let Some(annotation) = self.parse_line_annotation(line, line_num as u32) {
                annotations.push(annotation);
            }
        }

        annotations
    }

    /// Parse a single line for annotation directives
    fn parse_line_annotation(&self, line: &str, line_num: u32) -> Option<Annotation> {
        let trimmed = line.trim();

        // Check if line starts with any of the comment prefixes
        let comment_start = self
            .prefixes
            .iter()
            .find(|prefix| trimmed.starts_with(*prefix))?;

        // Extract comment content after the prefix
        let comment_content = trimmed.strip_prefix(comment_start)?.trim();

        // Look for forseti-ignore patterns
        if let Some(ignore_content) = comment_content.strip_prefix("forseti-ignore") {
            let remaining = ignore_content.trim();

            // Check for scope indicators
            let (scope, rule_part) = if remaining.starts_with("-file") {
                (
                    AnnotationScope::File,
                    remaining.strip_prefix("-file").unwrap_or("").trim(),
                )
            } else if remaining.starts_with("-next-line") {
                (
                    AnnotationScope::NextLine,
                    remaining.strip_prefix("-next-line").unwrap_or("").trim(),
                )
            } else if remaining.is_empty() {
                // Default to next-line if no scope specified
                (AnnotationScope::NextLine, "")
            } else {
                // No scope prefix, default to next-line and treat as rule list
                (AnnotationScope::NextLine, remaining)
            };

            // Parse rule IDs (comma-separated)
            let rule_ids = if rule_part.is_empty() {
                Vec::new() // Empty means ignore all rules
            } else {
                rule_part
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            };

            return Some(Annotation {
                scope,
                rule_ids,
                line: line_num,
            });
        }

        None
    }

    /// Check if a rule should be ignored for a specific line
    pub fn should_ignore_rule(&self, annotations: &[Annotation], rule_id: &str, line: u32) -> bool {
        for annotation in annotations {
            match annotation.scope {
                AnnotationScope::File => {
                    // File-level ignores apply to all lines
                    if annotation.rule_ids.is_empty()
                        || annotation.rule_ids.contains(&rule_id.to_string())
                    {
                        return true;
                    }
                }
                AnnotationScope::NextLine => {
                    // Next-line ignores apply only to the line immediately following the annotation
                    if line == annotation.line + 1 {
                        if annotation.rule_ids.is_empty()
                            || annotation.rule_ids.contains(&rule_id.to_string())
                        {
                            return true;
                        }
                    }
                }
            }
        }
        false
    }
}

#[derive(Clone)]
pub struct SharedConfig(pub std::sync::Arc<Config>);

impl SharedConfig {
    /// Stable borrow tied to &self (no temporaries).
    pub fn get(&self) -> &Config {
        &self.0
    }

    /// If you really need to clone the Arc.
    pub fn clone_arc(&self) -> std::sync::Arc<Config> {
        self.0.clone()
    }
}
