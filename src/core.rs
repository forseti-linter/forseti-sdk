use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{self, BufRead, Write};

pub use crate::config::{
    Config, ConfigError, EngineCfg, LinterCfg, LogLevel, OutputFormat, RulesetCfg,
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

/// Engine capabilities and metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineCapabilities {
    pub engine_id: String,
    pub version: String,
    pub file_patterns: Vec<String>,
    pub max_file_size: Option<u64>,
}

/// File preprocessing context from engine
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreprocessingContext {
    pub engine_id: String,
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
    pub engine_id: String,
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
    pub engines_used: Vec<String>,
    pub rulesets_used: Vec<String>,
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
