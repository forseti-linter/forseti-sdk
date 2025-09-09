# Forseti SDK

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

The Forseti SDK is the foundation for building **engines** and **rulesets** for the Forseti linter ecosystem. It provides a minimal, language-agnostic protocol for communication between linters and engines, along with Rust implementations for building robust linting tools.

## Overview

Forseti uses a protocol-based architecture where:

1. **Linter** queries engine capabilities → file patterns, limits
2. **Linter** discovers files → routes to appropriate engines  
3. **Engine** preprocesses → lightweight metadata (no content loading)
4. **Linter** routes to rulesets → with preprocessing context
5. **Rulesets** load content → on-demand, per file, per rule
6. **Results** aggregated → formatted output

This design enables memory-efficient processing of large codebases and supports multiple programming languages through separate engines.

## Features

- **Protocol-based**: NDJSON over stdin/stdout for cross-language compatibility
- **Memory-efficient**: On-demand file loading, no bulk content processing
- **Extensible**: Plugin architecture for engines and rulesets
- **Type-safe**: Full Rust type definitions for all protocol messages
- **Minimal dependencies**: Only `serde`, `anyhow`, `thiserror`, and `toml`

## Architecture

### Core Components

- **`core`** - Protocol envelopes, NDJSON I/O, common types (Position/Range/Diagnostic)
- **`engine`** - Engine server implementation with capabilities and preprocessing
- **`ruleset`** - Rule trait and ruleset container for memory-efficient execution
- **`linter`** - Engine management, lifecycle, and discovery
- **`config`** - Configuration system with git-based dependencies

### Protocol

Communication uses **NDJSON** (newline-delimited JSON) with versioned envelopes:

```json
{
  "v": 1,
  "kind": "req" | "res" | "event",
  "type": "initialize" | "getCapabilities" | "analyzeFile" | ...,
  "id": "string",
  "payload": { ... }
}
```

#### Message Types

- `initialize` - Bootstrap engine with configuration
- `getDefaultConfig` - Get engine's default configuration  
- `getCapabilities` - Query engine file patterns and limits
- `preprocessFiles` - Process file list, return lightweight context
- `analyzeFile` - Analyze individual files (legacy mode)
- `shutdown` - Clean engine teardown
- `diagnostics` - Emitted results from analysis
- `log` - Optional logging events

## Quick Start

### Building an Engine

```rust
use forseti_sdk::{engine::*, core::*};

struct MyEngine;

impl EngineOptions for MyEngine {
    fn get_default_config(&self) -> EngineConfig {
        EngineConfig::default()
    }
    
    fn load_ruleset(&self, id: &str) -> anyhow::Result<Ruleset> {
        // Load and return your ruleset
        todo!()
    }
    
    fn get_capabilities(&self) -> EngineCapabilities {
        EngineCapabilities {
            engine_id: "my-engine".to_string(),
            version: "1.0.0".to_string(),
            file_patterns: vec!["*.txt".to_string()],
            max_file_size: Some(1024 * 1024), // 1MB
        }
    }
    
    fn preprocess_files(&self, file_uris: &[String]) -> anyhow::Result<PreprocessingContext> {
        // Return lightweight file context
        todo!()
    }
}

fn main() -> anyhow::Result<()> {
    let engine = MyEngine;
    let mut server = EngineServer::new(Box::new(engine));
    server.run_stdio()
}
```

### Building a Rule

```rust
use forseti_sdk::{ruleset::*, core::*};

struct NoTrailingWhitespace;

impl Rule for NoTrailingWhitespace {
    fn id(&self) -> &'static str {
        "no-trailing-whitespace"
    }
    
    fn check(&self, ctx: &mut RuleContext) {
        let index = LineIndex::new(ctx.text);
        
        for (line_num, line) in ctx.text.lines().enumerate() {
            if line.ends_with(' ') || line.ends_with('\t') {
                let start = Position { line: line_num, character: line.trim_end().len() };
                let end = Position { line: line_num, character: line.len() };
                
                ctx.diagnostics.push(Diagnostic {
                    rule_id: self.id().to_string(),
                    message: "Trailing whitespace found".to_string(),
                    severity: "warn".to_string(),
                    range: Range { start, end },
                    code: None,
                    suggest: None,
                    docs_url: None,
                });
            }
        }
    }
}

// Bundle into a ruleset
let ruleset = Ruleset::new("my-rules")
    .with_rule(Box::new(NoTrailingWhitespace));
```

### Engine Management

```rust
use forseti_sdk::linter::*;

let mut manager = EngineManager::new("/path/to/cache");
let engines = manager.discover_engines()?;

// Start an engine
manager.start_engine("my-engine", Some(config))?;

// Analyze files
let result = manager.analyze_file("my-engine", "file.txt", content)?;

// Cleanup
manager.shutdown_all()?;
```

## Configuration

Engines accept configuration in this format:

```toml
[engines.my-engine]
enabled = true

[engines.my-engine.rulesets.my-rules]
no-trailing-whitespace = "warn"
max-line-length = ["error", { limit = 100 }]
some-rule = "off"
```

Rules can be configured as:
- `"off"` | `"warn"` | `"error"` - Simple severity levels
- `[level, options]` - Severity with custom options
- `{ ...options }` - Options object (implies enabled)

## Development

### Building

```bash
cargo build                    # Build the SDK
cargo test                     # Run tests
cargo clippy                   # Lint code
cargo fmt                      # Format code
```

### Testing Rules

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_trailing_whitespace() {
        let rule = NoTrailingWhitespace;
        let mut ctx = RuleContext {
            uri: "test.txt",
            text: "hello   \nworld",
            options: &serde_json::Value::Null,
            diagnostics: Vec::new(),
        };
        
        rule.check(&mut ctx);
        
        assert_eq!(ctx.diagnostics.len(), 1);
        assert_eq!(ctx.diagnostics[0].rule_id, "no-trailing-whitespace");
    }
}
```

## Examples

The [forseti-engine-base](../forseti_engine_base/) provides a complete example of:
- Engine implementation with multiple rulesets
- Text processing rules (trailing whitespace, line length, etc.)
- Configuration handling
- Error management

## Cross-Language Support

The NDJSON protocol is language-agnostic. Engines can be implemented in any language that can:
- Read/write NDJSON over stdin/stdout
- Parse the envelope format
- Implement the required message types

## Contributing

1. Follow the existing code style
2. Add tests for new functionality
3. Update documentation as needed
4. Ensure `cargo clippy` passes without warnings

## License

MIT License - see [LICENSE](../LICENSE) for details.

## Related Projects

- [**forseti**](../forseti/) - Main linter CLI
- [**forseti-engine-base**](../forseti_engine_base/) - Base engine with fundamental text rules
- **Forseti workspace** - Complete linting ecosystem

---

For detailed protocol specifications and advanced usage, see [CLAUDE.md](./CLAUDE.md).