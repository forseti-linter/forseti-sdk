use crate::core::{Diagnostic, Envelope};
use crate::engine::EngineConfig;
use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

/// Information about an available engine
#[derive(Debug, Clone)]
pub struct EngineInfo {
    pub id: String,
    pub binary_path: PathBuf,
    pub version: Option<String>,
    pub supported_file_patterns: Vec<String>,
}

/// Handle to a running engine process
pub struct EngineHandle {
    pub info: EngineInfo,
    process: EngineProcess,
    initialized: bool,
    last_activity: Instant,
    request_counter: u64,
}

/// Manages multiple engine processes
pub struct EngineManager {
    engines: HashMap<String, EngineHandle>,
    cache_dir: PathBuf,
    timeout: Duration,
}

/// Result from analyzing a file with an engine
#[derive(Debug)]
pub struct EngineAnalysisResult {
    pub engine_id: String,
    pub uri: String,
    pub diagnostics: Vec<Diagnostic>,
    pub duration: Duration,
}

/// Basic engine process wrapper (kept for backward compatibility)
pub struct EngineProcess {
    #[allow(dead_code)]
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl EngineInfo {
    /// Create engine info by probing a binary
    pub fn from_binary(binary_path: PathBuf) -> Result<Self> {
        let id = binary_path
            .file_stem()
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("Invalid binary filename"))?
            .to_string();

        // Remove forseti_ prefix if present
        let id = if id.starts_with("forseti_") {
            id.strip_prefix("forseti_").unwrap_or(&id).to_string()
        } else {
            id
        };

        Ok(Self {
            id,
            binary_path,
            version: None, // Could probe with --version flag in future
            supported_file_patterns: vec!["*".to_string()], // Default to all files
        })
    }
}

impl EngineHandle {
    /// Create a new engine handle and start the process
    pub fn new(info: EngineInfo, config: Option<EngineConfig>) -> Result<Self> {
        let process = EngineProcess::spawn(info.binary_path.to_str().unwrap(), &[])
            .context("Failed to spawn engine process")?;

        let mut handle = Self {
            info,
            process,
            initialized: false,
            last_activity: Instant::now(),
            request_counter: 0,
        };

        // Initialize the engine
        handle.initialize(config)?;
        Ok(handle)
    }

    /// Initialize the engine with configuration
    fn initialize(&mut self, config: Option<EngineConfig>) -> Result<()> {
        let config = config.unwrap_or_default();
        let request_id = self.next_request_id();

        let init_msg = Envelope::req(
            "initialize",
            request_id.clone(),
            json!({
                "engineId": self.info.id,
                "workspaceRoot": ".",
                "engineConfig": config
            }),
        );

        self.send_message(&init_msg)?;
        let response = self.read_response()?;

        // Verify initialization success
        if response
            .get("payload")
            .and_then(|p| p.get("ok"))
            .and_then(|ok| ok.as_bool())
            .unwrap_or(false)
        {
            self.initialized = true;
            Ok(())
        } else {
            Err(anyhow!("Engine initialization failed: {:?}", response))
        }
    }

    /// Analyze a file with this engine
    pub fn analyze_file(&mut self, uri: &str, content: &str) -> Result<EngineAnalysisResult> {
        if !self.initialized {
            return Err(anyhow!("Engine not initialized"));
        }

        let start = Instant::now();
        let request_id = self.next_request_id();

        let analyze_msg = Envelope::req(
            "analyzeFile",
            request_id.clone(),
            json!({
                "uri": uri,
                "content": content
            }),
        );

        self.send_message(&analyze_msg)?;

        // Read diagnostics event
        let diagnostics_event = self.read_response()?;
        let diagnostics =
            if diagnostics_event.get("type").and_then(|t| t.as_str()) == Some("diagnostics") {
                diagnostics_event
                    .get("payload")
                    .and_then(|p| p.get("diagnostics"))
                    .and_then(|d| serde_json::from_value(d.clone()).ok())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };

        // Read completion response
        let _completion = self.read_response()?;

        self.last_activity = Instant::now();

        Ok(EngineAnalysisResult {
            engine_id: self.info.id.clone(),
            uri: uri.to_string(),
            diagnostics,
            duration: start.elapsed(),
        })
    }

    /// Shutdown the engine gracefully
    pub fn shutdown(&mut self) -> Result<()> {
        if !self.initialized {
            return Ok(());
        }

        let request_id = self.next_request_id();
        let shutdown_msg = Envelope::req("shutdown", request_id, json!({}));

        self.send_message(&shutdown_msg)?;
        let _response = self.read_response()?;

        self.initialized = false;
        Ok(())
    }

    /// Check if engine has been idle for too long
    pub fn is_idle(&self, timeout: Duration) -> bool {
        self.last_activity.elapsed() > timeout
    }

    fn next_request_id(&mut self) -> String {
        self.request_counter += 1;
        format!("{}_{}", self.info.id, self.request_counter)
    }

    fn send_message<T: serde::Serialize>(&mut self, msg: &T) -> Result<()> {
        let json_str = serde_json::to_string(msg).context("Failed to serialize message")?;
        self.process
            .send_line(&json_str)
            .context("Failed to send message to engine")?;
        Ok(())
    }

