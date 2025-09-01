use forseti_sdk::linter::EngineManager;
use std::path::PathBuf;

/// Demonstrates the enhanced engine management functionality
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cache_dir = PathBuf::from("~/.forseti/cache");

    println!("🔍 Engine Manager Demo");
    println!("======================");

    // Create engine manager
    let mut manager = EngineManager::new(cache_dir.clone());

    // Discover available engines
    println!("📦 Discovering engines in: {}", cache_dir.display());
    let engines = manager.discover_engines()?;

    if engines.is_empty() {
        println!("❌ No engines found. Install some engines first with:");
        println!("   forseti install");
        return Ok(());
    }

    println!("✅ Found {} engine(s):", engines.len());
    for engine in &engines {
        println!("   - {} ({})", engine.id, engine.binary_path.display());
    }

    // Try to start the first engine
    if let Some(first_engine) = engines.first() {
        println!("🚀 Starting engine: {}", first_engine.id);

        match manager.start_engine(&first_engine.id, None) {
            Ok(_) => {
                println!("✅ Engine started successfully");

                // Analyze a sample file
                let sample_content = "Hello world   \nThis is a test file\n";
                let uri = "demo://sample.txt";

                println!("🔍 Analyzing sample content with engine...");

                match manager.analyze_file(&first_engine.id, uri, sample_content) {
                    Ok(result) => {
                        println!("✅ Analysis completed in {:?}", result.duration);
                        println!("   Found {} diagnostic(s):", result.diagnostics.len());

                        for (i, diagnostic) in result.diagnostics.iter().enumerate() {
                            println!(
                                "   {}. [{}] {} (line {}, col {})",
                                i + 1,
                                diagnostic.severity,
                                diagnostic.message,
                                diagnostic.range.start.line,
                                diagnostic.range.start.character
                            );
                        }
                    }
                    Err(e) => println!("❌ Analysis failed: {}", e),
                }

                // Shutdown the engine
                println!("🛑 Shutting down engine...");
                manager.shutdown_engine(&first_engine.id)?;
                println!("✅ Engine shutdown complete");
            }
            Err(e) => println!("❌ Failed to start engine: {}", e),
        }
    }

    println!("✅ Demo completed");
    Ok(())
}
