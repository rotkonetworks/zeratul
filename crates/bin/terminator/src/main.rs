//! Terminator - High-performance TUI trading terminal
//!
//! Mouse-controlled, resizable panels for professional trading
//! Optimized for full-screen trading experience

use anyhow::Result;
use terminator::shell::tui::TuiShell;

#[tokio::main]
async fn main() -> Result<()> {
    // Print startup messages
    eprintln!("🚀 Terminator - Penumbra DEX Trading Terminal");
    eprintln!("Loading wallet from pcli...");

    // Create and run TUI shell
    match TuiShell::new().await {
        Ok(mut shell) => {
            eprintln!("✓ Initialized");
            eprintln!("");

            // Run the shell
            if let Err(e) = shell.run().await {
                eprintln!("Error: {:?}", e);
            }
        }
        Err(e) => {
            eprintln!("Failed to initialize: {}", e);
            return Err(e);
        }
    }

    Ok(())
}
