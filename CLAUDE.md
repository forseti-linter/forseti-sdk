# CLAUDE.md — Forseti SDK (Rust, minimal)

This file provides guidance to Claude Code (claude.ai/code) when working with the Forseti SDK.

## Workspace Context

**This is part of the Forseti workspace at:** `/home/digitalfiz/projects/forseti/`

- **Workspace root:** `../` (contains workspace `Cargo.toml` and main `CLAUDE.md`)
- **Main linter CLI:** `../forseti/`
- **Base engine:** `../forseti-engine-base/`
- **This SDK:** Current directory (`forseti-sdk/`)

**Purpose:** This SDK is the foundation for building Forseti **engines** and **rulesets**, and for the main **linter** to orchestrate them. It defines the enhanced wire protocol (NDJSON over stdio), shared types, engine management, and memory-efficient processing flow.

```
1. Linter queries engine capabilities → file patterns, limits
2. Linter discovers files → routes to appropriate engines  
3. Engine preprocesses → lightweight metadata (no content loading)
4. Linter routes to rulesets → with preprocessing context
5. Rulesets load content → on-demand, per file, per rule
6. Results aggregated → formatted output
```

## Repo layout (public surface)

- `src/core.rs` — Protocol envelopes, NDJSON I/O, and common types (Position/Range/Diagnostic, EngineCapabilities, PreprocessingContext).
- `src/engine.rs` — Enhanced `EngineServer` with capabilities query and preprocessing support.
- `src/ruleset.rs` — `Rule` trait, `Ruleset` container, and memory-efficient execution with on-demand loading.
- `src/linter.rs` — Advanced engine management (EngineManager, lifecycle, discovery).
- `src/config.rs` — Configuration system with git-based dependencies and environment overrides.

No macros, no heavy deps — just `serde`/`serde_json` and `anyhow/thiserror` if you choose to use them.

---

## Protocol (wire) overview

**Transport:** NDJSON = one JSON object per line on `stdin`/`stdout`.

**Envelope:**

```json
{
  "v": 1,
  "kind": "req" | "res" | "event",
  "type": "initialize" | "getDefaultConfig" | "getCapabilities" | "preprocessFiles" | "analyzeFile" | "shutdown" | "diagnostics" | "log",
  "id": "string (req/res only)",
  "payload": { ... }   // type-specific
}
```

**Message types (v1):**

- `initialize (req→res)` — engine bootstraps, loads rulesets with provided config.
- `getDefaultConfig (req→res)` — engine returns its suggested EngineConfig.
- `getCapabilities (req→res)` — NEW: engine returns file patterns, version, limits.
- `preprocessFiles (req→res)` — NEW: engine processes file list, returns lightweight context.
- `analyzeFile (req→event+res)` — LEGACY: engine emits a `diagnostics` **event** (async) then a completion **res**.
- `shutdown (req→res)` — engine teardown.
- `diagnostics (event)` — `{ uri, diagnostics: Diagnostic[] }`.
- `log (event)` — `{ level, message }` for observability (optional).

**Versioning:** `v` is an integer. Backward-incompatible changes bump this value and engines should refuse unknown major versions.

---

## Core types

From `core.rs`:

**Protocol Types:**
- `Envelope<T>` — generic message wrapper (`v`, `kind`, `type`, `id?`, `payload?`).
- `Ndjson<W>` + `read_line_value()` — minimal, blocking line I/O.

**Positioning & Diagnostics:**
- `Position` / `Range` — 0-based LSP-like positions.
- `Diagnostic` — `{ ruleId, message, severity, range, code?, suggest?, docsUrl? }`.
- `LineIndex` — maps byte offsets ↔ positions for simple text rules.