    fn read_response(&mut self) -> Result<Value> {
        let line = self
            .process
            .read_line()
            .context("Failed to read response from engine")?;
        serde_json::from_str(line.trim()).context("Failed to parse JSON response")
    }
}

impl EngineManager {
    /// Create a new engine manager
    pub fn new(cache_dir: PathBuf) -> Self {
        Self {
            engines: HashMap::new(),
            cache_dir,
            timeout: Duration::from_secs(300), // 5 minutes idle timeout
        }
    }

    /// Discover available engines in the cache directory
    pub fn discover_engines(&self) -> Result<Vec<EngineInfo>> {
        let mut engines = Vec::new();

        if !self.cache_dir.exists() {
            return Ok(engines);
        }

        // Look for engines in cache_dir/*/bin/forseti_engine_*
        for entry in fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            let engine_dir = entry.path();

            if !engine_dir.is_dir() {
                continue;
            }

            let bin_dir = engine_dir.join("bin");
            if !bin_dir.exists() {
                continue;
            }

            // Look for engine binaries
            for bin_entry in fs::read_dir(&bin_dir)? {
                let bin_entry = bin_entry?;
                let binary_path = bin_entry.path();

                if !binary_path.is_file() {
                    continue;
                }

                let filename = binary_path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");

                // Match engine binaries (forseti_engine_*)
                if filename.starts_with("forseti_engine_")
                    && let Ok(info) = EngineInfo::from_binary(binary_path) {
                        engines.push(info);
                    }
            }
        }

        Ok(engines)
    }

    /// Start an engine with the given configuration
    pub fn start_engine(&mut self, engine_id: &str, config: Option<EngineConfig>) -> Result<()> {
        if self.engines.contains_key(engine_id) {
            return Ok(()); // Already running
        }

        // Find the engine info
        let engines = self.discover_engines()?;
        let engine_info = engines
            .into_iter()
            .find(|e| e.id == engine_id)
            .ok_or_else(|| anyhow!("Engine '{}' not found", engine_id))?;

        // Start the engine
        let handle = EngineHandle::new(engine_info, config).context("Failed to start engine")?;

        self.engines.insert(engine_id.to_string(), handle);
        Ok(())
    }

    /// Analyze a file with a specific engine
    pub fn analyze_file(
        &mut self,
        engine_id: &str,
        uri: &str,
        content: &str,
    ) -> Result<EngineAnalysisResult> {
        let handle = self
            .engines
            .get_mut(engine_id)
            .ok_or_else(|| anyhow!("Engine '{}' not running", engine_id))?;

        handle.analyze_file(uri, content)
    }

    /// Analyze a file with all running engines
    pub fn analyze_file_all(&mut self, uri: &str, content: &str) -> Vec<EngineAnalysisResult> {
        let mut results = Vec::new();

        // Clone the keys to avoid borrow checker issues
        let engine_ids: Vec<String> = self.engines.keys().cloned().collect();

        for engine_id in engine_ids {
            if let Ok(result) = self.analyze_file(&engine_id, uri, content) {
                results.push(result);
            }
        }

        results
    }

    /// Shutdown a specific engine
    pub fn shutdown_engine(&mut self, engine_id: &str) -> Result<()> {
        if let Some(mut handle) = self.engines.remove(engine_id) {
            handle.shutdown()?;
        }
        Ok(())
    }

    /// Shutdown all engines
    pub fn shutdown_all(&mut self) -> Result<()> {
        let engine_ids: Vec<String> = self.engines.keys().cloned().collect();
        for engine_id in engine_ids {
            self.shutdown_engine(&engine_id)?;
        }
        Ok(())
    }

    /// Clean up idle engines
    pub fn cleanup_idle_engines(&mut self) -> Result<()> {
        let idle_engines: Vec<String> = self
            .engines
            .iter()
            .filter(|(_, handle)| handle.is_idle(self.timeout))
            .map(|(id, _)| id.clone())
            .collect();

        for engine_id in idle_engines {
            self.shutdown_engine(&engine_id)?;
        }

        Ok(())
    }

    /// Get list of running engines
    pub fn running_engines(&self) -> Vec<&str> {
        self.engines.keys().map(|s| s.as_str()).collect()
    }

    /// Set idle timeout for engines
    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }
}

// Keep the original EngineProcess for backward compatibility
impl EngineProcess {
    pub fn spawn(cmd: &str, args: &[&str]) -> std::io::Result<Self> {
        let mut child = Command::new(cmd)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take().expect("stdin");
        let stdout = BufReader::new(child.stdout.take().expect("stdout"));
        Ok(Self {
            child,
            stdin,
            stdout,
        })
    }

    pub fn send_line(&mut self, line: &str) -> std::io::Result<()> {
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()
    }

    /// Blocking read of one NDJSON line from engine stdout.
    pub fn read_line(&mut self) -> std::io::Result<String> {
        let mut buf = String::new();
        self.stdout.read_line(&mut buf)?;
        Ok(buf)
    }
}