**Enhanced Flow Types (NEW):**
- `EngineCapabilities` — `{ engine_id, version, file_patterns, max_file_size? }`
- `PreprocessingContext` — `{ engine_id, files: [FileContext], global_context }`
- `FileContext` — `{ uri, content, language?, context }` (content empty for memory efficiency)
- `RulesetResult` — `{ ruleset_id, engine_id, diagnostics, execution_time_ms, files_processed }`
- `LintResults` — aggregated results with summary statistics

> Note: `severity` is `"error" | "warn" | "info"` by convention, but kept as `String` for flexibility.

---

## Engine API

From `engine.rs`:

```rust
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngineConfig {
  pub enabled: Option<bool>,
  pub rulesets: Option<HashMap<String, serde_json::Value>>,
// rulesets: { "<ruleset-id>": { "<rule-id>": "off" | "warn" | "error" | [level, options] | optionsObj } }
}
```

- **Enablement**: `enabled` defaults to `true` if unspecified.
- **Ruleset config**: For each ruleset, map rule IDs to either:
  - `"off" | "warn" | "error"`,
  - `[level, { ...options... }]`, or
  - `{ ...options... }` (implies enabled with default severity on the engine side).

**Enhanced Server:**

```rust
pub trait EngineOptions {
  fn get_default_config(&self) -> EngineConfig;
  fn load_ruleset(&self, id: &str) -> anyhow::Result<Ruleset>;
  fn get_capabilities(&self) -> EngineCapabilities;        // NEW
  fn preprocess_files(&self, file_uris: &[String]) -> anyhow::Result<PreprocessingContext>; // NEW
}

pub struct EngineServer { ... }
```

**Enhanced Lifecycle handled by `EngineServer`:**

- `initialize` — merges user config with `get_default_config` and calls `load_ruleset` for each configured ruleset.
- `getDefaultConfig` — returns `EngineOptions::get_default_config`.
- `getCapabilities` — NEW: returns `EngineOptions::get_capabilities` (file patterns, version, limits).
- `preprocessFiles` — NEW: calls `EngineOptions::preprocess_files` with file list, returns lightweight context.
- `analyzeFile` — LEGACY: runs all active rules across loaded rulesets, emits one `diagnostics` event, then an OK response.
- `shutdown` — clears state and replies OK.

**Merging config:**

- User config overlays engine defaults (shallow merge for `rulesets`).

---

## Enhanced Ruleset API

From `ruleset.rs`:

```rust
pub trait Rule: Send + Sync {
  fn id(&self) -> &'static str;
  fn check(&self, ctx: &mut RuleContext);
}

pub struct RuleContext<'a> {
  pub uri: &'a str,
  pub text: &'a str,
  pub options: &'a serde_json::Value,
  pub diagnostics: Vec<Diagnostic>,
}
```

**Execution Methods:**
- `run_ruleset(...)` — LEGACY: executes rules on single file text
- `run_ruleset_with_context(...)` — NEW: executes rules with preprocessing context, loads content on-demand

**Memory-Efficient Design:**
- Implement `check` to inspect `ctx.text` and `ctx.report(...)` diagnostics.
- Use `LineIndex` (from `core.rs`) to compute `Range`s if needed.  
- `Ruleset` is just an ID plus a list of `Box<dyn Rule>`.
- NEW: Content loaded on-demand per file, not bulk loaded for memory efficiency.

**Example rule sketch:**

```rust
struct NoTrailingWhitespace;
impl Rule for NoTrailingWhitespace {
  fn id(&self) -> &'static str { "no-trailing-whitespace" }
  fn check(&self, ctx: &mut RuleContext) {
    // inspect ctx.text, push ctx.diagnostics with Range + message
  }
}
```

**Bundling:**

```rust
let rs = Ruleset::new("@acme/text")
  .with_rule(Box::new(NoTrailingWhitespace));
```

---

## Engine Management

From `linter.rs`:

**Core Types:**
- `EngineInfo` — metadata about available engines (ID, binary path, supported patterns)
- `EngineHandle` — manages a running engine instance with lifecycle and communication
- `EngineManager` — orchestrates multiple engines with discovery, startup, and cleanup
- `EngineAnalysisResult` — results from analyzing files with engines

**Key Features:**
- **Auto-discovery:** Finds installed engines in cache directories
- **Lifecycle management:** Start, initialize, analyze files, shutdown
- **Idle cleanup:** Automatically shuts down unused engines after timeout
- **Multi-engine support:** Route files to appropriate engines
- **Error resilience:** Handles engine crashes and communication failures

**Basic Usage:**
```rust
let mut manager = EngineManager::new(cache_dir);
let engines = manager.discover_engines()?;
manager.start_engine("base", Some(config))?;
let result = manager.analyze_file("base", "file.txt", content)?;
manager.shutdown_all()?;
```

**Legacy Support:**
- `EngineProcess` — original simple process wrapper (kept for compatibility)

---

## Example NDJSON flow

**Host → Engine**

```json
{
  "v": 1,
  "kind": "req",
  "type": "initialize",
  "id": "1",
  "payload": {
    "engineId": "simple",
    "workspaceRoot": ".",
    "engineConfig": {
      "enabled": true,
      "rulesets": {
        "@forseti/example": {
          "no-trailing-whitespace": "warn",
          "max-line-length": ["info", { "limit": 100 }]
        }
      }
    }
  }
}
```

**Host → Engine**

```json
{
  "v": 1,
  "kind": "req",
  "type": "analyzeFile",
  "id": "2",
  "payload": {
    "uri": "mem://sample.txt",
    "content": "hello   \nthis is a very very very very very very long line..."
  }
}
```

**Engine → Host (event)**

```json
{
  "v": 1,
  "kind": "event",
  "type": "diagnostics",
  "payload": {
    "uri": "mem://sample.txt",
    "diagnostics": [
      {
        "ruleId": "no-trailing-whitespace",
        "message": "Trailing whitespace",
        "severity": "warn",
        "range": { "start": { "line": 0, "character": 5 }, "end": { "line": 0, "character": 8 } }
      }
    ]
  }
}
```

**Engine → Host (res)**

```json
{ "v": 1, "kind": "res", "type": "analyzeFile", "id": "2", "payload": { "ok": true } }
```

---

## Error handling & logging

- Engines may emit `{"type":"log","payload":{"level":"info|warn|error","message":"..."}}` events.
- For invalid inputs, engines should still send a well-formed `res` with an error payload where possible (e.g., `{ "ok": false, "error": "not_initialized" }`).

---

## Cross-language interop

The protocol is language-agnostic. Other engines (Go, Python, etc.) can mirror the envelope and message shapes. JSON Schemas can be added later; the minimal SDK stays focused on runtime types and I/O.

---

## Testing tips

- Unit test rules with plain strings; use `LineIndex` to assert `Range`s.
- Integration test the engine by feeding NDJSON lines and asserting emitted events/responses.
- For the linter host, mock an engine by writing canned NDJSON to stdout.

---

## Compatibility & stability

- **Protocol v1** is stable for this SDK revision.
- Backward-incompatible changes will bump `v`.
- The SDK keeps config semantics intentionally simple: disabled rules are filtered in the engine before execution.

---

## Roadmap (optional extensions)

- Rule-level severity normalization (`off|warn|error`) into diagnostics.
- Better docs URLs linking + quickfix (`suggest.fix`) helpers.
- Pluggable parser contexts (AST hooks per language).
- Async file I/O and workspace awareness (multi-file rules).
- Dynamic ruleset discovery (dlopen / `cdylib`) behind a feature flag.

---

## TL;DR

- Engines: implement `EngineOptions`, instantiate `EngineServer`, call `run_stdio()`.
- Rulesets: implement `Rule::check`, bundle in a `Ruleset`, return it from `load_ruleset`.
- Linters/hosts: spawn engines, speak NDJSON with the defined envelopes.

Small, predictable, and built for composability.
